#!/usr/bin/env bash
# Проверка установщика на собственном подписанном выпуске-образце (временный ключ Ed25519, мелкие поддельные пакеты): верный проходит,
# а любая подмена, чужой ключ, истёкший срок, неверная схема манифеста, откат и порча состояния — нет. Каждый отказ проверяется ПО ПРИЧИНЕ
# (ожидаемый текст в выводе), каждая подмена — на то, что вход действительно изменился. Установка проверяется с записывающей заглушкой sudo.
# Необязательный аргумент — каталог настоящего выпуска (положительная проверка).
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

# make_release DIR [ИМЯ=ЗНАЧЕНИЕ ...]: expires, seq, product, format(=true|1.0|N), platform, extra, dup, compact, broken, drop=ПОЛЕ, root_extra
make_release() {
  local d="$1"; shift; mkdir -p "$d"
  head -c 300 /dev/urandom > "$d/yandi-node_0.0.1_${ARCH}.deb"
  head -c 500 /dev/urandom > "$d/yandi-desktop_0.0.1_${ARCH}.deb"
  python3 - "$d" "$ARCH" "$@" <<'PY'
import hashlib, json, sys, time, os
d, arch = sys.argv[1], sys.argv[2]
o = dict(a.split("=", 1) for a in sys.argv[3:])
files = []
for n in ("yandi-node_0.0.1_%s.deb" % arch, "yandi-desktop_0.0.1_%s.deb" % arch):
    b = open(os.path.join(d, n), "rb").read()
    files.append({"name": n, "sha256": hashlib.sha256(b).hexdigest(), "size": len(b)})
if "dup" in o: files.append(dict(files[0]))
if "extra" in o: files[0]["mode"] = "x"
m = {"format": 1, "product": o.get("product", "yandi-node"), "version": "0.0.1", "seq": int(o.get("seq", 10)),
     "created": 1, "key_id": "0123456789abcdef", "files": files, "platform": o.get("platform", "deb"),
     "expires": int(o.get("expires", time.time() + 86400)), "notes": ""}
if "format" in o: m["format"] = {"true": True, "1.0": 1.0}.get(o["format"], None) if o["format"] in ("true", "1.0") else int(o["format"])
if "drop" in o: m.pop(o["drop"], None)
if "root_extra" in o: m["extra"] = 1
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

STATE="$W/state"
run_inst() { XDG_STATE_HOME="$STATE" bash "$HERE/install.sh" --verify-only "$@" >"$W/out.txt" 2>&1; }
pass() { local n="$1"; shift; run_inst "$@" && ok "$n" || { bad "$n"; sed 's/^/     /' "$W/out.txt" | tail -4; }; }
# fails ИМЯ ПРИЧИНА аргументы…: отказ обязан произойти и содержать причину
fails() { local n="$1" why="$2"; shift 2; if run_inst "$@"; then bad "$n (принято, а не должно)"; elif grep -Eq -- "$why" "$W/out.txt"; then ok "$n"; else bad "$n (отказ не по той причине; ждали «$why»)"; sed 's/^/     /' "$W/out.txt" | tail -3; fi; }
differs() { cmp -s "$1" "$2" && { bad "подмена не изменила вход: $3"; return 1; }; return 0; }

make_release "$W/good"
pass "верный выпуск (сервер)" --server --from "$W/good" --pubkey "$PUB"
pass "верный выпуск (оболочка)" --desktop --from "$W/good" --pubkey "$PUB"
make_release "$W/compact" compact=1
pass "верный компактный JSON" --server --from "$W/compact" --pubkey "$PUB"
fails "чужой ключ" "ПОДПИСЬ ВЫПУСКА НЕ СОВПАДАЕТ" --server --from "$W/good" --pubkey "$OTHER"
fails "http вместо https" "только https" --server --from "http://example.invalid/" --pubkey "$PUB"

cp -r "$W/good" "$W/m"; flip_byte "$W/m/MANIFEST.json" 20; differs "$W/good/MANIFEST.json" "$W/m/MANIFEST.json" манифест && fails "изменён байт манифеста" "ПОДПИСЬ ВЫПУСКА НЕ СОВПАДАЕТ" --server --from "$W/m" --pubkey "$PUB"
cp -r "$W/good" "$W/s"; python3 - "$W/s/MANIFEST.json.sig" <<'PY'
import sys
p = sys.argv[1]; t = open(p).read(); c = t[5]; open(p, "w").write(t[:5] + ("0" if c != "0" else "1") + t[6:])
PY
differs "$W/good/MANIFEST.json.sig" "$W/s/MANIFEST.json.sig" подпись && fails "изменена подпись" "ПОДПИСЬ ВЫПУСКА НЕ СОВПАДАЕТ" --server --from "$W/s" --pubkey "$PUB"
cp -r "$W/good" "$W/e"; : > "$W/e/MANIFEST.json.sig"; fails "пустая подпись" "128 шестнадцатеричных" --server --from "$W/e" --pubkey "$PUB"
cp -r "$W/good" "$W/d"; f=$(ls "$W/d"/yandi-node_*.deb | head -1); flip_byte "$f" 100; differs "$W/good/$(basename "$f")" "$f" пакет && fails "изменён байт в пакете" "СУММА НЕ СОВПАДАЕТ" --server --from "$W/d" --pubkey "$PUB"
cp -r "$W/good" "$W/t"; f=$(ls "$W/t"/yandi-node_*.deb | head -1); head -c 299 "$f" > "$f.cut"; mv "$f.cut" "$f"; fails "обрезан пакет" "размер не совпадает" --server --from "$W/t" --pubkey "$PUB"
cp -r "$W/good" "$W/n"; rm -f "$W/n"/yandi-node_*.deb; fails "пакета нет" "" --server --from "$W/n" --pubkey "$PUB"

# верно подписанные, но неверные по содержанию манифесты — отказ именно строгого разбора
for spec in "expires=1|срок действия" "product=other|не выпуск yandi-node" "format=999|формат манифеста" "format=true|формат манифеста" "format=1.0|недопустимое число" \
            "platform=exe|Debian" "dup=1|указан дважды" "extra=1|ровно name" "broken=1|манифест отвергнут" "drop=key_id|состав полей" \
            "drop=version|состав полей" "drop=created|состав полей" "root_extra=1|состав полей" "seq=0|номер выпуска" "seq=9007199254740993|номер выпуска"; do
  arg="${spec%%|*}"; why="${spec##*|}"; make_release "$W/x" "$arg"
  fails "манифест с $arg" "$why" --server --from "$W/x" --pubkey "$PUB"; rm -rf "$W/x"
done
make_release "$W/x" expires=1
PYTHONOPTIMIZE=2 run_inst --server --from "$W/x" --pubkey "$PUB" && bad "PYTHONOPTIMIZE отключил проверки" || { grep -q "срок действия" "$W/out.txt" && ok "PYTHONOPTIMIZE не отключает проверки" || bad "PYTHONOPTIMIZE: не та причина"; }

# состояние номера выпуска
mkdir -p "$STATE/yandi"; chmod 700 "$STATE/yandi"; echo 50 > "$STATE/yandi/installed-seq"
fails "откат на более старый выпуск" "откат" --server --from "$W/good" --pubkey "$PUB"
echo 10 > "$STATE/yandi/installed-seq"; pass "тот же номер выпуска допустим" --server --from "$W/good" --pubkey "$PUB"
echo corrupt > "$STATE/yandi/installed-seq"; fails "повреждённое состояние" "повреждён" --server --from "$W/good" --pubkey "$PUB"
chmod 000 "$STATE/yandi/installed-seq"; fails "нечитаемое состояние" "небезопасен или нечитаем|нечитаем" --server --from "$W/good" --pubkey "$PUB"; chmod 600 "$STATE/yandi/installed-seq"
rm "$STATE/yandi/installed-seq"; ln -s "$W/victim" "$STATE/yandi/installed-seq"; fails "состояние — ссылка" "символическая ссылка" --server --from "$W/good" --pubkey "$PUB"; rm -f "$STATE/yandi/installed-seq"
chmod 777 "$STATE/yandi"; fails "каталог состояния открыт для записи всем" "небезопасен" --server --from "$W/good" --pubkey "$PUB"; chmod 700 "$STATE/yandi"

# настоящая установка с записывающей заглушкой sudo (apt не вызывается)
mkdir -p "$W/bin"; cat > "$W/bin/sudo" <<'STUB'
#!/usr/bin/env bash
echo "$*" >> "${STUB_LOG:-/dev/null}"
[ -n "${STUB_SLEEP:-}" ] && sleep "$STUB_SLEEP"
exit "${STUB_RC:-0}"
STUB
chmod +x "$W/bin/sudo"
inst() { PATH="$W/bin:$PATH" XDG_STATE_HOME="$STATE" STUB_LOG="$W/sudo.log" bash "$HERE/install.sh" --no-service --yes "$@" >"$W/inst.txt" 2>&1; }
rm -f "$STATE/yandi/installed-seq" "$W/sudo.log"
make_release "$W/s10" seq=10; make_release "$W/s50" seq=50
inst --server --from "$W/s10" --pubkey "$PUB" && [ "$(cat "$STATE/yandi/installed-seq")" = 10 ] && grep -Eq "^apt-get install -y /.*/yandi-node_0.0.1_" "$W/sudo.log" && ok "установка записывает номер выпуска и ставит проверенный файл" || { bad "установка (seq 10)"; tail -3 "$W/inst.txt"; }
inst --server --from "$W/s50" --pubkey "$PUB" && [ "$(cat "$STATE/yandi/installed-seq")" = 50 ] && ok "новый выпуск поднимает номер" || bad "установка (seq 50)"
rm -f "$W/sudo.log"; inst --server --from "$W/s10" --pubkey "$PUB" && bad "откат установлен" || { [ ! -s "$W/sudo.log" ] && [ "$(cat "$STATE/yandi/installed-seq")" = 50 ] && ok "откат не доходит до установки и не меняет номер" || bad "откат: следы установки"; }
STUB_RC=1 inst --server --from "$W/s50" --pubkey "$PUB" && bad "сбой apt принят" || { [ "$(cat "$STATE/yandi/installed-seq")" = 50 ] && ok "сбой установки не меняет номер" || bad "сбой установки изменил номер"; }
# параллельные установки: итоговый номер — максимум
echo 5 > "$STATE/yandi/installed-seq"
( STUB_SLEEP=2 inst --server --from "$W/s10" --pubkey "$PUB" ) & sleep 0.5
inst --server --from "$W/s50" --pubkey "$PUB"; wait
[ "$(cat "$STATE/yandi/installed-seq")" = 50 ] && ok "параллельные установки: номер не уменьшился" || bad "параллельные установки: номер $(cat "$STATE/yandi/installed-seq")"

[ -n "${1:-}" ] && { STATE="$W/state-real"; pass "настоящий выпуск (сервер)" --server --from "$1"; pass "настоящий выпуск (оболочка)" --desktop --from "$1"; }
[ $fail = 0 ] && echo "ВСЕ ПРОВЕРКИ УСТАНОВЩИКА ПРОШЛИ" || { echo "ЕСТЬ ОШИБКИ"; exit 1; }
