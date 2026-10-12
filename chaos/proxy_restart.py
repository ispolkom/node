#!/usr/bin/env python3
"""Остановка SOCKS5-прокси узла: слушатель реально закрывается, повторный запуск работает без перезапуска узла.
Usage: python3 chaos/proxy_restart.py [binary] [out]   (out — на песочнице, например /run/proxy_restart)
Сценарий в rootless-лаборатории (клиент за NAT → выход → «интернет»): start → передача → stop → порт закрыт → start → передача."""
import json, os, socket, struct, sys, threading, time

sys.path.insert(0, os.path.dirname(__file__))
import chaos as C
import lab as LAB

PORT = 8099


def server():
    s = socket.socket()
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("0.0.0.0", PORT))
    s.listen(16)

    def serve(c):
        try:
            c.recv(1)
            c.sendall(b"PONG" * 1024)
        except OSError:
            pass
        finally:
            c.close()

    while True:
        c, _ = s.accept()
        threading.Thread(target=serve, args=(c,), daemon=True).start()


def socks_ping(ns, port, pw, host):
    s = LAB.ns_socket(ns, "127.0.0.1", port, 20)
    s.settimeout(30)
    s.sendall(b"\x05\x01\x02")
    assert s.recv(2) == b"\x05\x02"
    s.sendall(b"\x01\x05yandi" + bytes([len(pw)]) + pw.encode())
    assert s.recv(2)[1] == 0, "пароль не принят"
    s.sendall(b"\x05\x01\x00\x01" + socket.inet_aton(host) + struct.pack(">H", PORT))
    rep = b""
    while len(rep) < 10:
        d = s.recv(10 - len(rep))
        if not d:
            raise OSError("closed")
        rep += d
    assert rep[1] == 0, f"socks error {rep[1]}"
    s.sendall(b"x")
    got = b""
    while len(got) < 4096:
        d = s.recv(4096 - len(got))
        if not d:
            break
        got += d
    s.close()
    return got == b"PONG" * 1024


def main():
    binary = os.path.abspath(sys.argv[1]) if len(sys.argv) > 1 else os.path.abspath("target/release/yandi")
    out_dir = sys.argv[2] if len(sys.argv) > 2 else "/run/proxy_restart"
    LAB.reexec_in_sandbox()
    lab = LAB.Lab()
    lab.base()
    os.makedirs(out_dir, exist_ok=True)
    nets = {k: lab.public_node(k) for k in (1, 3)}
    lab.router(1, "full")
    nets[2] = lab.natted_node(2, 1)
    threading.Thread(target=server, daemon=True).start()
    nodes = [C.Node(k, out_dir, binary, client=(k == 2), anchor=(k == 1), net=nets[k]) for k in (1, 2, 3)]
    X, Cl, E = nodes
    for n in nodes:
        n.start()
    for n in nodes:
        assert n.login(120) and n.ready(120), n.k
    for n in nodes:
        for m in nodes:
            if m is not n:
                n.trust(m)
    time.sleep(30)
    internet = f"{LAB.INTERNET_NET}.0.1"
    ns = lab.nspath(2)
    results = []

    def start():
        for _ in range(40):
            st, r = Cl.api("POST", f"/api/socks5/start/{X.id[:16]}")
            if st == 200 and r.get("status") == "success":
                return r
            time.sleep(3)
        raise AssertionError(f"start: {st} {r}")

    def port_open(port):
        try:
            LAB.ns_socket(ns, "127.0.0.1", port, 3).close()
            return True
        except Exception:
            return False

    def check(name, ok):
        results.append((name, bool(ok)))
        print(json.dumps({"case": name, "ok": bool(ok)}, ensure_ascii=False), flush=True)

    r = start()
    port, pw = r["local_port"], r["password"]
    ok = False
    last = ''
    for _ in range(30):
        try:
            ok = socks_ping(ns, port, pw, internet)
            break
        except Exception as e:
            last = repr(e)
            time.sleep(1)
    if not ok:
        print("последняя ошибка:", last, flush=True)
    check("первый запуск: данные идут", ok)
    st, r2 = Cl.api("POST", f"/api/socks5/stop/{X.id[:16]}")
    check("остановка принята", st == 200 and r2.get("status") == "success")
    time.sleep(1)
    check("после остановки порт закрыт", not port_open(port))
    r = start()
    port2, pw2 = r["local_port"], r["password"]
    ok = False
    for _ in range(30):
        try:
            ok = socks_ping(ns, port2, pw2, internet)
            break
        except Exception:
            time.sleep(1)
    check("повторный запуск без перезапуска узла: данные идут", ok)
    for n in nodes:
        n.kill()
    bad = [n for n, ok in results if not ok]
    print("ИТОГ:", "ВСЕ ПРОВЕРКИ ПРОШЛИ" if not bad else f"ПРОВАЛЕНО: {bad}", flush=True)
    sys.exit(1 if bad else 0)


main()
