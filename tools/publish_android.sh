#!/usr/bin/env bash
# Подписанный выпуск Android-приложения: ОДНА папка update/android/ на yandi.su служит и для ручного скачивания, и для проверки обновлений
# в приложении. В ней три файла: yandi.apk (имя постоянное), MANIFEST.json и MANIFEST.json.sig (платформа android, SHA-256 файла, номер
# выпуска, срок действия). Подпись — тем же ключом выпусков, что у пакетов узла (release/yandi-release-pubkey.hex); сам APK подписан
# отдельным ключом APK (tools/apk_release.sh).
#
# Порядок: bash tools/apk_release.sh  →  bash tools/publish_android.sh [путь к APK]
#   результат: site/update/android/ (корень выпуска) и плоский архив в archive/android_<версия>/.
# Фраза выпусков — две одинаковые строки на stdin (скрытый ввод); нигде не сохраняется.
#   YANDI_RELEASE_NOTES="что нового"  YANDI_RELEASE_EXPIRES_DAYS=120
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/paths.sh
APK=${1:-mobile/yandi_mobile/build/app/outputs/flutter-apk/app-release.apk}
[ -f "$APK" ] || { echo "нет $APK: сначала bash tools/apk_release.sh"; exit 1; }
PUB=release/yandi-release-pubkey.hex
[ -f "$PUB" ] || { echo "нет $PUB"; exit 1; }
REL=target/release/yandi-release
[ -x "$REL" ] || env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY cargo build --release --offline --bin yandi-release
[ ! -L release/.lock ] || { echo 'Недопустимый lock'; exit 1; }
exec 9>release/.lock
flock -x 9

# версия — из pubspec (0.2.0+5 → 0.2.0-b5); приложение и сайт показывают её же
RAW=$(sed -n 's/^version: *//p' mobile/yandi_mobile/pubspec.yaml | head -1)
VERSION=${RAW/+/-b}
[ -n "$VERSION" ] || { echo "не найдена версия в pubspec.yaml"; exit 1; }

read -rs -p "Фраза: " P1; echo
read -rs -p "Ещё раз: " P2; echo
PHRASE_IN=$(printf '%s\n%s\n' "$P1" "$P2")
unset P1 P2
KEY=$(printf '%s\n' "$PHRASE_IN" | "$REL" pubkey) || { echo "фраза отвергнута"; exit 1; }
[ "$KEY" = "$(cat "$PUB")" ] || { echo "Фраза не та: ключ не совпадает с $PUB. Остановлено."; exit 1; }

SEQ=$(python3 -I tools/release_seq.py release)
SITE="$SITE_DIR/update/android"
rm -rf "$SITE"
mkdir -p "$SITE"
cp "$APK" "$SITE/yandi.apk"
YANDI_RELEASE_PLATFORM=android "$REL" manifest "$SITE" "$VERSION" "$SEQ" --pubkey "$KEY" yandi.apk
printf '%s\n' "$PHRASE_IN" | "$REL" sign "$SITE"
unset PHRASE_IN
"$REL" verify "$SITE" --pubkey "$KEY" --min-seq "$SEQ"

OUT="$ARCHIVE_DIR/android_${VERSION}"
mkdir -p "$OUT"
ZIP="$OUT/yandi-su_update-android.zip"
rm -f "$ZIP"
(cd "$SITE" && zip -q -X "$ZIP" MANIFEST.json MANIFEST.json.sig yandi.apk)
echo "готово: версия ${VERSION}, номер ${SEQ}"
echo "Три файла уже лежат в site/update/android/; архив для заливки: $ZIP"
echo "Страница скачивания: ссылка на https://yandi.su/update/android/yandi.apk (тот же файл, второй копии нет)."
