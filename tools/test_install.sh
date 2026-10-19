#!/usr/bin/env bash
# Проверка установщика на настоящем подписанном выпуске: верный проходит, подмена манифеста, файла, ключа и истёкший срок — нет.
#   bash tools/test_install.sh КАТАЛОГ_С_ВЫПУСКОМ   (MANIFEST.json, MANIFEST.json.sig, *.deb)
set -uo pipefail
SRC="${1:?каталог выпуска}"; HERE="$(cd "$(dirname "$0")" && pwd)"
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
fail=0
ok()   { echo "ok   $*"; }
bad()  { echo "FAIL $*"; fail=1; }
expect_pass() { local n="$1"; shift; bash "$HERE/install.sh" --verify-only "$@" >/dev/null 2>&1 && ok "$n" || bad "$n"; }
expect_fail() { local n="$1"; shift; bash "$HERE/install.sh" --verify-only "$@" >/dev/null 2>&1 && bad "$n (принято, а не должно)" || ok "$n"; }

expect_pass "верный выпуск (сервер)" --server --from "$SRC"
expect_pass "верный выпуск (оболочка)" --desktop --from "$SRC"
expect_fail "чужой ключ" --server --from "$SRC" --pubkey "$(printf 'a%.0s' $(seq 64))"
expect_fail "http вместо https" --server --from "http://example.invalid/"

cp -r "$SRC" "$W/m"; sed -i 's/"seq": [0-9]*/"seq": 1/' "$W/m/MANIFEST.json"
expect_fail "изменён манифест" --server --from "$W/m"

cp -r "$SRC" "$W/s"; printf '%s' "$(head -c 127 "$W/s/MANIFEST.json.sig")0" > "$W/s/MANIFEST.json.sig"
expect_fail "изменена подпись" --server --from "$W/s"

cp -r "$SRC" "$W/d"; f=$(ls "$W/d"/yandi-node_*.deb | head -1); printf '\x00' | dd of="$f" bs=1 seek=100 conv=notrunc 2>/dev/null
expect_fail "изменён байт в пакете" --server --from "$W/d"

cp -r "$SRC" "$W/t"; f=$(ls "$W/t"/yandi-node_*.deb | head -1); head -c $(( $(stat -c %s "$f") - 5 )) "$f" > "$f.cut"; mv "$f.cut" "$f"
expect_fail "обрезан пакет" --server --from "$W/t"

cp -r "$SRC" "$W/n"; rm -f "$W/n"/yandi-node_*.deb
expect_fail "пакета нет" --server --from "$W/n"

[ $fail = 0 ] && echo "ВСЕ ПРОВЕРКИ УСТАНОВЩИКА ПРОШЛИ" || { echo "ЕСТЬ ОШИБКИ"; exit 1; }
