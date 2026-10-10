#!/usr/bin/env bash
# Release-сборка YANDI, подписанная ключом из фразы разработчика.
#
# Фразу вводит сам разработчик (скрыто, дважды); она не сохраняется. Ключ выводится Argon2id (src/bin/apk_signing_key.rs) в /dev/shm
# (память, не диск), собирается PKCS12 для Gradle, приложение собирается и подписывается, после чего файлы ключа стираются.
# Сертификат (открытая часть) лежит в репозитории: mobile/yandi_mobile/android/signing/yandi-release-cert.pem. Если его нет — это первая
# подпись, он создаётся (его надо закоммитить). Если есть — фраза проверяется по нему: другая фраза = другой ключ, сборка остановится.
#
# Запуск из корня репозитория:  bash tools/apk_release.sh
set -euo pipefail
cd "$(dirname "$0")/.."

CERT=${YANDI_CERT:-mobile/yandi_mobile/android/signing/yandi-release-cert.pem}  # YANDI_CERT — только для пробного прогона
OUT_APK=mobile/yandi_mobile/build/app/outputs/flutter-apk/app-release.apk
export JAVA_HOME=/home/iam/.local/jdk/jdk-17.0.20.1+1
export ANDROID_HOME=/home/iam/.local/android-sdk ANDROID_SDK_ROOT=/home/iam/.local/android-sdk
export PATH="$JAVA_HOME/bin:$PATH"
FLUTTER=/home/iam/.local/flutter-sdk/bin/flutter
APKSIGNER=$(ls -d "$ANDROID_HOME"/build-tools/*/apksigner | sort -V | tail -1)

env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY cargo build --release --offline --bin apk_signing_key >/dev/null 2>&1 \
  || { echo "не собрался инструмент вывода ключа"; exit 1; }

W=$(mktemp -d /dev/shm/yandi-sign.XXXXXX)
chmod 700 "$W"
cleanup() { find "$W" -type f -exec shred -u {} + 2>/dev/null || true; rm -rf "$W"; }
trap cleanup EXIT INT TERM

echo "Фраза разработчика: не меньше 12 случайных слов, любой язык и символы, регистр важен (Вода ≠ вода, ё ≠ е). Ввод не отображается."
read -rs -p "Фраза: " P1; echo
read -rs -p "Ещё раз: " P2; echo
# NFC: одна и та же буква (й, ё, буквы с ударением, эмодзи с модификаторами) бывает записана разными байтами — на другой клавиатуре или
# в другом терминале фраза дала бы другой ключ. Нормализуем стандартной библиотекой Python, ключ выводит apk_signing_key.
printf '%s\n%s\n' "$P1" "$P2" \
  | python3 -I -c 'import sys, unicodedata; sys.stdout.write("".join(unicodedata.normalize("NFC", l) for l in sys.stdin))' \
  | target/release/apk_signing_key "$W/key.der"
unset P1 P2

openssl ec -inform DER -in "$W/key.der" -out "$W/key.pem" 2>/dev/null
chmod 600 "$W/key.pem"

if [ ! -f "$CERT" ]; then
  mkdir -p "$(dirname "$CERT")"
  openssl req -new -x509 -key "$W/key.pem" -subj "/CN=YANDI/O=YANDI" -days 36500 -sha256 -out "$CERT"
  echo "Первая подпись: создан сертификат $CERT — его нужно закоммитить (он открытый)."
else
  if [ "$(openssl x509 -in "$CERT" -pubkey -noout)" != "$(openssl pkey -in "$W/key.pem" -pubout)" ]; then
    echo "Фраза не та: ключ не совпадает с сертификатом YANDI. Сборка остановлена."
    exit 1
  fi
  echo "Фраза верна: ключ совпадает с сертификатом YANDI."
fi

PASS=$(openssl rand -hex 24)
openssl pkcs12 -export -inkey "$W/key.pem" -in "$CERT" -name yandi -out "$W/yandi.p12" -passout "pass:$PASS"

export YANDI_RELEASE_STORE_FILE="$W/yandi.p12" YANDI_RELEASE_STORE_PASSWORD="$PASS"
export YANDI_RELEASE_KEY_ALIAS=yandi YANDI_RELEASE_KEY_PASSWORD="$PASS"
(cd mobile/yandi_mobile && env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY "$FLUTTER" build apk --release)

echo
"$APKSIGNER" verify --print-certs "$OUT_APK" | grep -E "SHA-256|DN" | head -3
echo "Готово: $OUT_APK"
sha1sum "$OUT_APK"
