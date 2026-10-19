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
printf '%s' "$PUBKEY" | sed 's/../\\x&/g' | xargs -0 printf > "$W/pub.raw"
[ "$(stat -c %s "$W/pub.raw")" = 32 ] || die "ключ должен быть ровно 32 байта"
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
KEY_ID=$(sha256sum "$W/pub.raw" | cut -c1-16)
PARSED=$(python3 -I -B - "$W/MANIFEST.json" "$KEY_ID" <<'PY'
import json, re, sys, time, unicodedata

class Bad(Exception):
    pass

def need(cond, msg):
    if not cond:
        raise Bad(msg)

def is_int(x):
    return type(x) is int

def pairs(items):
    d = {}
    for k, v in items:
        need(k not in d, "повторяющееся поле " + k)
        d[k] = v
    return d

def nofloat(x):
    raise Bad("недопустимое число " + x)

def tron_ok(addr):
    import hashlib
    alpha = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
    if len(addr) != 34 or any(c not in alpha for c in addr):
        return False
    n = 0
    for c in addr:
        n = n * 58 + alpha.index(c)
    if n >= 1 << 200:
        return False
    raw = n.to_bytes(25, "big")
    return raw[0] == 0x41 and hashlib.sha256(hashlib.sha256(raw[:21]).digest()).digest()[:4] == raw[21:]

def keccak256(data):
    RC = [0x1, 0x8082, 0x800000000000808A, 0x8000000080008000, 0x808B, 0x80000001, 0x8000000080008081, 0x8000000000008009, 0x8A, 0x88, 0x80008009, 0x8000000A, 0x8000808B, 0x800000000000008B, 0x8000000000008089, 0x8000000000008003, 0x8000000000008002, 0x8000000000000080, 0x800A, 0x800000008000000A, 0x8000000080008081, 0x8000000000008080, 0x80000001, 0x8000000080008008]
    R = [[0, 36, 3, 41, 18], [1, 44, 10, 45, 2], [62, 6, 43, 15, 61], [28, 55, 25, 21, 56], [27, 20, 39, 8, 14]]
    M = (1 << 64) - 1
    rol = lambda x, n: ((x << n) | (x >> (64 - n))) & M if n else x
    s = [[0] * 5 for _ in range(5)]
    p = bytearray(data) + b"\x01"
    p += b"\x00" * ((-len(p)) % 136)
    p[-1] |= 0x80
    for i in range(0, len(p), 136):
        for j in range(17):
            s[j % 5][j // 5] ^= int.from_bytes(p[i + 8 * j:i + 8 * j + 8], "little")
        for rc in RC:
            C = [s[x][0] ^ s[x][1] ^ s[x][2] ^ s[x][3] ^ s[x][4] for x in range(5)]
            D = [C[(x - 1) % 5] ^ rol(C[(x + 1) % 5], 1) for x in range(5)]
            s = [[s[x][y] ^ D[x] for y in range(5)] for x in range(5)]
            B = [[0] * 5 for _ in range(5)]
            for x in range(5):
                for y in range(5):
                    B[y][(2 * x + 3 * y) % 5] = rol(s[x][y], R[x][y])
            s = [[B[x][y] ^ ((~B[(x + 1) % 5][y]) & B[(x + 2) % 5][y]) for y in range(5)] for x in range(5)]
            s[0][0] ^= rc
    return b"".join(s[j % 5][j // 5].to_bytes(8, "little") for j in range(4))

def evm_ok(addr):
    # EIP-55: регистр букв несёт контрольную сумму; адрес целиком в одном регистре не принимается (как в src/release.rs)
    if not (addr.startswith("0x") and len(addr) == 42 and all(c in "0123456789abcdefABCDEF" for c in addr[2:])):
        return False
    low = addr[2:].lower()
    k = keccak256(low.encode()).hex()
    return addr[2:] == "".join(c.upper() if c.isalpha() and int(k[i], 16) >= 8 else c for i, c in enumerate(low))

def wallet_ok(net, addr):
    tron_net = net.lstrip().lower().startswith("tron")
    return (not tron_net and evm_ok(addr)) if addr.startswith("0x") else (tron_net and tron_ok(addr))

# схема совпадает с src/release.rs: validate_manifest (deny_unknown_fields)
ROOT_REQUIRED = {"format", "product", "version", "seq", "created", "key_id", "files"}
ROOT_ALLOWED = ROOT_REQUIRED | {"platform", "expires", "notes", "support"}
try:
    raw = open(sys.argv[1], "rb").read()
    need(len(raw) <= 1 << 20, "манифест слишком велик")
    m = json.loads(raw.decode("utf-8"), object_pairs_hook=pairs, parse_float=nofloat, parse_constant=nofloat)
    need(type(m) is dict, "корень манифеста — не объект")
    need(ROOT_REQUIRED <= set(m) <= ROOT_ALLOWED, "состав полей манифеста не совпадает со схемой")
    need(is_int(m["format"]) and m["format"] == 1, "формат манифеста не поддерживается")
    need(m["product"] == "yandi-node", "это не выпуск yandi-node")
    need(m.get("platform") == "deb", "это не выпуск для Debian/Ubuntu")
    v = m["version"]
    # как в src/release.rs: до 200 БАЙТ UTF-8, без управляющих знаков Юникода (категория Cc, включая C1)
    need(type(v) is str and 1 <= len(v.encode("utf-8")) <= 200 and not any(unicodedata.category(c) == "Cc" for c in v), "неверная версия")
    seq, created, exp = m["seq"], m["created"], m.get("expires")
    need(is_int(seq) and 1 <= seq < 2**53, "неверный номер выпуска")
    need(is_int(created) and 0 <= created < 2**63, "неверное время выпуска")
    need(is_int(exp) and 0 < exp < 2**63 and exp > time.time(), "срок действия выпуска истёк или не указан")
    need(type(m["key_id"]) is str and re.fullmatch(r"[0-9a-f]{16}", m["key_id"]), "неверный идентификатор ключа")
    need(m["key_id"] == sys.argv[2], "идентификатор ключа в манифесте не совпадает с закреплённым ключом")
    notes = m.get("notes", "")
    need(type(notes) is str and len(notes) <= 2000, "неверные заметки")
    sw = m.get("support", [])
    # кошельки проекта: до 8, те же правила, что в src/release.rs (SupportWallet::valid): монета 1..16, сеть 1..32 знаков без управляющих и «|»,
    # адрес Tron с Base58Check или EVM с EIP-55, дубликатов нет
    need(type(sw) is list and len(sw) <= 8, "неверный список кошельков поддержки")
    seen = set()
    for w in sw:
        need(type(w) is dict and set(w) == {"asset", "network", "address"} and all(type(x) is str for x in w.values()), "неверный кошелёк поддержки")
        need(1 <= len(w["asset"]) <= 16 and 1 <= len(w["network"]) <= 32, "неверная длина монеты или сети кошелька")
        need(not any(unicodedata.category(c) == "Cc" or c == "|" for c in w["asset"] + w["network"]), "недопустимые знаки в монете или сети кошелька")
        need(wallet_ok(w["network"], w["address"]), "адрес кошелька не проходит проверку контрольной суммы")
        key = (w["asset"], w["network"], w["address"])
        need(key not in seen, "кошелёк указан дважды")
        seen.add(key)
    files = m["files"]
    need(type(files) is list and 1 <= len(files) <= 32, "неверный список файлов")
    names = set()
    out = ["SEQ %d" % seq]
    for f in files:
        need(type(f) is dict and set(f) == {"name", "sha256", "size"}, "запись файла должна содержать ровно name, sha256, size")
        n, h, z = f["name"], f["sha256"], f["size"]
        need(type(n) is str and re.fullmatch(r"[A-Za-z0-9._-]{1,128}", n) and not n.startswith("."), "недопустимое имя файла")
        need(type(h) is str and re.fullmatch(r"[0-9a-f]{64}", h), "неверная сумма")
        need(is_int(z) and 1 <= z <= 256 * 1024 * 1024, "неверный размер")
        need(n not in names, "файл указан дважды")
        names.add(n)
        out.append("FILE %s %s %d" % (n, h, z))
    print("\n".join(out))
except Bad as e:
    sys.stderr.write("манифест отвергнут: %s\n" % e)
    sys.exit(1)
except Exception as e:
    sys.stderr.write("манифест отвергнут: %s\n" % e)
    sys.exit(1)
PY
) || die "манифест не прошёл строгую проверку"
SEQ=$(awk '$1=="SEQ"{print $2}' <<<"$PARSED")
# Номер последнего установленного выпуска (защита от отката). Состояние должно быть нашим обычным файлом в нашем закрытом каталоге;
# повреждённое или чужое — отказ, а не «защиты нет». Чтение, проверка и запись идут под одной блокировкой.
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/yandi"; SEQ_FILE="$STATE_DIR/installed-seq"; LOCK_FILE="$STATE_DIR/lock"
ME=$(id -u)
# все каталоги пути обязаны быть нашими (или root) и не открытыми для записи другим (кроме каталогов с sticky-битом, как /tmp)
safe_dir_chain() {
  local d="$1" mode owner
  while [ "$d" != "/" ] && [ -n "$d" ]; do
    if [ -e "$d" ]; then
      [ -d "$d" ] || return 1
      owner=$(stat -c %u "$d"); mode=$(stat -c %a "$d")
      [ "$owner" = "$ME" ] || [ "$owner" = 0 ] || return 1
      if [ $(( 0$mode & 0022 )) -ne 0 ] && [ $(( 0$mode & 01000 )) -eq 0 ]; then return 1; fi
    fi
    d=$(dirname "$d")
  done
  return 0
}
mkdir -p -m 0700 "$STATE_DIR" 2>/dev/null || true
[ -d "$STATE_DIR" ] && [ ! -L "$STATE_DIR" ] && [ "$(stat -c %u "$STATE_DIR")" = "$ME" ] && [ -z "$(find "$STATE_DIR" -maxdepth 0 -perm /077)" ] && safe_dir_chain "$STATE_DIR" \
  || die "каталог состояния $STATE_DIR небезопасен (ссылка, чужой владелец, доступ другим или небезопасный родитель); нужен закрытый каталог 0700"
command -v flock >/dev/null || die "нужен flock (пакет util-linux)"
# файл блокировки: только обычный файл нашего владельца, без ссылок и каналов; открывается без обрезания
if [ -e "$LOCK_FILE" ] || [ -L "$LOCK_FILE" ]; then
  [ -f "$LOCK_FILE" ] && [ ! -L "$LOCK_FILE" ] && [ "$(stat -c %u "$LOCK_FILE")" = "$ME" ] && [ -z "$(find "$LOCK_FILE" -maxdepth 0 -perm /022)" ] || die "файл блокировки $LOCK_FILE небезопасен"
fi
exec 9>>"$LOCK_FILE" || die "не удалось открыть файл блокировки"
flock -x 9
read_seq() {
  [ ! -L "$SEQ_FILE" ] || die "файл $SEQ_FILE — символическая ссылка: небезопасно"
  [ -e "$SEQ_FILE" ] || { echo 0; return; }
  [ -f "$SEQ_FILE" ] && [ "$(stat -c %u "$SEQ_FILE")" = "$ME" ] && [ -r "$SEQ_FILE" ] && [ -z "$(find "$SEQ_FILE" -maxdepth 0 -perm /022)" ] || die "файл $SEQ_FILE небезопасен или нечитаем"
  local v; v=$(cat "$SEQ_FILE") || die "файл $SEQ_FILE нечитаем"
  [[ "$v" =~ ^[0-9]{1,16}$ ]] && [ "$v" -lt 9007199254740992 ] || die "файл $SEQ_FILE повреждён (ожидалось одно число): удалите его, если уверены в выпуске"
  echo "$v"
}
OLD=$(read_seq)
[ "$SEQ" -ge "$OLD" ] || die "выпуск №$SEQ старше уже установленного №$OLD (откат): установка отменена"
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
# номер фиксируется как максимум под той же блокировкой, атомарной заменой файла
CUR=$(read_seq); [ "$SEQ" -ge "$CUR" ] || SEQ="$CUR"
TMP_SEQ=$(mktemp "$STATE_DIR/installed-seq.XXXXXX") || die "пакеты установлены, но номер выпуска не сохранён (не создать временный файл в $STATE_DIR)"
{ printf '%s\n' "$SEQ" > "$TMP_SEQ" && chmod 600 "$TMP_SEQ" && mv -f "$TMP_SEQ" "$SEQ_FILE"; } || { rm -f "$TMP_SEQ"; die "пакеты установлены, но номер выпуска не сохранён в $SEQ_FILE: защита от отката не обновлена"; }
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
  case "$a" in n|N) ;; *) exec 9>&-; exec yandi setup;; esac
fi
