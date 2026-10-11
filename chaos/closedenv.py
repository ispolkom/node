#!/usr/bin/env python3
"""Закрытая среда узла на настоящем процессе: настройка, перенос открытого файла ключей, перезапуск ЗАПЕРТЫМ, вход, смена пароля.
Только стандартная библиотека. Использование: closedenv.py [каталог] [бинарь]. Каталог — только в /tmp, боевые данные не трогаются."""
import sys, os, time, json, base64, shutil
sys.path.insert(0, os.path.dirname(__file__))
import chaos as C

root = sys.argv[1] if len(sys.argv) > 1 else "/tmp/claude_closedenv"
binary = os.path.abspath(sys.argv[2]) if len(sys.argv) > 2 else os.path.abspath("target/debug/yandi")
assert root.startswith("/tmp/"), "тестовые узлы только из /tmp"
shutil.rmtree(root, ignore_errors=True)
os.makedirs(root)
fails = []

def check(name, cond, extra=""):
    print(("ok   " if cond else "FAIL ") + name + (" " + str(extra) if extra and not cond else ""))
    if not cond:
        fails.append(name)

n = C.Node(0, root, binary, base_port=27400)
data = os.path.join(n.dir, "home/.local/share/yandi")
os.makedirs(data, exist_ok=True)
# прежняя установка: ключи «ПК как собеседник» лежат открыто
plain = os.path.join(data, "mobile_self.json")
with open(plain, "w") as f:
    json.dump({"x25519_secret": base64.b64encode(bytes([5]) * 32).decode(), "ed25519_secret": base64.b64encode(bytes([6]) * 32).decode()}, f)

def boot():
    n.start()
    assert n.login(), "вход в веб-интерфейс"
    assert n.ready(), "узел поднялся"
    return n.api("GET", "/api/vault/status")

try:
    st, body = boot()
    check("до настройки: не создана", st == 200 and body["configured"] is False and body["unlocked"] is False, body)
    st, b = n.api("POST", "/api/vault/setup", {"password": "short", "phrase": None})
    check("короткий пароль отклонён", st == 400, (st, b))
    st, b = n.api("POST", "/api/vault/setup", {"password": "owner password", "phrase": "alpha1 bravo2 charlie3 delta4"})
    check("настройка", st == 200 and b["status"]["unlocked"] and not b["status"]["migration_pending"], (st, b))
    enc = os.path.join(data, "mobile_self.enc")
    check("открытый файл ключей перенесён и убран", os.path.exists(enc) and not os.path.exists(plain))
    enc_bytes = open(enc, "rb").read() if os.path.exists(enc) else b""
    closed = os.path.join(data, "closed")
    check("каталог закрытой среды 0700", os.path.isdir(closed) and (os.stat(closed).st_mode & 0o777) == 0o700)
    env = open(os.path.join(closed, "envelope.json"), "rb").read() if os.path.exists(os.path.join(closed, "envelope.json")) else b""
    check("envelope без открытых секретов", env and b"owner password" not in env)

    st, b = n.api("POST", "/api/vault/lock", {})
    check("блокировка", st == 200 and not b["status"]["unlocked"], (st, b))

    # перезапуск узла: среда есть, заперта сама, пароль не вводился
    n.kill()
    time.sleep(1)
    st, body = boot()
    check("после перезапуска заперта", st == 200 and body["configured"] and not body["unlocked"], body)
    st, b = n.api("POST", "/api/vault/unlock", {"password": "wrong password"})
    check("неверный пароль: 401", st == 401, (st, b))
    st, b = n.api("POST", "/api/vault/unlock", {"password": "owner password"})
    check("вход паролем", st == 200 and b["status"]["unlocked"], (st, b))
    check("файл ключей не менялся при входе", open(enc, "rb").read() == enc_bytes)

    st, b = n.api("POST", "/api/vault/change_password", {"old_password": "owner password", "new_password": "second password"})
    check("смена пароля", st == 200, (st, b))
    st, b = n.api("POST", "/api/vault/lock", {})
    st, b = n.api("POST", "/api/vault/unlock", {"password": "owner password"})
    check("старый пароль не работает", st in (401, 429), (st, b))
    # после неверной попытки действует счётчик; правильный пароль проходит, пока свободные попытки не исчерпаны
    st, b = n.api("POST", "/api/vault/unlock", {"password": "second password"})
    check("новый пароль работает", st == 200 and b["status"]["unlocked"], (st, b))
finally:
    n.kill()

print("FAILS:", fails if fails else "none")
sys.exit(1 if fails else 0)
