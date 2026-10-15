#!/usr/bin/env python3
"""Заменяет RUNPATH/RPATH всех ELF каталога на «$ORIGIN» (строка в .dynstr перезаписывается на месте, длина не меняется).
В пакет не должны попадать пути сборочного дерева. Использование: fix_runpath.py <каталог бандла>"""
import os, re, subprocess, sys

def runpath(path):
    out = subprocess.run(["readelf", "-d", path], capture_output=True, text=True, env={**os.environ, "LC_ALL": "C"}).stdout
    m = re.search(r"\((?:RUNPATH|RPATH)\)\s+Library (?:runpath|rpath): \[(.*)\]", out)
    return m.group(1) if m else None

def main(root):
    changed = 0
    for d, _, files in os.walk(root):
        for f in files:
            p = os.path.join(d, f)
            if os.path.islink(p) or not os.path.isfile(p):
                continue
            with open(p, "rb") as fh:
                if fh.read(4) != b"\x7fELF":
                    continue
            rp = runpath(p)
            if rp is None or all(x.startswith("$ORIGIN") for x in rp.split(":")):
                continue
            data = open(p, "rb").read()
            old = rp.encode() + b"\0"
            i = data.find(old)
            if i < 0:
                sys.exit(f"{p}: строку RUNPATH не нашли в файле")
            new = b"$ORIGIN" + b"\0" * (len(old) - len(b"$ORIGIN"))
            data = data[:i] + new + data[i + len(old):]
            tmp = p + ".tmp"
            with open(tmp, "wb") as fh:
                fh.write(data)
            os.chmod(tmp, os.stat(p).st_mode)
            os.replace(tmp, p)
            changed += 1
    # проверка: ни одного абсолютного пути сборки
    for d, _, files in os.walk(root):
        for f in files:
            p = os.path.join(d, f)
            if os.path.isfile(p) and not os.path.islink(p):
                with open(p, "rb") as fh:
                    if fh.read(4) == b"\x7fELF":
                        rp = runpath(p)
                        if rp and any(not x.startswith("$ORIGIN") for x in rp.split(":")):
                            sys.exit(f"{p}: RUNPATH всё ещё {rp}")
    print(f"RUNPATH исправлен у {changed} файлов")

main(sys.argv[1])
