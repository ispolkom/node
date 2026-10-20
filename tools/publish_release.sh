#!/usr/bin/env bash
# Подписанный выпуск пакета узла для двух каналов обновления: сайт (папка /update/deb/) и GitHub Releases.
#
# Что делает: собирает .deb (tools/build_deb.sh) → резервирует номер выпуска (release/SEQ) → манифест MANIFEST.json (версия, номер,
# SHA-256, платформа, срок действия, описание) → подпись ключом из фразы выпусков → раскладывает:
#   site/update/deb/ (корень выпуска)       — то, что заливается на сайт как есть (папка update/deb/ на yandi.su)
#   dist/github/                 — ассеты для релиза на GitHub, имена с префиксом «deb-» (deb-MANIFEST.json, deb-….deb)
#   archive/release_<версия>/    — архив для заливки на сайт, пакеты и памятка (корень выпуска)
#
# Фраза выпусков — две одинаковые строки на stdin (скрытый ввод в терминале); она нигде не сохраняется. Ключ подписи выводится из
# неё Argon2id (отдельная соль: это НЕ ключ APK). Открытый ключ закреплён в release/yandi-release-pubkey.hex.
#
#   bash tools/publish_release.sh                    собрать пакет из готового release-бинаря, подписать, разложить
#   bash tools/publish_release.sh --build            перед этим выполнить cargo build --release --offline
#   YANDI_RELEASE_NOTES="что нового"  YANDI_RELEASE_EXPIRES_DAYS=120  — описание и срок действия (по умолчанию 120 дней)
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/paths.sh
BUILD=0; NODE_ONLY=0
for a in "$@"; do case "$a" in --build) BUILD=1;; --node-only) NODE_ONLY=1;; *) echo "неизвестный ключ: $a"; exit 2;; esac; done
PUB=release/yandi-release-pubkey.hex
[ -f "$PUB" ] || { echo "нет $PUB"; exit 1; }
# один выпуск за раз (общий замок с node_release.sh): номер, site/ и github/ не делятся между параллельными запусками
[ ! -L release/.lock ] || { echo 'Недопустимый lock'; exit 1; }
exec 9>release/.lock
flock -x 9
if [ "$BUILD" = 1 ]; then
  env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY cargo build --release --offline --workspace --bins
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

bash tools/build_deb.sh >/dev/null || { echo "не собрался пакет узла: запустите bash tools/build_deb.sh и смотрите ошибку"; exit 1; }
DEB=$(ls -t dist/yandi-node_*_*.deb | head -1)
NAME=$(basename "$DEB")
VERSION=$(dpkg-deb --field "$DEB" Version)
# оболочка ПК — вторым пакетом той же версии (если собрана flutter build linux --release)
DDEB=""; DNAME=""
BUNDLE_BIN=desktop/yandi_desktop/build/linux/x64/release/bundle/yandi_desktop
# код Dart лежит в libapp.so: файл-запускатель при правках Dart не меняется, сверять надо с ним
BUNDLE_APP=desktop/yandi_desktop/build/linux/x64/release/bundle/lib/libapp.so
if [ "$NODE_ONLY" = 1 ]; then
  echo "режим --node-only: выпуск БЕЗ оболочки ПК (так задумано)"
else
  [ -x "$BUNDLE_BIN" ] && [ -f "$BUNDLE_APP" ] || { echo "нет собранной оболочки $BUNDLE_BIN: соберите (--build) или объявите выпуск только узла ключом --node-only"; exit 1; }
  # бандл должен быть новее исходников оболочки: иначе под новой версией подписалась бы старая сборка
  STALE=$(find desktop/yandi_desktop/lib desktop/yandi_desktop/linux desktop/yandi_desktop/assets desktop/yandi_desktop/pubspec.yaml desktop/yandi_desktop/pubspec.lock -type f -newer "$BUNDLE_APP" 2>/dev/null | { grep -v '/ephemeral/' || true; } | head -3)
  [ -z "$STALE" ] || { echo "оболочка пересобрана не после правок исходников (например: $STALE): выполните --build"; exit 1; }
  bash tools/build_desktop_deb.sh >/dev/null || { echo "не собрался пакет оболочки: запустите bash tools/build_desktop_deb.sh"; exit 1; }
  DDEB="dist/yandi-desktop_${VERSION//+/-}_$(dpkg --print-architecture).deb"
  [ -f "$DDEB" ] || { echo "не собрался $DDEB"; exit 1; }
  DNAME=$(basename "$DDEB")
fi
SEQ=$(python3 -I tools/release_seq.py release)

SITE="$SITE_DIR/update/deb"
GH="$GH_DIR"
rm -rf "$SITE" "$GH"
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

OUT="$ARCHIVE_DIR"; mkdir -p "$OUT"; VSAFE="${VERSION//+/-}"
cat > "$OUT/ПАМЯТКА-${VSAFE}.txt" <<TXT
Выпуск ${VERSION} (номер ${SEQ}), платформа deb.

САЙТ (yandi.su):
  Файлы лежат в site/update/deb/ (корень выпуска). Архив всего сайта делает tools/pack_site.sh (в ~/Загрузки).
  На сайте файлы должны лежать по адресу https://yandi.su/update/deb/ (MANIFEST.json, MANIFEST.json.sig, ${NAME}).
  Старые файлы в update/deb/ заменить целиком: манифест и пакет должны быть одной версии.

GITHUB:
  Создать релиз (Releases → Draft a new release) и приложить файлы из папки github/ (корень выпуска)
  (deb-MANIFEST.json, deb-MANIFEST.json.sig, deb-${NAME}). Релиз должен быть «latest».

УСТАНОВКА ВРУЧНУЮ:
  sudo apt install ./${NAME}
  Проверка файла перед установкой: yandi-release verify <папка с файлами> --pubkey $(cat "$PUB") --min-seq 0
TXT
echo "готово: версия ${VERSION}, номер ${SEQ}"
ls -la "$OUT" "$SITE"
