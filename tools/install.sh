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
#               --lang ru|en             язык сообщений (без ключа спрашивается в терминале; выбор запоминается)
# Подтверждение установки — строго по раскладке выбранного языка: по-русски Д/н (кириллица), по-английски Y/n (латиница).
# Нужны: bash, curl (для URL), openssl 3, sha256sum, sudo. Фраза восстановления и пароли вводятся только в `yandi setup`.
set -euo pipefail

PUBKEY_DEFAULT="23a55aa8e0762e6ca9447dfa92ca401be2ba274c3571b7c3fc67bcb3c7abf43b"
FROM="https://yandi.su/update/deb/"
MODE=""; VERIFY_ONLY=0; NO_SERVICE=0; YES=0; PUBKEY="$PUBKEY_DEFAULT"; LANG_ARG=""

while [ $# -gt 0 ]; do
  case "$1" in
    --server) MODE=server;; --desktop) MODE=desktop;;
    --from) FROM="${2:?--from: нужно значение / needs a value}"; shift;;
    --pubkey) PUBKEY="${2:?--pubkey: нужно значение / needs a value}"; shift;;
    --lang) LANG_ARG="${2:?--lang ru|en}"; shift;;
    --verify-only) VERIFY_ONLY=1;; --no-service) NO_SERVICE=1;; --yes) YES=1;;
    -h|--help) sed -n 2,14p "$0"; exit 0;;
    *) echo "неизвестный параметр / unknown option: $1" >&2; exit 2;;
  esac; shift
done

# Язык: ключ --lang, YANDI_LANG, сохранённый выбор (~/.config/yandi/lang, его читает и сама программа), язык системы. В терминале без явного
# выбора спрашивает «Выберите язык / Select language» и запоминает ответ.
case "$LANG_ARG" in ""|ru|en) ;; *) echo "--lang: ru | en" >&2; exit 2;; esac
LANG_CFG="${XDG_CONFIG_HOME:-$HOME/.config}/yandi/lang"
LANG_SEL="$LANG_ARG"
[ -n "$LANG_SEL" ] || case "${YANDI_LANG:-}" in ru|en) LANG_SEL="${YANDI_LANG}";; esac
LANG_EXPLICIT=0; [ -z "$LANG_SEL" ] || LANG_EXPLICIT=1
if [ -z "$LANG_SEL" ] && [ -r "$LANG_CFG" ] && [ ! -L "$LANG_CFG" ]; then
  case "$(head -c 8 "$LANG_CFG" | tr -d ' \n\r')" in ru) LANG_SEL=ru;; en) LANG_SEL=en;; esac
fi
if [ -z "$LANG_SEL" ]; then
  case "${LC_ALL:-${LC_MESSAGES:-${LANG:-}}}" in ru*|RU*) LANG_SEL=ru;; *) LANG_SEL=en;; esac
fi
LANG_SAVE=0; [ -z "$LANG_ARG" ] || LANG_SAVE=1
if [ "$LANG_EXPLICIT" = 0 ] && [ "$YES" = 0 ] && [ -t 0 ] && [ -t 1 ]; then
  LANG_SAVE=1
  for _ in 1 2 3; do
    read -r -p "Выберите язык / Select language [ru/en] ($LANG_SEL): " c || break
    case "$c" in "") break;; ru|RU|Ru|рус|русский) LANG_SEL=ru; break;; en|EN|En|eng|english) LANG_SEL=en; break;; *) echo "ru | en";; esac
  done
fi
if [ "$LANG_SAVE" = 1 ]; then
  mkdir -p -m 0700 "$(dirname "$LANG_CFG")" 2>/dev/null && [ ! -L "$LANG_CFG" ] && printf '%s\n' "$LANG_SEL" > "$LANG_CFG" 2>/dev/null || true
fi
T() { if [ "$LANG_SEL" = ru ]; then printf '%s' "$1"; else printf '%s' "$2"; fi; }
die() { echo "$(T 'ОШИБКА' 'ERROR'): $*" >&2; exit 1; }
# Вопрос «да/нет» строго по раскладке выбранного языка: русский принимает только кириллицу (Д/н), английский только латиницу (Y/n).
# Чужая раскладка не угадывается (латинская Y на русской раскладке печатает «н»): вопрос повторяется. Возврат: 0 — да, 1 — нет, 2 — нет внятного ответа.
ask_yn() { # ask_yn "вопрос ru" "question en" y|n
  local q hint a
  if [ "$LANG_SEL" = ru ]; then q="$1"; [ "$3" = y ] && hint="[Д/н]" || hint="[д/Н]"; else q="$2"; [ "$3" = y ] && hint="[Y/n]" || hint="[y/N]"; fi
  for _ in 1 2 3 4 5; do
    read -r -p "$q $hint: " a || return 2
    if [ -z "$a" ]; then [ "$3" = y ] && return 0 || return 1; fi
    if [ "$LANG_SEL" = ru ]; then
      case "$a" in д|Д|да|Да|дА|ДА) return 0;; н|Н|нет|Нет|нЕт|НЕТ) return 1;; esac
      echo "Не понял ответ. Введите Д или Н (кириллицей), переключите раскладку на русскую."
    else
      case "$a" in y|Y|yes|Yes|YES|yEs|yeS) return 0;; n|N|no|No|NO) return 1;; esac
      echo "Did not understand. Type Y or N (Latin letters); switch the keyboard layout to English."
    fi
  done
  return 2
}
if [ "$VERIFY_ONLY" = 0 ]; then
  [ "$(id -u)" != 0 ] || die "$(T "запустите скрипт от обычного пользователя (данные узла лежат в его домашней папке); sudo он вызовет сам" "run the script as a regular user (the node data lives in their home folder); it calls sudo itself")"
  command -v sudo >/dev/null || die "$(T "нужен sudo" "sudo is required")"
  if [ "$NO_SERVICE" = 0 ]; then
    U=$(id -un); G=$(id -gn); H=$(getent passwd "$U" | cut -d: -f6)
    # значения попадают в unit-файл systemd (там пробелы разделяют аргументы, а % — подстановка): допускаем только безопасные знаки
    [[ "$U" =~ ^[A-Za-z0-9._-]+$ && "$G" =~ ^[A-Za-z0-9._-]+$ ]] || die "$(T "имя пользователя или группы содержит знаки, которые нельзя безопасно записать в службу systemd; создайте службу вручную (--no-service)" "the user or group name contains characters that cannot be written safely into a systemd service; create the service by hand (--no-service)")"
    [[ "$H" =~ ^/[A-Za-z0-9._/-]+$ ]] || die "$(T "домашняя папка «$H» содержит пробелы или %, ... — службу так не создать; используйте --no-service и опишите службу вручную" "the home folder “$H” contains spaces or %, ... — the service cannot be created like that; use --no-service and describe the service by hand")"
  fi
fi
[[ "$PUBKEY" =~ ^[0-9a-f]{64}$ ]] || die "$(T "ключ — 64 шестнадцатеричных знака" "the key is 64 hexadecimal characters")"
for t in openssl sha256sum awk; do command -v "$t" >/dev/null || die "$(T "нужна программа $t" "the program $t is required")"; done
openssl version | grep -q "OpenSSL 3" || die "$(T "нужен OpenSSL 3 (проверка Ed25519)" "OpenSSL 3 is required (Ed25519 verification)")"
case "$FROM" in http://*) die "$(T "только https:// или каталог" "only https:// or a directory")";; https://*) command -v curl >/dev/null || die "$(T "нужен curl" "curl is required")"; FROM="${FROM%/}/";; *) [ -d "$FROM" ] || die "$(T "нет каталога $FROM" "no such directory: $FROM")"; FROM="${FROM%/}/";; esac

if [ -z "$MODE" ]; then
  if [ "$YES" = 0 ] && [ -t 0 ] && [ -t 1 ]; then
    echo "$(T "Что установить?" "What to install?")"
    echo "$(T "  1) Узел для сервера (без оболочки) — управление из консоли: yandi setup, yandi admin" "  1) Node for a server (no shell) — managed from the console: yandi setup, yandi admin")"
    echo "$(T "  2) Узел и оболочка ПК (окно программы)" "  2) Node and the desktop shell (a program window)")"
    read -r -p "$(T "Выбор" "Choice") [1/2]: " c
    case "$c" in 2) MODE=desktop;; *) MODE=server;; esac
  else MODE=server; fi
fi
echo "$(T "Режим: $MODE" "Mode: $MODE")"

W=$(mktemp -d); chmod 700 "$W"; trap 'rm -rf "$W"' EXIT
fetch() { # имя -> файл в $W
  case "$FROM" in https://*) curl --fail --silent --show-error --proto '=https' --tlsv1.2 --max-redirs 0 --max-time 600 -o "$W/$1" "${FROM}$1";; *) cp -- "${FROM}$1" "$W/$1";; esac
}
fetch MANIFEST.json; fetch MANIFEST.json.sig
[ "$(stat -c %s "$W/MANIFEST.json")" -le 1048576 ] || die "$(T "манифест слишком велик" "the manifest is too large")"

# --- подпись: Ed25519 над «YANDI-RELEASE-MANIFEST-V1\0» + байты манифеста ---
printf '\x30\x2a\x30\x05\x06\x03\x2b\x65\x70\x03\x21\x00' > "$W/pub.der"
printf '%s' "$PUBKEY" | sed 's/../\\x&/g' | xargs -0 printf > "$W/pub.raw"
[ "$(stat -c %s "$W/pub.raw")" = 32 ] || die "$(T "ключ должен быть ровно 32 байта" "the key must be exactly 32 bytes")"
cat "$W/pub.raw" >> "$W/pub.der"
openssl pkey -pubin -inform DER -in "$W/pub.der" -out "$W/pub.pem" 2>/dev/null || die "$(T "ключ не читается" "the key cannot be read")"
tr -d ' \n\r' < "$W/MANIFEST.json.sig" | grep -Eq '^[0-9a-f]{128}$' || die "$(T "подпись должна быть 128 шестнадцатеричных знаков" "the signature must be 128 hexadecimal characters")"
tr -d ' \n\r' < "$W/MANIFEST.json.sig" | sed 's/../\\x&/g' | xargs -0 printf > "$W/sig.bin"
{ printf 'YANDI-RELEASE-MANIFEST-V1\0'; cat "$W/MANIFEST.json"; } > "$W/signed.bin"
openssl pkeyutl -verify -rawin -pubin -inkey "$W/pub.pem" -in "$W/signed.bin" -sigfile "$W/sig.bin" >/dev/null 2>&1 \
  || die "$(T "ПОДПИСЬ ВЫПУСКА НЕ СОВПАДАЕТ — не устанавливайте этот выпуск" "RELEASE SIGNATURE DOES NOT MATCH — do not install this release")"
echo "$(T "Подпись выпуска верна (ключ ${PUBKEY:0:16}…). Сверьте этот отпечаток с сайтом yandi.su/network/downloads." "The release signature is valid (key ${PUBKEY:0:16}…). Compare this fingerprint with the site yandi.su/network/downloads.")"

# --- содержимое манифеста: строгий разбор JSON (данные уже подписаны, но формат проверяется целиком) ---
command -v python3 >/dev/null || die "$(T "нужен python3 (разбор манифеста): sudo apt install python3-minimal" "python3 is required (manifest parsing): sudo apt install python3-minimal")"
ARCH=$(dpkg --print-architecture 2>/dev/null || echo amd64)
KEY_ID=$(sha256sum "$W/pub.raw" | cut -c1-16)
PARSED=$(python3 -I -B - "$W/MANIFEST.json" "$KEY_ID" "$LANG_SEL" <<'PY'
import json, re, sys, time, unicodedata

class Bad(Exception):
    pass

# язык сообщений: третий аргумент (ru | en)
L = (lambda ru, en: ru) if sys.argv[3] == "ru" else (lambda ru, en: en)

def need(cond, msg):
    if not cond:
        raise Bad(msg)

def is_int(x):
    return type(x) is int

def pairs(items):
    d = {}
    for k, v in items:
        need(k not in d, L("повторяющееся поле ", "duplicate field ") + k)
        d[k] = v
    return d

def nofloat(x):
    raise Bad(L("недопустимое число ", "invalid number ") + x)

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
    need(len(raw) <= 1 << 20, L("манифест слишком велик", "the manifest is too large"))
    m = json.loads(raw.decode("utf-8"), object_pairs_hook=pairs, parse_float=nofloat, parse_constant=nofloat)
    need(type(m) is dict, L("корень манифеста — не объект", "the manifest root is not an object"))
    need(ROOT_REQUIRED <= set(m) <= ROOT_ALLOWED, L("состав полей манифеста не совпадает со схемой", "the manifest fields do not match the schema"))
    need(is_int(m["format"]) and m["format"] == 1, L("формат манифеста не поддерживается", "the manifest format is not supported"))
    need(m["product"] == "yandi-node", L("это не выпуск yandi-node", "this is not a yandi-node release"))
    need(m.get("platform") == "deb", L("это не выпуск для Debian/Ubuntu", "this is not a release for Debian/Ubuntu"))
    v = m["version"]
    # как в src/release.rs: до 200 БАЙТ UTF-8, без управляющих знаков Юникода (категория Cc, включая C1)
    need(type(v) is str and 1 <= len(v.encode("utf-8")) <= 200 and not any(unicodedata.category(c) == "Cc" for c in v), L("неверная версия", "invalid version"))
    seq, created, exp = m["seq"], m["created"], m.get("expires")
    need(is_int(seq) and 1 <= seq < 2**53, L("неверный номер выпуска", "invalid release number"))
    need(is_int(created) and 0 <= created < 2**63, L("неверное время выпуска", "invalid release time"))
    need(is_int(exp) and 0 < exp < 2**63 and exp > time.time(), L("срок действия выпуска истёк или не указан", "the release has expired or has no expiry"))
    need(type(m["key_id"]) is str and re.fullmatch(r"[0-9a-f]{16}", m["key_id"]), L("неверный идентификатор ключа", "invalid key id"))
    need(m["key_id"] == sys.argv[2], L("идентификатор ключа в манифесте не совпадает с закреплённым ключом", "the key id in the manifest does not match the pinned key"))
    notes = m.get("notes", "")
    need(type(notes) is str and len(notes) <= 2000, L("неверные заметки", "invalid notes"))
    sw = m.get("support", [])
    # кошельки проекта: до 8, те же правила, что в src/release.rs (SupportWallet::valid): монета 1..16, сеть 1..32 знаков без управляющих и «|»,
    # адрес Tron с Base58Check или EVM с EIP-55, дубликатов нет
    need(type(sw) is list and len(sw) <= 8, L("неверный список кошельков поддержки", "invalid list of support wallets"))
    seen = set()
    for w in sw:
        need(type(w) is dict and set(w) == {"asset", "network", "address"} and all(type(x) is str for x in w.values()), L("неверный кошелёк поддержки", "invalid support wallet"))
        need(1 <= len(w["asset"]) <= 16 and 1 <= len(w["network"]) <= 32, L("неверная длина монеты или сети кошелька", "invalid length of the wallet coin or network"))
        need(not any(unicodedata.category(c) == "Cc" or c == "|" for c in w["asset"] + w["network"]), L("недопустимые знаки в монете или сети кошелька", "invalid characters in the wallet coin or network"))
        need(wallet_ok(w["network"], w["address"]), L("адрес кошелька не проходит проверку контрольной суммы", "the wallet address fails the checksum check"))
        key = (w["asset"], w["network"], w["address"])
        need(key not in seen, L("кошелёк указан дважды", "the wallet is listed twice"))
        seen.add(key)
    files = m["files"]
    need(type(files) is list and 1 <= len(files) <= 32, L("неверный список файлов", "invalid file list"))
    names = set()
    out = ["SEQ %d" % seq]
    for f in files:
        need(type(f) is dict and set(f) == {"name", "sha256", "size"}, L("запись файла должна содержать ровно name, sha256, size", "a file entry must contain exactly name, sha256, size"))
        n, h, z = f["name"], f["sha256"], f["size"]
        need(type(n) is str and re.fullmatch(r"[A-Za-z0-9._-]{1,128}", n) and not n.startswith("."), L("недопустимое имя файла", "invalid file name"))
        need(type(h) is str and re.fullmatch(r"[0-9a-f]{64}", h), L("неверная сумма", "invalid checksum"))
        need(is_int(z) and 1 <= z <= 256 * 1024 * 1024, L("неверный размер", "invalid size"))
        need(n not in names, L("файл указан дважды", "the file is listed twice"))
        names.add(n)
        out.append("FILE %s %s %d" % (n, h, z))
    print("\n".join(out))
except Bad as e:
    sys.stderr.write(L("манифест отвергнут: %s\n", "manifest rejected: %s\n") % e)
    sys.exit(1)
except Exception as e:
    sys.stderr.write(L("манифест отвергнут: %s\n", "manifest rejected: %s\n") % e)
    sys.exit(1)
PY
) || die "$(T "манифест не прошёл строгую проверку" "the manifest failed the strict check")"
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
  || die "$(T "каталог состояния $STATE_DIR небезопасен (ссылка, чужой владелец, доступ другим или небезопасный родитель); нужен закрытый каталог 0700" "the state directory $STATE_DIR is unsafe (a link, a foreign owner, access for others or an unsafe parent); a private 0700 directory is required")"
command -v flock >/dev/null || die "$(T "нужен flock (пакет util-linux)" "flock is required (package util-linux)")"
# файл блокировки: только обычный файл нашего владельца, без ссылок и каналов; открывается без обрезания
if [ -e "$LOCK_FILE" ] || [ -L "$LOCK_FILE" ]; then
  [ -f "$LOCK_FILE" ] && [ ! -L "$LOCK_FILE" ] && [ "$(stat -c %u "$LOCK_FILE")" = "$ME" ] && [ -z "$(find "$LOCK_FILE" -maxdepth 0 -perm /022)" ] || die "$(T "файл блокировки $LOCK_FILE небезопасен" "the lock file $LOCK_FILE is unsafe")"
fi
exec 9>>"$LOCK_FILE" || die "$(T "не удалось открыть файл блокировки" "could not open the lock file")"
flock -x 9
read_seq() {
  [ ! -L "$SEQ_FILE" ] || die "$(T "файл $SEQ_FILE — символическая ссылка: небезопасно" "the file $SEQ_FILE is a symbolic link: unsafe")"
  [ -e "$SEQ_FILE" ] || { echo 0; return; }
  [ -f "$SEQ_FILE" ] && [ "$(stat -c %u "$SEQ_FILE")" = "$ME" ] && [ -r "$SEQ_FILE" ] && [ -z "$(find "$SEQ_FILE" -maxdepth 0 -perm /022)" ] || die "$(T "файл $SEQ_FILE небезопасен или нечитаем" "the file $SEQ_FILE is unsafe or unreadable")"
  local v; v=$(cat "$SEQ_FILE") || die "$(T "файл $SEQ_FILE нечитаем" "the file $SEQ_FILE is unreadable")"
  [[ "$v" =~ ^[0-9]{1,16}$ ]] && [ "$v" -lt 9007199254740992 ] || die "$(T "файл $SEQ_FILE повреждён (ожидалось одно число): удалите его, если уверены в выпуске" "the file $SEQ_FILE is damaged (a single number was expected): delete it if you trust the release")"
  echo "$v"
}
OLD=$(read_seq)
[ "$SEQ" -ge "$OLD" ] || die "$(T "выпуск №$SEQ старше уже установленного №$OLD (откат): установка отменена" "release #$SEQ is older than the installed #$OLD (a rollback): installation cancelled")"
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
    [ "$(stat -c %s "$W/$name")" = "$size" ] || die "$(T "$name: размер не совпадает с манифестом" "$name: the size does not match the manifest")"
    [ "$(sha256sum "$W/$name" | cut -d' ' -f1)" = "$sha" ] || die "$(T "$name: СУММА НЕ СОВПАДАЕТ — файл подменён или повреждён" "$name: CHECKSUM DOES NOT MATCH — the file was replaced or damaged")"
    echo "$(T "Файл проверен: $name" "File verified: $name")"; found="$W/$name"; DEBS+=("$found")
  done
  [ -n "$found" ] || die "$(T "в выпуске нет пакета ${prefix}…_${ARCH}.deb" "the release has no package ${prefix}…_${ARCH}.deb")"
done
[ "$VERIFY_ONLY" = 1 ] && { echo "$(T "Проверка выпуска завершена (--verify-only): ничего не установлено." "Release verification finished (--verify-only): nothing was installed.")"; exit 0; }
# подтверждение установки (в терминале; --yes и работа без терминала — без вопроса). Ответ строго по раскладке выбранного языка.
if [ "$YES" = 0 ] && [ -t 0 ] && [ -t 1 ]; then
  echo "$(T "Будет установлено:" "To be installed:")"
  for d in "${DEBS[@]}"; do echo "  $(basename "$d")"; done
  rc=0; ask_yn "Подтвердите установку?" "Confirm the installation?" y || rc=$?
  [ "$rc" = 0 ] || { echo "$(T "Установка отменена." "Installation cancelled.")"; exit 1; }
fi

# --- установка ---
sudo apt-get install -y "${DEBS[@]}"
# номер фиксируется как максимум под той же блокировкой, атомарной заменой файла
CUR=$(read_seq); [ "$SEQ" -ge "$CUR" ] || SEQ="$CUR"
TMP_SEQ=$(mktemp "$STATE_DIR/installed-seq.XXXXXX") || die "$(T "пакеты установлены, но номер выпуска не сохранён (не создать временный файл в $STATE_DIR)" "packages are installed, but the release number was not saved (cannot create a temporary file in $STATE_DIR)")"
{ printf '%s\n' "$SEQ" > "$TMP_SEQ" && chmod 600 "$TMP_SEQ" && mv -f "$TMP_SEQ" "$SEQ_FILE"; } || { rm -f "$TMP_SEQ"; die "$(T "пакеты установлены, но номер выпуска не сохранён в $SEQ_FILE: защита от отката не обновлена" "packages are installed, but the release number was not saved to $SEQ_FILE: rollback protection was not updated")"; }
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
  [ -S "$SOCK" ] || echo "$(T "Узел ещё запускается: смотрите journalctl -u yandi-node" "The node is still starting: see journalctl -u yandi-node")"
fi
echo
echo "$(T "Установлено. Дальше:" "Installed. Next:")"
echo "$(T "  yandi setup        первая настройка (пароль и фраза восстановления)" "  yandi setup        first-time setup (password and recovery phrase)")"
echo "$(T "  yandi admin        управление узлом; yandi admin pairing — код сопряжения телефона" "  yandi admin        node management; yandi admin pairing — phone pairing code")"
echo "$(T "  Порты: откройте в брандмауэре порты узла (см. /usr/share/doc/yandi-node/DEPLOYMENT.md)." "  Ports: open the node ports in the firewall (see /usr/share/doc/yandi-node/DEPLOYMENT.md).")"
if [ "$YES" = 0 ] && [ -t 0 ] && [ -t 1 ] && command -v yandi >/dev/null; then
  rc=0; ask_yn "Запустить первую настройку сейчас?" "Run the first-time setup now?" y || rc=$?
  if [ "$rc" = 0 ]; then exec 9>&-; exec yandi setup; fi
fi
