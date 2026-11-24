#!/usr/bin/env bash
# Полный выпуск одной командой: узел + оболочка ПК (подписанные .deb и манифест) и подписанный APK, все ключи — из ОДНОЙ фразы разработчика.
#
# Фразу вводит сам разработчик (скрыто, дважды); она нигде не сохраняется и не печатается, живёт только в памяти этого скрипта и передаётся
# дочерним скриптам через стандартный ввод. Ключи выводятся из неё (отдельные соли: ключ выпусков узла/APK-манифестов и ключ подписи APK —
# разные ключи).
#
#   bash tools/release_all.sh --new-keys   ПЕРВЫЙ выпуск с НОВОЙ фразой: пишет новый открытый ключ выпусков (release/yandi-release-pubkey.hex),
#                                          подставляет его в tools/install.sh, убирает прежний сертификат APK (создастся новый), затем строит всё.
#   bash tools/release_all.sh              следующие выпуски: фраза обязана совпасть с закреплённым ключом.
#   bash tools/release_all.sh --plan       только проверить фразу и показать, что будет сделано (ничего не меняет и не строит).
#   YANDI_RELEASE_NOTES="что нового"  YANDI_RELEASE_EXPIRES_DAYS=120   — описание и срок манифестов (по умолчанию 120 дней).
#
# После выпуска закоммитить публичные файлы, которые скрипт назовёт (открытый ключ, сертификат APK, release/SEQ).
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/paths.sh
NEW=0; PLAN=0
for a in "$@"; do case "$a" in --new-keys) NEW=1;; --plan) PLAN=1;; *) echo "неизвестный ключ: $a"; exit 2;; esac; done
PUB=release/yandi-release-pubkey.hex
CERT=mobile/yandi_mobile/android/signing/yandi-release-cert.pem
ENV_CLEAN=(env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY)

# До сборки: список входных узлов подписан тем ключом, который вшивается в узел и приложение, и вшитая копия в приложении та же.
python3 - <<'PY' || { echo "Выпуск остановлен: подпись списка не готова (см. выше). Подпишите: yandi admin bootstrap sign bootstrap/bootstrap.json"; exit 1; }
import json,re,sys
L=json.load(open("bootstrap/bootstrap.json")); pub=open("bootstrap/signer.pub").read().strip().lower()
dart=open("mobile/yandi_mobile/lib/models/bootstrap_list.dart",encoding="utf-8").read()
bad=[]
if not L.get("signature"): bad.append("в bootstrap/bootstrap.json нет подписи")
if L.get("signer","").lower()!=pub: bad.append("signer списка %s… не равен bootstrap/signer.pub %s…"%(L.get("signer","")[:8],pub[:8]))
if pub not in dart.lower(): bad.append("ключ из signer.pub не закреплён в bootstrap_list.dart")
if pub not in open("desktop/yandi_desktop/lib/guest/guest_bootstrap.dart",encoding="utf-8").read().lower(): bad.append("ключ из signer.pub не закреплён в desktop/yandi_desktop/lib/guest/guest_bootstrap.dart")
if open("mobile/yandi_mobile/assets/bootstrap.json",encoding="utf-8").read()!=open("bootstrap/bootstrap.json",encoding="utf-8").read(): bad.append("mobile/yandi_mobile/assets/bootstrap.json не совпадает с bootstrap/bootstrap.json")
if open("desktop/yandi_desktop/assets/bootstrap.json",encoding="utf-8").read()!=open("bootstrap/bootstrap.json",encoding="utf-8").read(): bad.append("desktop/yandi_desktop/assets/bootstrap.json не совпадает с bootstrap/bootstrap.json")
for b in bad: print("ОШИБКА:",b)
sys.exit(1 if bad else 0)
PY

REL=target/release/yandi-release
"${ENV_CLEAN[@]}" cargo build --release --offline --bin yandi-release >/dev/null 2>&1 || { echo "не собрался инструмент подписи"; exit 1; }

echo "Фраза разработчика: не меньше 12 слов и 60 символов, регистр важен. Ввод не отображается."
read -rs -p "Фраза: " P1; echo
read -rs -p "Ещё раз: " P2; echo
[ "$P1" = "$P2" ] || { echo "Фразы не совпали."; exit 1; }
PHRASE_IN=$(printf '%s\n%s\n' "$P1" "$P2")
unset P1 P2
feed() { printf '%s\n' "$PHRASE_IN"; printf '%s\n' "$PHRASE_IN"; }   # две строки на каждый дочерний скрипт (по одной на «Фраза» и «Ещё раз»)

KEY=$(printf '%s\n' "$PHRASE_IN" | "$REL" pubkey) || { echo "фраза отвергнута (слишком слабая или не совпала)"; exit 1; }
if [ "$NEW" = 0 ]; then
  [ -f "$PUB" ] && [ "$KEY" = "$(cat "$PUB")" ] || { echo "Фраза не та: ключ не совпадает с $PUB. Для НОВОЙ фразы запустите с --new-keys."; exit 1; }
  echo "Фраза верна: ключ совпадает с закреплённым ${KEY:0:16}…"
else
  echo "Новый открытый ключ выпусков: ${KEY}"
  OLD=$(cat "$PUB" 2>/dev/null || true)
  if [ "$PLAN" = 1 ]; then
    echo "[план] будут изменены: $PUB, tools/install.sh (PUBKEY_DEFAULT), удалён сертификат APK $CERT (создастся новый)."
  else
    printf '%s\n' "$KEY" > "$PUB"
    if [ -n "$OLD" ]; then sed -i "s/$OLD/$KEY/" tools/install.sh; fi
    grep -q "$KEY" tools/install.sh || { echo "в tools/install.sh не нашёлся прежний ключ ($OLD): обновите PUBKEY_DEFAULT вручную на $KEY"; }
    rm -f "$CERT"
    echo "Записано: $PUB, tools/install.sh; прежний сертификат APK убран."
  fi
fi
if [ "$PLAN" = 1 ]; then
  echo "[план] затем: узел и оболочка — tools/publish_release.sh --build (сборка, .deb, манифест, подпись); APK — tools/apk_release.sh и tools/publish_android.sh."
  echo "Проверка фразы завершена, ничего не изменено."
  exit 0
fi

echo "=== Узел и оболочка ПК ==="
feed | bash tools/publish_release.sh --build
echo "=== APK ==="
feed | bash tools/apk_release.sh
feed | bash tools/publish_android.sh
unset PHRASE_IN KEY

echo
echo "Готово. Закоммитить публичные файлы: release/SEQ, $PUB, tools/install.sh$( [ -f "$CERT" ] && echo ", $CERT" )."
bash tools/pack_site.sh
echo "Архив всего сайта — в ~/Загрузки (yandi-site_*.zip); зеркало: $SITE_DIR. Для GitHub: $GH_DIR/."
