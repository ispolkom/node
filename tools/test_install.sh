#!/usr/bin/env bash
# Проверка установщика на собственном подписанном выпуске-образце (временный ключ Ed25519, мелкие поддельные пакеты): верный проходит,
# а любая подмена, чужой ключ, истёкший срок, неверная структура манифеста и откат номера выпуска — нет.
# Каждая подмена проверяется на то, что вход действительно изменился. Необязательный аргумент — каталог настоящего выпуска (положительная проверка).
#   bash tools/test_install.sh [КАТАЛОГ_НАСТОЯЩЕГО_ВЫПУСКА]
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
fail=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fail=1; }
ARCH=$(dpkg --print-architecture 2>/dev/null || echo amd64)

openssl genpkey -algorithm ed25519 -out "$W/key.pem" 2>/dev/null
PUB=$(openssl pkey -in "$W/key.pem" -pubout -outform DER 2>/dev/null | tail -c 32 | od -An -tx1 | tr -d ' \n')
OTHER=$(openssl genpkey -algorithm ed25519 2>/dev/null | openssl pkey -pubout -outform DER 2>/dev/null | tail -c 32 | od -An -tx1 | tr -d ' \n')

# make_release DIR [ИМЯ=ЗНАЧЕНИЕ ...]: срок, seq, product, format, platform, extra, dup, compact
make_release() {
  local d="$1"; shift; mkdir -p "$d"
  head -c 300 /dev/urandom > "$d/yandi-node_0.0.1_${ARCH}.deb"
  head -c 500 /dev/urandom > "$d/yandi-desktop_0.0.1_${ARCH}.deb"
  python3 - "$d" "$ARCH" "$@" <<'PY'
import hashlib, json, sys, time, subprocess, os
d, arch = sys.argv[1], sys.argv[2]
o = dict(a.split("=", 1) for a in sys.argv[3:])
files = []
for n in ("yandi-node_0.0.1_%s.deb" % arch, "yandi-desktop_0.0.1_%s.deb" % arch):
    b = open(os.path.join(d, n), "rb").read()
    files.append({"name": n, "sha256": hashlib.sha256(b).hexdigest(), "size": len(b)})
if "dup" in o: files.append(dict(files[0]))
m = {"format": int(o.get("format", 1)), "product": o.get("product", "yandi-node"), "version": "0.0.1", "seq": int(o.get("seq", 10)),
     "created": 1, "key_id": "test", "files": files, "platform": o.get("platform", "deb"),
     "expires": int(o.get("expires", time.time() + 86400)), "notes": ""}
if "extra" in o: m["files"][0]["mode"] = "x"
text = json.dumps(m, separators=(",", ":")) if "compact" in o else json.dumps(m, indent=2)
if "broken" in o: text = text.rstrip()[:-1]
open(os.path.join(d, "MANIFEST.json"), "w").write(text)
PY
  { printf 'YANDI-RELEASE-MANIFEST-V1\0'; cat "$d/MANIFEST.json"; } > "$W/msg.bin"
  openssl pkeyutl -sign -rawin -inkey "$W/key.pem" -in "$W/msg.bin" -out "$W/sig.bin" 2>/dev/null
  od -An -tx1 -v "$W/sig.bin" | tr -d ' \n' > "$d/MANIFEST.json.sig"
}
flip_byte() { python3 - "$1" "$2" <<'PY'
import sys
p, off = sys.argv[1], int(sys.argv[2]); b = bytearray(open(p, "rb").read()); b[off] ^= 0x01; open(p, "wb").write(b)
PY
}

run_inst() { XDG_STATE_HOME="$W/state" bash "$HERE/install.sh" --verify-only "$@" >"$W/out.txt" 2>&1; }
pass() { local n="$1"; shift; run_inst "$@" && ok "$n" || { bad "$n"; sed 's/^/     /' "$W/out.txt" | tail -4; }; }
fails() { local n="$1"; shift; run_inst "$@" && bad "$n (принято, а не должно)" || ok "$n"; }
differs() { cmp -s "$1" "$2" && { bad "подмена не изменила вход: $3"; return 1; }; return 0; }

make_release "$W/good"
pass "верный выпуск (сервер)" --server --from "$W/good" --pubkey "$PUB"
pass "верный выпуск (оболочка)" --desktop --from "$W/good" --pubkey "$PUB"
make_release "$W/compact" compact=1
pass "верный компактный JSON" --server --from "$W/compact" --pubkey "$PUB"
fails "чужой ключ" --server --from "$W/good" --pubkey "$OTHER"
fails "http вместо https" --server --from "http://example.invalid/" --pubkey "$PUB"

cp -r "$W/good" "$W/m"; flip_byte "$W/m/MANIFEST.json" 20; differs "$W/good/MANIFEST.json" "$W/m/MANIFEST.json" манифест && fails "изменён байт манифеста" --server --from "$W/m" --pubkey "$PUB"
cp -r "$W/good" "$W/s"; python3 - "$W/s/MANIFEST.json.sig" <<'PY'
import sys
p = sys.argv[1]; t = open(p).read(); c = t[5]; open(p, "w").write(t[:5] + ("0" if c != "0" else "1") + t[6:])
PY
differs "$W/good/MANIFEST.json.sig" "$W/s/MANIFEST.json.sig" подпись && fails "изменена подпись" --server --from "$W/s" --pubkey "$PUB"
printf '' > "$W/empty.sig"; cp -r "$W/good" "$W/e"; cp "$W/empty.sig" "$W/e/MANIFEST.json.sig"; fails "пустая подпись" --server --from "$W/e" --pubkey "$PUB"
cp -r "$W/good" "$W/d"; f=$(ls "$W/d"/yandi-node_*.deb | head -1); flip_byte "$f" 100; differs "$W/good/$(basename "$f")" "$f" пакет && fails "изменён байт в пакете" --server --from "$W/d" --pubkey "$PUB"
cp -r "$W/good" "$W/t"; f=$(ls "$W/t"/yandi-node_*.deb | head -1); head -c 299 "$f" > "$f.cut"; mv "$f.cut" "$f"; fails "обрезан пакет" --server --from "$W/t" --pubkey "$PUB"
cp -r "$W/good" "$W/n"; rm -f "$W/n"/yandi-node_*.deb; fails "пакета нет" --server --from "$W/n" --pubkey "$PUB"

# верно подписанные, но неверные по содержанию манифесты
make_release "$W/x1" expires=1;                 fails "истёкший срок" --server --from "$W/x1" --pubkey "$PUB"
make_release "$W/x2" product=other;             fails "чужой продукт" --server --from "$W/x2" --pubkey "$PUB"
make_release "$W/x3" format=999;                fails "неизвестный формат" --server --from "$W/x3" --pubkey "$PUB"
make_release "$W/x4" platform=exe;              fails "не та платформа" --server --from "$W/x4" --pubkey "$PUB"
make_release "$W/x5" dup=1;                     fails "файл указан дважды" --server --from "$W/x5" --pubkey "$PUB"
make_release "$W/x6" extra=1;                   fails "лишнее поле в записи файла" --server --from "$W/x6" --pubkey "$PUB"
make_release "$W/x7" broken=1;                  fails "незакрытый JSON" --server --from "$W/x7" --pubkey "$PUB"
# откат: уже установлен выпуск №50, предлагают №10
mkdir -p "$W/state/yandi"; echo 50 > "$W/state/yandi/installed-seq"
fails "откат на более старый выпуск" --server --from "$W/good" --pubkey "$PUB"
echo 10 > "$W/state/yandi/installed-seq"; pass "тот же номер выпуска допустим" --server --from "$W/good" --pubkey "$PUB"

[ -n "${1:-}" ] && { pass "настоящий выпуск (сервер)" --server --from "$1"; pass "настоящий выпуск (оболочка)" --desktop --from "$1"; }
[ $fail = 0 ] && echo "ВСЕ ПРОВЕРКИ УСТАНОВЩИКА ПРОШЛИ" || { echo "ЕСТЬ ОШИБКИ"; exit 1; }
