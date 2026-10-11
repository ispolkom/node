#!/usr/bin/env bash
# Подписанный выпуск ноды (Linux): сборка → архив → манифест → подпись ключом из фразы → самопроверка.
#
# Фразу вводит сам разработчик (скрыто, дважды); она не сохраняется, ключ живёт только в памяти процесса. Ключ подписи — Ed25519,
# выводится из фразы Argon2id с собственной солью (это НЕ ключ APK, даже при той же фразе). Открытый ключ лежит в репозитории:
# release/yandi-release-pubkey.hex (закоммитить при первой подписи); при следующих подписях фраза проверяется по нему.
# Номер выпуска (против отката) — release/SEQ, увеличивается на единицу при каждой подписи (закоммитить).
#
# Запуск из корня репозитория:  bash tools/node_release.sh
#   --check  только проверить фразу по открытому ключу, без сборки
#   --show   вводить фразу видимо (проверить раскладку/опечатку; делать только когда никто не смотрит)
set -euo pipefail
cd "$(dirname "$0")/.."
SHOW=0; CHECK=0
for a in "$@"; do case "$a" in --show) SHOW=1;; --check) CHECK=1;; *) echo "неизвестный ключ: $a"; exit 2;; esac; done

PUB=release/yandi-release-pubkey.hex
SEQFILE=release/SEQ
ENV_CLEAN=(env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY)

"${ENV_CLEAN[@]}" cargo build --release --offline --bin yandi --bin yandi-release >/dev/null 2>&1 \
  || { echo "не собрались yandi и yandi-release"; exit 1; }
REL=target/release/yandi-release

echo "Фраза разработчика: не меньше 12 слов и 60 символов. Это фраза подписи ВЫПУСКОВ НОДЫ."
if [ "$SHOW" = 1 ]; then
  read -r -p "Фраза: " P1; read -r -p "Ещё раз: " P2
else
  read -rs -p "Фраза: " P1; echo
  read -rs -p "Ещё раз: " P2; echo
fi
NFC() { python3 -I -c 'import sys, unicodedata; sys.stdout.write("".join(unicodedata.normalize("NFC", l) for l in sys.stdin))'; }
PHRASE_IN=$(printf '%s\n%s\n' "$P1" "$P2" | NFC)
unset P1 P2

PUBKEY=$(printf '%s\n' "$PHRASE_IN" | "$REL" pubkey) || { echo "фраза отвергнута"; exit 1; }
mkdir -p release
if [ ! -f "$PUB" ]; then
  echo "$PUBKEY" > "$PUB"
  echo "Первая подпись: создан $PUB — его нужно закоммитить (он открытый)."
elif [ "$(cat "$PUB")" != "$PUBKEY" ]; then
  echo "Фраза не та: ключ не совпадает с release/yandi-release-pubkey.hex. Остановлено."
  exit 1
else
  echo "Фраза верна: ключ совпадает с закреплённым."
fi
if [ "$CHECK" = 1 ]; then echo "Проверка завершена, сборка не запускалась."; exit 0; fi

VERSION=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')
SEQ=$(( $(cat "$SEQFILE" 2>/dev/null || echo 0) + 1 ))
ARCH=$(uname -m)
NAME="yandi-node-${VERSION}-linux-${ARCH}"
OUT="dist/${NAME}"
rm -rf "$OUT"; mkdir -p "$OUT/pkg"
install -m 0755 target/release/yandi "$OUT/pkg/yandi"
install -m 0755 target/release/yandi-release "$OUT/pkg/yandi-release"
cp LICENSE "$OUT/pkg/" 2>/dev/null || true
cp deploy/yandi-node.service "$OUT/pkg/" 2>/dev/null || true
cp "$PUB" "$OUT/pkg/yandi-release-pubkey.hex"
# детерминированный архив: порядок, владелец, время — фиксированные (воспроизводимый хэш при тех же входах)
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime='UTC 2020-01-01' -C "$OUT/pkg" -cf - . | gzip -n -9 > "$OUT/${NAME}.tar.gz"
rm -rf "$OUT/pkg"

"$REL" manifest "$OUT" "$VERSION" "$SEQ" --pubkey "$PUBKEY" "${NAME}.tar.gz"
printf '%s\n' "$PHRASE_IN" | "$REL" sign "$OUT"
unset PHRASE_IN
"$REL" verify "$OUT" --pubkey "$PUBKEY" --min-seq "$SEQ"
echo "$SEQ" > "$SEQFILE"
echo
echo "Готово: $OUT/"
ls -l "$OUT"
echo "Закоммитить: $SEQFILE${PUB:+ и $PUB (если создан)}."
