#!/usr/bin/env bash
# Установка YANDI на Debian/Ubuntu из подписанного выпуска (MANIFEST.json + подпись Ed25519), без «curl | sh» вслепую:
# сначала проверяется подпись и сумма каждого файла, и только потом ставится пакет.
#
#   bash install.sh                  спросит, что ставить (в терминале); без терминала — только узел (сервер)
#   bash install.sh --server         только узел (yandi-node + служба), оболочка не ставится
#   bash install.sh --desktop        узел + оболочка ПК (yandi-desktop)
#   параметры:  --from КАТАЛОГ_ИЛИ_URL   откуда брать (по умолчанию https://yandi.su/update/deb/)
#               --pubkey HEX64           ключ подписи выпуска (по умолчанию тот, что вшит в этот скрипт; сверьте его отпечаток)
#               --verify-only            скачать и проверить, ничего не ставить
#               --no-service             не создавать службу systemd
#               --yes                    не задавать вопросов (первую настройку не запускать)
# Нужны: bash, curl (для URL), openssl 3, sha256sum, sudo. Фраза восстановления и пароли вводятся только в `yandi setup`.
set -euo pipefail

PUBKEY_DEFAULT="427e076fe0d91d06157cc2cf342310a2df9ee0ca8a0112e384798e080251395f"
FROM="https://yandi.su/update/deb/"
MODE=""; VERIFY_ONLY=0; NO_SERVICE=0; YES=0; PUBKEY="$PUBKEY_DEFAULT"

while [ $# -gt 0 ]; do
  case "$1" in
    --server) MODE=server;; --desktop) MODE=desktop;;
    --from) FROM="${2:?--from требует значение}"; shift;;
    --pubkey) PUBKEY="${2:?--pubkey требует значение}"; shift;;
    --verify-only) VERIFY_ONLY=1;; --no-service) NO_SERVICE=1;; --yes) YES=1;;
    -h|--help) sed -n 2,14p "$0"; exit 0;;
    *) echo "неизвестный параметр: $1" >&2; exit 2;;
  esac; shift
done

die() { echo "ОШИБКА: $*" >&2; exit 1; }
if [ "$VERIFY_ONLY" = 0 ]; then
  [ "$(id -u)" != 0 ] || die "запустите скрипт от обычного пользователя (данные узла лежат в его домашней папке); sudo он вызовет сам"
  command -v sudo >/dev/null || die "нужен sudo"
  if [ "$NO_SERVICE" = 0 ]; then
    U=$(id -un); G=$(id -gn); H=$(getent passwd "$U" | cut -d: -f6)
    # значения попадают в unit-файл systemd (там пробелы разделяют аргументы, а % — подстановка): допускаем только безопасные знаки
    [[ "$U" =~ ^[A-Za-z0-9._-]+$ && "$G" =~ ^[A-Za-z0-9._-]+$ ]] || die "имя пользователя или группы содержит знаки, которые нельзя безопасно записать в службу systemd; создайте службу вручную (--no-service)"
    [[ "$H" =~ ^/[A-Za-z0-9._/-]+$ ]] || die "домашняя папка «$H» содержит пробелы или %, ... — службу так не создать; используйте --no-service и опишите службу вручную"
  fi
fi
[[ "$PUBKEY" =~ ^[0-9a-f]{64}$ ]] || die "ключ — 64 шестнадцатеричных знака"
for t in openssl sha256sum awk; do command -v "$t" >/dev/null || die "нужна программа $t"; done
openssl version | grep -q "OpenSSL 3" || die "нужен OpenSSL 3 (проверка Ed25519)"
case "$FROM" in http://*) die "только https:// или каталог";; https://*) command -v curl >/dev/null || die "нужен curl"; FROM="${FROM%/}/";; *) [ -d "$FROM" ] || die "нет каталога $FROM"; FROM="${FROM%/}/";; esac

if [ -z "$MODE" ]; then
  if [ "$YES" = 0 ] && [ -t 0 ] && [ -t 1 ]; then
    echo "Что установить?"
    echo "  1) Узел для сервера (без оболочки) — управление из консоли: yandi setup, yandi admin"
    echo "  2) Узел и оболочка ПК (окно программы)"
    read -r -p "Выбор [1/2]: " c
    case "$c" in 2) MODE=desktop;; *) MODE=server;; esac
  else MODE=server; fi
fi
echo "Режим: $MODE"

W=$(mktemp -d); chmod 700 "$W"; trap 'rm -rf "$W"' EXIT
fetch() { # имя -> файл в $W
  case "$FROM" in https://*) curl --fail --silent --show-error --proto '=https' --tlsv1.2 --max-redirs 0 --max-time 600 -o "$W/$1" "${FROM}$1";; *) cp -- "${FROM}$1" "$W/$1";; esac
}
fetch MANIFEST.json; fetch MANIFEST.json.sig
[ "$(stat -c %s "$W/MANIFEST.json")" -le 1048576 ] || die "манифест слишком велик"

# --- подпись: Ed25519 над «YANDI-RELEASE-MANIFEST-V1\0» + байты манифеста ---
printf '\x30\x2a\x30\x05\x06\x03\x2b\x65\x70\x03\x21\x00' > "$W/pub.der"
echo "$PUBKEY" | sed 's/../\\x&/g' | xargs -0 printf > "$W/pub.raw"
cat "$W/pub.raw" >> "$W/pub.der"
openssl pkey -pubin -inform DER -in "$W/pub.der" -out "$W/pub.pem" 2>/dev/null || die "ключ не читается"
tr -d ' \n\r' < "$W/MANIFEST.json.sig" | grep -Eq '^[0-9a-f]{128}$' || die "подпись должна быть 128 шестнадцатеричных знаков"
tr -d ' \n\r' < "$W/MANIFEST.json.sig" | sed 's/../\\x&/g' | xargs -0 printf > "$W/sig.bin"
{ printf 'YANDI-RELEASE-MANIFEST-V1\0'; cat "$W/MANIFEST.json"; } > "$W/signed.bin"
openssl pkeyutl -verify -rawin -pubin -inkey "$W/pub.pem" -in "$W/signed.bin" -sigfile "$W/sig.bin" >/dev/null 2>&1 \
  || die "ПОДПИСЬ ВЫПУСКА НЕ СОВПАДАЕТ — не устанавливайте этот выпуск"
echo "Подпись выпуска верна (ключ ${PUBKEY:0:16}…). Сверьте этот отпечаток с сайтом yandi.su/network/downloads."

# --- содержимое манифеста: строгий разбор JSON (данные уже подписаны, но формат проверяется целиком) ---
command -v python3 >/dev/null || die "нужен python3 (разбор манифеста): sudo apt install python3-minimal"
ARCH=$(dpkg --print-architecture 2>/dev/null || echo amd64)
PARSED=$(python3 - "$W/MANIFEST.json" <<'PY'
import json, re, sys, time
def pairs(items):
    d = {}
    for k, v in items:
        if k in d: raise ValueError("повторяющееся поле " + k)
        d[k] = v
    return d
try:
    m = json.loads(open(sys.argv[1], "rb").read().decode("utf-8"), object_pairs_hook=pairs)
    assert isinstance(m, dict)
    assert m.get("format") == 1, "формат манифеста не поддерживается"
    assert m.get("product") == "yandi-node", "это не выпуск yandi-node"
    assert m.get("platform") == "deb", "это не выпуск для Debian/Ubuntu"
    exp, seq = m.get("expires"), m.get("seq")
    assert isinstance(exp, int) and not isinstance(exp, bool) and exp > time.time(), "срок действия выпуска истёк или не указан"
    assert isinstance(seq, int) and not isinstance(seq, bool) and 0 <= seq < 2**53, "неверный номер выпуска"
    files = m.get("files")
    assert isinstance(files, list) and 1 <= len(files) <= 32, "в манифесте неверный список файлов"
    names = set()
    out = ["SEQ %d" % seq]
    for f in files:
        assert isinstance(f, dict) and set(f) == {"name", "sha256", "size"}, "запись файла должна содержать ровно name, sha256, size"
        n, h, z = f["name"], f["sha256"], f["size"]
        assert isinstance(n, str) and re.fullmatch(r"[A-Za-z0-9._-]{1,128}", n) and not n.startswith("."), "недопустимое имя файла"
        assert isinstance(h, str) and re.fullmatch(r"[0-9a-f]{64}", h), "неверная сумма"
        assert isinstance(z, int) and not isinstance(z, bool) and 1 <= z <= 256 * 1024 * 1024, "неверный размер"
        assert n not in names, "файл указан дважды"
        names.add(n)
        out.append("FILE %s %s %d" % (n, h, z))
    print("\n".join(out))
except Exception as e:
    sys.stderr.write("манифест отвергнут: %s\n" % e)
    sys.exit(1)
PY
) || die "манифест не прошёл строгую проверку"
SEQ=$(awk '$1=="SEQ"{print $2}' <<<"$PARSED")
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/yandi"; SEQ_FILE="$STATE_DIR/installed-seq"
if [ -r "$SEQ_FILE" ]; then
  OLD=$(tr -dc '0-9' < "$SEQ_FILE")
  [ -z "$OLD" ] || [ "$SEQ" -ge "$OLD" ] || die "выпуск №$SEQ старше уже установленного №$OLD (откат): установка отменена"
fi
mapfile -t ROWS < <(awk '$1=="FILE"{print $2" "$3" "$4}' <<<"$PARSED")
want=("yandi-node_")
[ "$MODE" = desktop ] && want+=("yandi-desktop_")
DEBS=()
for prefix in "${want[@]}"; do
  found=""
  for row in "${ROWS[@]}"; do
    read -r name sha size <<<"$row"
    [[ "$name" == ${prefix}*_${ARCH}.deb ]] || continue
    fetch "$name"
    [ "$(stat -c %s "$W/$name")" = "$size" ] || die "$name: размер не совпадает с манифестом"
    [ "$(sha256sum "$W/$name" | cut -d' ' -f1)" = "$sha" ] || die "$name: СУММА НЕ СОВПАДАЕТ — файл подменён или повреждён"
    echo "Файл проверен: $name"; found="$W/$name"; DEBS+=("$found")
  done
  [ -n "$found" ] || die "в выпуске нет пакета ${prefix}…_${ARCH}.deb"
done
[ "$VERIFY_ONLY" = 1 ] && { echo "Проверка выпуска завершена (--verify-only): ничего не установлено."; exit 0; }

# --- установка ---
sudo apt-get install -y "${DEBS[@]}"
mkdir -p "$STATE_DIR" && printf '%s\n' "$SEQ" > "$SEQ_FILE"
if [ "$NO_SERVICE" = 0 ]; then
  UNIT=$(mktemp); trap 'rm -rf "$W" "$UNIT"' EXIT
  cat > "$UNIT" <<UNITEOF
[Unit]
Description=YANDI node
After=network-online.target
Wants=network-online.target
StartLimitIntervalSec=300
StartLimitBurst=10

[Service]
Type=simple
User=$U
Group=$G
WorkingDirectory=-$H/.local/share/yandi
ExecStartPre=/usr/bin/mkdir -p -m 0700 $H/.local/share/yandi
Environment=YANDI_MOBILE_TLS_PORT=8444
ExecStart=/usr/bin/yandi
StandardInput=null
StandardOutput=journal
StandardError=journal
SyslogIdentifier=yandi-node
Restart=on-failure
RestartSec=5
KillSignal=SIGTERM
TimeoutStopSec=30
LimitNOFILE=65536
NoNewPrivileges=yes
PrivateTmp=yes
ProtectSystem=full
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectControlGroups=yes
RestrictSUIDSGID=yes
LockPersonality=yes
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6 AF_NETLINK

[Install]
WantedBy=multi-user.target
UNITEOF
  sudo install -m 0644 "$UNIT" /etc/systemd/system/yandi-node.service
  sudo systemctl daemon-reload
  sudo systemctl enable --now yandi-node
  SOCK="$H/.local/share/yandi/run/yandi-ui.sock"
  for _ in $(seq 1 60); do [ -S "$SOCK" ] && break; sleep 1; done
  [ -S "$SOCK" ] || echo "Узел ещё запускается: смотрите journalctl -u yandi-node"
fi
echo
echo "Установлено. Дальше:"
echo "  yandi setup        первая настройка (пароль и фраза восстановления)"
echo "  yandi admin        управление узлом; yandi admin pairing — код сопряжения телефона"
echo "  Порты: откройте в брандмауэре порты узла (см. /usr/share/doc/yandi-node/DEPLOYMENT.md)."
if [ "$YES" = 0 ] && [ -t 0 ] && [ -t 1 ] && command -v yandi >/dev/null; then
  read -r -p "Запустить первую настройку сейчас? [Y/n]: " a
  case "$a" in n|N) ;; *) exec yandi setup;; esac
fi
