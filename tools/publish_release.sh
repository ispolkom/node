#!/usr/bin/env bash
# Подписанный выпуск пакета узла для двух каналов обновления: сайт (папка /update/deb/) и GitHub Releases.
#
# Что делает: собирает .deb (tools/build_deb.sh) → резервирует номер выпуска (release/SEQ) → манифест MANIFEST.json (версия, номер,
# SHA-256, платформа, срок действия, описание) → подпись ключом из фразы выпусков → раскладывает:
#   dist/site/update/deb/        — то, что заливается на сайт как есть (папка update/deb/ на yandi.su)
#   dist/github/                 — ассеты для релиза на GitHub, имена с префиксом «deb-» (deb-MANIFEST.json, deb-….deb)
#   «~/Рабочий стол/Релизы/»     — архив для заливки на сайт, пакет, ассеты GitHub и памятка
#
# Фраза выпусков — две одинаковые строки на stdin (скрытый ввод в терминале); она нигде не сохраняется. Ключ подписи выводится из
# неё Argon2id (отдельная соль: это НЕ ключ APK). Открытый ключ закреплён в release/yandi-release-pubkey.hex.
#
#   bash tools/publish_release.sh                    собрать пакет из готового release-бинаря, подписать, разложить
#   bash tools/publish_release.sh --build            перед этим выполнить cargo build --release --offline
#   YANDI_RELEASE_NOTES="что нового"  YANDI_RELEASE_EXPIRES_DAYS=120  — описание и срок действия (по умолчанию 120 дней)
set -euo pipefail
cd "$(dirname "$0")/.."
BUILD=0; NODE_ONLY=0
for a in "$@"; do case "$a" in --build) BUILD=1;; --node-only) NODE_ONLY=1;; *) echo "неизвестный ключ: $a"; exit 2;; esac; done
PUB=release/yandi-release-pubkey.hex
[ -f "$PUB" ] || { echo "нет $PUB"; exit 1; }
if [ "$BUILD" = 1 ]; then
  env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY cargo build --release --offline --bins
  if [ "$NODE_ONLY" = 0 ]; then
    (cd desktop/yandi_desktop && env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY -u all_proxy -u ALL_PROXY flutter build linux --release)
  fi
fi
REL=target/release/yandi-release
[ -x "$REL" ] || { echo "нет $REL: cargo build --release --offline --bins"; exit 1; }

echo "Фраза выпусков узла (12+ слов; ввод скрыт, дважды):"
read -rs -p "Фраза: " P1; echo
read -rs -p "Ещё раз: " P2; echo
PHRASE_IN=$(printf '%s\n%s\n' "$P1" "$P2")
unset P1 P2
KEY=$(printf '%s\n' "$PHRASE_IN" | "$REL" pubkey) || { echo "фраза отвергнута"; exit 1; }
[ "$KEY" = "$(cat "$PUB")" ] || { echo "Фраза не та: ключ не совпадает с $PUB. Остановлено."; exit 1; }

bash tools/build_deb.sh >/dev/null
DEB=$(ls -t dist/yandi-node_*_*.deb | head -1)
NAME=$(basename "$DEB")
VERSION=$(dpkg-deb --field "$DEB" Version)
# оболочка ПК — вторым пакетом той же версии (если собрана flutter build linux --release)
DDEB=""; DNAME=""
BUNDLE_BIN=desktop/yandi_desktop/build/linux/x64/release/bundle/yandi_desktop
if [ "$NODE_ONLY" = 1 ]; then
  echo "режим --node-only: выпуск БЕЗ оболочки ПК (так задумано)"
else
  [ -x "$BUNDLE_BIN" ] || { echo "нет собранной оболочки $BUNDLE_BIN: соберите (--build) или объявите выпуск только узла ключом --node-only"; exit 1; }
  # бандл должен быть новее исходников оболочки: иначе под новой версией подписалась бы старая сборка
  STALE=$(find desktop/yandi_desktop/lib desktop/yandi_desktop/linux desktop/yandi_desktop/pubspec.yaml -type f -newer "$BUNDLE_BIN" 2>/dev/null | grep -v '/ephemeral/' | head -3)
  [ -z "$STALE" ] || { echo "оболочка пересобрана не после правок исходников (например: $STALE): выполните --build"; exit 1; }
  bash tools/build_desktop_deb.sh >/dev/null
  DDEB="dist/yandi-desktop_${VERSION//+/-}_$(dpkg --print-architecture).deb"
  [ -f "$DDEB" ] || { echo "не собрался $DDEB"; exit 1; }
  DNAME=$(basename "$DDEB")
fi
SEQ=$(python3 -I tools/release_seq.py release)

SITE=dist/site/update/deb
GH=dist/github
rm -rf dist/site dist/github
mkdir -p "$SITE" "$GH"
cp "$DEB" "$SITE/$NAME"
FILES=("$NAME")
if [ -n "$DDEB" ]; then cp "$DDEB" "$SITE/$DNAME"; FILES+=("$DNAME"); fi
YANDI_RELEASE_PLATFORM=deb "$REL" manifest "$SITE" "$VERSION" "$SEQ" --pubkey "$KEY" "${FILES[@]}"
printf '%s\n' "$PHRASE_IN" | "$REL" sign "$SITE"
unset PHRASE_IN
# самопроверка тем же инструментом, как это сделает установщик
"$REL" verify "$SITE" --pubkey "$KEY" --min-seq "$SEQ"

# ассеты GitHub: те же файлы с префиксом «deb-»
for f in MANIFEST.json MANIFEST.json.sig "${FILES[@]}"; do cp "$SITE/$f" "$GH/deb-$f"; done
(cd "$GH" && sha256sum deb-* > SHA256SUMS)

OUT="$HOME/Рабочий стол/Релизы"
mkdir -p "$OUT"
VSAFE="${VERSION//+/-}"
ARCH="yandi-update-site_${VSAFE}.tar.gz"
tar -C dist/site --sort=name --owner=0 --group=0 --numeric-owner -czf "$OUT/$ARCH" update
cp "$DEB" "$OUT/$NAME"
[ -n "$DDEB" ] && cp "$DDEB" "$OUT/$DNAME"
# плоский архив ровно для заливки в update/deb/: только файлы, без вложенных папок
ZIP="$HOME/Рабочий стол/yandi-su_update-deb.zip"
rm -f "$ZIP"
(cd "$SITE" && zip -q -X "$ZIP" MANIFEST.json MANIFEST.json.sig "${FILES[@]}")
rm -rf "$OUT/github-assets_${VSAFE}" && cp -r "$GH" "$OUT/github-assets_${VSAFE}"
cat > "$OUT/ПАМЯТКА.txt" <<TXT
Выпуск ${VERSION} (номер ${SEQ}), платформа deb.

САЙТ (yandi.su):
  1. Распаковать ${ARCH}: получится папка update/ с подпапкой deb/.
  2. Залить update/deb/ на сайт так, чтобы файлы лежали по адресу https://yandi.su/update/deb/
     (MANIFEST.json, MANIFEST.json.sig, ${NAME}).
  Старые файлы в update/deb/ заменить целиком: манифест и пакет должны быть одной версии.

GITHUB:
  Создать релиз (Releases → Draft a new release) и приложить файлы из папки github-assets_${VSAFE}/
  (deb-MANIFEST.json, deb-MANIFEST.json.sig, deb-${NAME}). Релиз должен быть «latest».

УСТАНОВКА ВРУЧНУЮ:
  sudo apt install ./${NAME}
  Проверка файла перед установкой: yandi-release verify <папка с файлами> --pubkey $(cat "$PUB") --min-seq 0
TXT
echo "готово: версия ${VERSION}, номер ${SEQ}"
ls -la "$OUT"
