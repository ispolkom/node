#!/usr/bin/env bash
# Проверка установщика на собственном подписанном выпуске-образце (временный ключ Ed25519, мелкие поддельные пакеты): верный проходит,
# а любая подмена, чужой ключ, истёкший срок, неверная схема манифеста, откат и порча состояния — нет. Каждый отказ проверяется ПО ПРИЧИНЕ
# (ожидаемый текст в выводе), каждая подмена — на то, что вход действительно изменился. Установка проверяется с записывающей заглушкой sudo.
# Необязательный аргумент — каталог настоящего выпуска (положительная проверка).
#   bash tools/test_install.sh [КАТАЛОГ_НАСТОЯЩЕГО_ВЫПУСКА]
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
# сообщения проверяются по-русски; выбор языка не пишется в настоящий ~/.config
export YANDI_LANG=ru XDG_CONFIG_HOME="$W/cfg"
fail=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fail=1; }
ARCH=$(dpkg --print-architecture 2>/dev/null || echo amd64)

openssl genpkey -algorithm ed25519 -out "$W/key.pem" 2>/dev/null
PUB=$(openssl pkey -in "$W/key.pem" -pubout -outform DER 2>/dev/null | tail -c 32 | od -An -tx1 | tr -d ' \n')
OTHER=$(openssl genpkey -algorithm ed25519 2>/dev/null | openssl pkey -pubout -outform DER 2>/dev/null | tail -c 32 | od -An -tx1 | tr -d ' \n')

KEYID=$(printf '%s' "$PUB" | sed 's/../\\x&/g' | xargs -0 printf | sha256sum | cut -c1-16)
# make_release DIR [ИМЯ=ЗНАЧЕНИЕ ...]: (key_id ставится равным отпечатку ключа; wrongkey, version=… переопределяют) expires, seq, product, format(=true|1.0|N), platform, extra, dup, compact, broken, drop=ПОЛЕ, root_extra
make_release() {
  local d="$1"; shift; mkdir -p "$d"
  head -c 300 /dev/urandom > "$d/yandi-node_0.0.1_${ARCH}.deb"
  head -c 500 /dev/urandom > "$d/yandi-desktop_0.0.1_${ARCH}.deb"
  KEYID_ENV="$KEYID" python3 - "$d" "$ARCH" "$@" <<'PY'
import hashlib, json, sys, time, os
d, arch = sys.argv[1], sys.argv[2]
o = dict(a.split("=", 1) for a in sys.argv[3:])
files = []
for n in ("yandi-node_0.0.1_%s.deb" % arch, "yandi-desktop_0.0.1_%s.deb" % arch):
    b = open(os.path.join(d, n), "rb").read()
    files.append({"name": n, "sha256": hashlib.sha256(b).hexdigest(), "size": len(b)})
if "dup" in o: files.append(dict(files[0]))
if "extra" in o: files[0]["mode"] = "x"
m = {"format": 1, "product": o.get("product", "yandi-node"), "version": o.get("version", "0.0.1").replace("@C1@", chr(0x85)).replace("@E101@", "é" * 101), "seq": int(o.get("seq", 10)),
     "created": 1, "key_id": ("0123456789abcdef" if "wrongkey" in o else os.environ["KEYID_ENV"]), "files": files, "platform": o.get("platform", "deb"),
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
cp -r "$W/good" "$W/n"; rm -f "$W/n"/yandi-node_*.deb; fails "пакета нет" "cp:|curl" --server --from "$W/n" --pubkey "$PUB"

# верно подписанные, но неверные по содержанию манифесты — отказ именно строгого разбора
for spec in "expires=1|срок действия" "product=other|не выпуск yandi-node" "format=999|формат манифеста" "format=true|формат манифеста" "format=1.0|недопустимое число" \
            "platform=exe|Debian" "dup=1|указан дважды" "extra=1|ровно name" "broken=1|манифест отвергнут" "drop=key_id|состав полей" \
            "wrongkey=1|не совпадает с закреплённым" "version=@E101@|неверная версия" "version=v@C1@|неверная версия" "drop=version|состав полей" "drop=created|состав полей" "root_extra=1|состав полей" "seq=0|номер выпуска" "seq=9007199254740993|номер выпуска"; do
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

echo 10 > "$STATE/yandi/installed-seq"; chmod 666 "$STATE/yandi/installed-seq"; fails "файл состояния открыт для записи другим" "небезопасен" --server --from "$W/good" --pubkey "$PUB"; chmod 600 "$STATE/yandi/installed-seq"
chmod 755 "$STATE/yandi"; fails "каталог состояния не закрытый (0755)" "небезопасен" --server --from "$W/good" --pubkey "$PUB"; chmod 700 "$STATE/yandi"
mkdir -p "$W/openparent"; chmod 777 "$W/openparent"; XDG_STATE_HOME="$W/openparent" bash "$HERE/install.sh" --verify-only --server --from "$W/good" --pubkey "$PUB" >"$W/out.txt" 2>&1 \
  && bad "родитель открыт для записи принят" || { grep -q "небезопасен" "$W/out.txt" && ok "родитель каталога состояния открыт для записи" || bad "родитель: не та причина"; }
printf 'valuable data' > "$W/victim"; rm -f "$STATE/yandi/lock"; ln -s "$W/victim" "$STATE/yandi/lock"
fails "lock — ссылка" "небезопасен" --server --from "$W/good" --pubkey "$PUB"; [ "$(cat "$W/victim")" = "valuable data" ] && ok "ссылка lock не обнуляет чужой файл" || bad "содержимое файла по ссылке lock потеряно"; rm -f "$STATE/yandi/lock"
mkfifo "$STATE/yandi/lock"; fails "lock — канал" "небезопасен" --server --from "$W/good" --pubkey "$PUB"; rm -f "$STATE/yandi/lock"

# настоящая установка с записывающей заглушкой sudo (apt не вызывается)
mkdir -p "$W/bin"; cat > "$W/bin/sudo" <<'STUB'
#!/usr/bin/env bash
echo "$*" >> "${STUB_LOG:-/dev/null}"
[ -n "${STUB_SLEEP:-}" ] && sleep "$STUB_SLEEP"
[ -n "${STUB_EVAL:-}" ] && eval "$STUB_EVAL"
exit "${STUB_RC:-0}"
STUB
chmod +x "$W/bin/sudo"
inst() { PATH="$W/bin:$PATH" XDG_STATE_HOME="$STATE" STUB_LOG="$W/sudo.log" bash "$HERE/install.sh" --no-service --yes "$@" >"$W/inst.txt" 2>&1; }
rm -f "$STATE/yandi/installed-seq" "$W/sudo.log"
make_release "$W/s10" seq=10; make_release "$W/s50" seq=50
inst --server --from "$W/s10" --pubkey "$PUB" && [ "$(cat "$STATE/yandi/installed-seq")" = 10 ] && grep -Eq "^apt-get install -y /.*/yandi-node_0.0.1_" "$W/sudo.log" && ok "установка записывает номер выпуска и ставит проверенный файл" || { bad "установка (seq 10)"; tail -3 "$W/inst.txt"; }
inst --server --from "$W/s50" --pubkey "$PUB" && [ "$(cat "$STATE/yandi/installed-seq")" = 50 ] && ok "новый выпуск поднимает номер" || bad "установка (seq 50)"
rm -f "$W/sudo.log"; inst --server --from "$W/s10" --pubkey "$PUB" && bad "откат установлен" || { [ ! -s "$W/sudo.log" ] && [ "$(cat "$STATE/yandi/installed-seq")" = 50 ] && ok "откат не доходит до установки и не меняет номер" || bad "откат: следы установки"; }
echo 10 > "$STATE/yandi/installed-seq"; rm -f "$W/sudo.log"
STUB_RC=1 inst --server --from "$W/s50" --pubkey "$PUB" && bad "сбой apt принят" || { [ "$(cat "$STATE/yandi/installed-seq")" = 10 ] && grep -q "^apt-get install" "$W/sudo.log" && ok "сбой установки (apt достигнут) не меняет номер" || bad "сбой установки: номер $(cat "$STATE/yandi/installed-seq")"; }
# сбой записи номера после успешной установки не скрывается
echo 10 > "$STATE/yandi/installed-seq"
STUB_EVAL='chmod 500 "$STATE/yandi"' STATE="$STATE" inst --server --from "$W/s50" --pubkey "$PUB" && bad "сбой записи номера скрыт" || { chmod 700 "$STATE/yandi"; grep -q "номер выпуска не сохранён" "$W/inst.txt" && [ "$(cat "$STATE/yandi/installed-seq")" = 10 ] && ok "сбой записи номера — ошибка, а не «установлено»" || bad "сбой записи номера: не та диагностика"; }
chmod 700 "$STATE/yandi"
# граничный номер выпуска: записывается и читается обратно
make_release "$W/big" seq=9007199254740991
echo 1 > "$STATE/yandi/installed-seq"
inst --server --from "$W/big" --pubkey "$PUB" && pass "после граничного номера состояние читается" --server --from "$W/big" --pubkey "$PUB"
# параллельные установки: итоговый номер — максимум
echo 5 > "$STATE/yandi/installed-seq"
( STUB_SLEEP=2 PATH="$W/bin:$PATH" XDG_STATE_HOME="$STATE" STUB_LOG="$W/sudoA.log" bash "$HERE/install.sh" --no-service --yes --server --from "$W/s10" --pubkey "$PUB" >"$W/A.txt" 2>&1; echo $? > "$W/A.rc" ) & sleep 0.5
PATH="$W/bin:$PATH" XDG_STATE_HOME="$STATE" STUB_LOG="$W/sudoB.log" bash "$HERE/install.sh" --no-service --yes --server --from "$W/s50" --pubkey "$PUB" >"$W/B.txt" 2>&1; echo $? > "$W/B.rc"; wait
[ "$(cat "$W/B.rc")" = 0 ] && [ "$(cat "$STATE/yandi/installed-seq")" = 50 ] && { [ "$(cat "$W/A.rc")" = 0 ] || grep -q "откат" "$W/A.txt"; } && ok "параллельные установки: B успешна, итог 50, A завершилась или отказала откатом" || bad "параллельные установки: A=$(cat "$W/A.rc") B=$(cat "$W/B.rc") номер=$(cat "$STATE/yandi/installed-seq")"

# --- два языка и подтверждение установки строго по раскладке выбранного языка
run_inst_en() { rm -rf "$W/state-en"; XDG_STATE_HOME="$W/state-en" bash "$HERE/install.sh" --lang en --verify-only "$@" >"$W/out.txt" 2>&1; }
run_inst_en --server --from "$W/good" --pubkey "$OTHER" && bad "английский: чужой ключ принят" || { grep -q "RELEASE SIGNATURE DOES NOT MATCH" "$W/out.txt" && ! grep -q "ПОДПИСЬ" "$W/out.txt" && ok "английский: отказ по причине, на английском" || { bad "английский: не та диагностика"; tail -3 "$W/out.txt"; }; }
run_inst_en --server --from "$W/good" --pubkey "$PUB" && grep -q "The release signature is valid" "$W/out.txt" && ok "английский: проверка верного выпуска" || bad "английский: верный выпуск"
bash "$HERE/install.sh" --lang de >"$W/out.txt" 2>&1; [ $? = 2 ] && ok "неизвестный язык — отказ" || bad "неизвестный язык принят"
# диалог в настоящем терминале (pty): ответ другой раскладки не засчитывается, правильный — да/нет
pty_dialog() { # pty_dialog ЯЗЫК ОТВЕТ… → вывод в $W/pty.txt, код возврата — установщика
  rm -rf "$W/state-pty"; PATH="$W/bin:$PATH" XDG_STATE_HOME="$W/state-pty" STUB_LOG="$W/sudo.pty.log" python3 - "$HERE/install.sh" "$W/good" "$PUB" "$W/pty.txt" "$@" <<'PY'
import os, pty, select, sys, time
script, rel, pub, out, lang, *answers = sys.argv[1:]
pid, fd = pty.fork()
if pid == 0:
    os.execvp("bash", ["bash", script, "--lang", lang, "--no-service", "--server", "--from", rel, "--pubkey", pub])
buf = b""
def read(t):
    global buf
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.2)
        if r:
            try:
                d = os.read(fd, 4096)
            except OSError:
                return
            if not d:
                return
            buf += d
for a in answers:
    read(2)
    try:
        os.write(fd, (a + "\n").encode())
    except OSError:
        break  # установщик уже завершился (последнего вопроса не было, например нет команды yandi)
read(4)
_, st = os.waitpid(pid, 0)
open(out, "w").write(buf.decode("utf-8", "replace"))
sys.exit(os.waitstatus_to_exitcode(st))
PY
}
rm -f "$W/sudo.pty.log"
pty_dialog ru y н; rc=$?
{ grep -q "Подтвердите установку? \[Д/н\]" "$W/pty.txt" && grep -q "Введите Д или Н" "$W/pty.txt" && [ "$rc" != 0 ] && [ ! -s "$W/sudo.pty.log" ] && grep -q "Установка отменена" "$W/pty.txt" && ok "русский: латинская y не засчитана, «н» — отмена, ничего не установлено" || { bad "русский диалог (код $rc)"; tail -6 "$W/pty.txt"; }; }
pty_dialog en н n; rc=$?
{ grep -q "Confirm the installation? \[Y/n\]" "$W/pty.txt" && grep -q "Did not understand" "$W/pty.txt" && [ "$rc" != 0 ] && [ ! -s "$W/sudo.pty.log" ] && grep -q "Installation cancelled" "$W/pty.txt" && ok "английский: кириллическая «н» не засчитана, n — отмена, ничего не установлено" || { bad "английский диалог (код $rc)"; tail -6 "$W/pty.txt"; }; }
pty_dialog en "" n; rc=$?
{ grep -q "^apt-get install" "$W/sudo.pty.log" 2>/dev/null && ok "английский: Enter — «да» по умолчанию, установка выполнена" || { bad "английский: Enter не подтвердил установку (код $rc)"; tail -6 "$W/pty.txt"; }; }
rm -f "$W/sudo.pty.log"
pty_dialog ru Д н; rc=$?
{ grep -q "^apt-get install" "$W/sudo.pty.log" 2>/dev/null && ok "русский: «Д» — установка выполнена" || { bad "русский: Д не подтвердил установку (код $rc)"; tail -6 "$W/pty.txt"; }; }

[ -n "${1:-}" ] && { STATE="$W/state-real"; pass "настоящий выпуск (сервер)" --server --from "$1"; pass "настоящий выпуск (оболочка)" --desktop --from "$1"; }
[ $fail = 0 ] && echo "ВСЕ ПРОВЕРКИ УСТАНОВЩИКА ПРОШЛИ" || { echo "ЕСТЬ ОШИБКИ"; exit 1; }
