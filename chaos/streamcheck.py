#!/usr/bin/env python3
"""Throughput of the exit path in the rootless lab: client node --(SOCKS5 proxy of the node)--> exit node --> "internet" host in the sandbox.
Reports MB/s and Mbit/s for downloads and uploads of one stream and of several parallel streams, and a baseline without the nodes (the same
transfer straight over the lab network), so a limit of the lab or of the link is not mistaken for a limit of the node.
Usage: [LOSS=percent] python3 chaos/speedtest.py [delay_ms=0] [rate=] [megabytes=64] [binary] [out]      e.g.  LOSS=1 speedtest.py 20 300mbit 64
"""
import hashlib, json, os, socket, struct, sys, threading, time

sys.path.insert(0, os.path.dirname(__file__))
import chaos as C
import lab as LAB

PORT = 8099
BLOCK = 1 << 20


def gen_block(seed, i):
    """Детерминированный 1 МиБ: у каждого блока свой отпечаток, перестановка и пропуск блоков меняют хэш потока."""
    d = hashlib.sha256(seed + i.to_bytes(8, "big")).digest()
    return (d * (BLOCK // 32))


def stream_hash(seed, mb):
    h = hashlib.sha256()
    for i in range(mb):
        h.update(gen_block(seed, i))
    return h.digest()


def server():
    s = socket.socket()
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("0.0.0.0", PORT))
    s.listen(64)

    def serve(c):
        try:
            hdr = b""
            while len(hdr) < 17:
                d = c.recv(17 - len(hdr))
                if not d:
                    return
                hdr += d
            mode, mb, seed8 = hdr[:1], struct.unpack(">Q", hdr[1:9])[0], hdr[9:17]
            seed = seed8
            if mode == b"D":      # скачивание: сервер шлёт детерминированный поток
                for i in range(mb):
                    c.sendall(gen_block(seed, i))
            elif mode == b"U":    # отдача: сервер считает хэш принятого и возвращает 32 байта
                h = hashlib.sha256()
                got, need = 0, mb * BLOCK
                while got < need:
                    d = c.recv(1 << 20)
                    if not d:
                        break
                    h.update(d)
                    got += len(d)
                c.sendall(h.digest() + struct.pack(">Q", got))
            elif mode == b"N":    # медленный потребитель: принимает соединение и ничего не читает
                time.sleep(3600)
        except OSError:
            pass
        finally:
            c.close()

    while True:
        c, _ = s.accept()
        threading.Thread(target=serve, args=(c,), daemon=True).start()


def socks_open(sock, user, pw, host, port):
    sock.sendall(b"\x05\x01\x02")
    assert sock.recv(2) == b"\x05\x02"
    sock.sendall(b"\x01" + bytes([len(user)]) + user.encode() + bytes([len(pw)]) + pw.encode())
    assert sock.recv(2)[1] == 0, "proxy refused the password"
    sock.sendall(b"\x05\x01\x00\x01" + socket.inet_aton(host) + struct.pack(">H", port))
    rep = b""
    while len(rep) < 10:
        d = sock.recv(10 - len(rep))
        if not d:
            raise OSError("closed")
        rep += d
    if rep[1] != 0:
        raise OSError(f"socks error {rep[1]}")


def checked_transfer(opener, mode, mb, timeout=90):
    """Возвращает (ok, секунд, пояснение). ok только если хэш потока совпал."""
    seed = os.urandom(8)
    s = opener()
    s.settimeout(timeout)
    s.sendall(mode + struct.pack(">Q", mb) + seed)
    t = time.time()
    try:
        if mode == b"D":
            h = hashlib.sha256()
            got, need = 0, mb * BLOCK
            while got < need:
                d = s.recv(1 << 20)
                if not d:
                    break
                h.update(d)
                got += len(d)
            ok = got == need and h.digest() == stream_hash(seed, mb)
            note = f"получено {got}/{need}"
        else:
            for i in range(mb):
                s.sendall(gen_block(seed, i))
            ans = b""
            while len(ans) < 40:
                d = s.recv(40 - len(ans))
                if not d:
                    break
                ans += d
            ok = len(ans) == 40 and ans[:32] == stream_hash(seed, mb)
            note = "хэш на сервере " + ("совпал" if ok else "НЕ совпал / нет ответа")
    except OSError as e:
        ok, note = False, f"ошибка: {e!r}"
    finally:
        s.close()
    return ok, round(time.time() - t, 2), note


def slow_consumer_case(opener, mb_ok, mb_stuck, timeout=90):
    """Поток N не читается получателем; параллельный нормальный поток должен дойти с верным хэшем."""
    stuck = {"done": False}

    def feed():
        seed = os.urandom(8)
        try:
            s = opener()
            s.settimeout(timeout)
            s.sendall(b"N" + struct.pack(">Q", mb_stuck) + seed)
            for i in range(mb_stuck):
                s.sendall(gen_block(seed, i))
            stuck["done"] = True
        except OSError:
            pass

    th = threading.Thread(target=feed, daemon=True)
    th.start()
    time.sleep(3)
    res = checked_transfer(opener, b"D", mb_ok, timeout)
    res2 = checked_transfer(opener, b"U", mb_ok, timeout)
    return res, res2


def main():
    delay = int(sys.argv[1]) if len(sys.argv) > 1 else 0
    mb = int(sys.argv[2]) if len(sys.argv) > 2 else 32
    binary = os.path.abspath(sys.argv[3]) if len(sys.argv) > 3 else os.path.abspath("target/release/yandi")
    out_dir = sys.argv[4] if len(sys.argv) > 4 else "/tmp/streamcheck"
    par = int(os.environ.get("PAR", "4"))
    os.makedirs(out_dir, exist_ok=True)
    LAB.reexec_in_sandbox()
    lab = LAB.Lab()
    lab.base()
    nets = {k: lab.public_node(k) for k in (1, 3)}
    lab.router(1, 'full')
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
    for _ in range(40):
        st, r = Cl.api("POST", f"/api/socks5/start/{X.id[:16]}")
        if st == 200 and r.get("status") == "success":
            break
        time.sleep(3)
    assert st == 200 and r.get("status") == "success", (st, r)
    port, pw = r["local_port"], r["password"]
    internet = f"{LAB.INTERNET_NET}.0.1"
    Cl_ns = lab.nspath(2)

    def via_node():
        s = LAB.ns_socket(Cl_ns, "127.0.0.1", port, 20)
        socks_open(s, "yandi", pw, internet, PORT)
        return s

    for _ in range(30):
        try:
            via_node().close()
            break
        except Exception:
            time.sleep(1)
    results = []

    def rec(name, ok, secs, note):
        r = {"case": name, "ok": ok, "seconds": secs, "note": note}
        results.append(r)
        print(json.dumps(r, ensure_ascii=False), flush=True)

    for mode, label in ((b"D", "download"), (b"U", "upload")):
        ok, secs, note = checked_transfer(via_node, mode, mb)
        rec(f"{label} 1 поток {mb} МБ", ok, secs, note)
    # параллельные потоки: проверка порядка и целостности при нескольких туннелях сразу
    for mode, label in ((b"D", "download"), (b"U", "upload")):
        out = []

        def one():
            out.append(checked_transfer(via_node, mode, mb))

        ths = [threading.Thread(target=one) for _ in range(par)]
        [t.start() for t in ths]
        [t.join() for t in ths]
        rec(f"{label} {par} потока по {mb} МБ", all(o[0] for o in out), max(o[1] for o in out), "; ".join(o[2] for o in out))
    (d_ok, d_secs, d_note), (u_ok, u_secs, u_note) = slow_consumer_case(via_node, max(8, mb // 2), 256)
    rec("медленный потребитель: скачивание параллельно", d_ok, d_secs, d_note)
    rec("медленный потребитель: отдача параллельно", u_ok, u_secs, u_note)
    json.dump(results, open(os.path.join(out_dir, "result.json"), "w"), ensure_ascii=False, indent=1)
    bad = [r for r in results if not r["ok"]]
    print("ИТОГ:", "ВСЕ ПРОВЕРКИ ПРОШЛИ" if not bad else f"ПРОВАЛЕНО: {len(bad)} из {len(results)}", flush=True)
    for n in nodes:
        n.kill()
    sys.exit(1 if bad else 0)


try:
    main()
finally:
    pass
