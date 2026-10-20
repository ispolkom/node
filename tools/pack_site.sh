#!/usr/bin/env bash
# Собрать всё, что заливается на yandi.su/update/: список входных узлов + подписанные пакеты, и положить один архив в archive/.
#   bash tools/pack_site.sh
# Сначала проверяет подписанный список входных узлов (bootstrap/bootstrap.json), потом кладёт его в site/update/bootstrap.json.
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/paths.sh
BIN=target/release/yandi
[ -x "$BIN" ] || { echo "нет $BIN: cargo build --release --offline --bins"; exit 1; }
"$BIN" bootstrap verify bootstrap/bootstrap.json
mkdir -p "$SITE_DIR/update" "$ARCHIVE_DIR"
cp bootstrap/bootstrap.json "$SITE_DIR/update/bootstrap.json"
# открытые ключи и сертификаты — в одну папку-витрину (ссылки на файлы репозитория, второй копии нет)
mkdir -p "$PUBLIC_KEYS_DIR"
ln -sf ../src/release/yandi-release-pubkey.hex "$PUBLIC_KEYS_DIR/yandi-release-pubkey.hex"
ln -sf ../src/bootstrap/signer.pub "$PUBLIC_KEYS_DIR/bootstrap-signer.pub"
[ -f mobile/yandi_mobile/android/signing/yandi-release-cert.pem ] && ln -sf ../src/mobile/yandi_mobile/android/signing/yandi-release-cert.pem "$PUBLIC_KEYS_DIR/yandi-release-cert.pem"
Z="$ARCHIVE_DIR/yandi-su_update_$(date +%Y%m%d-%H%M).zip"
(cd "$SITE_DIR" && zip -q -r -X "$Z" update)
echo "готово: $Z"; unzip -l "$Z" | tail -12
