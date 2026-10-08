#!/usr/bin/env python3
"""Throughput of the exit path in the rootless lab: client node --(SOCKS5 proxy of the node)--> exit node --> "internet" host in the sandbox.
Reports MB/s and Mbit/s for downloads and uploads of one stream and of several parallel streams, and a baseline without the nodes (the same
transfer straight over the lab network), so a limit of the lab or of the link is not mistaken for a limit of the node.
Usage: python3 chaos/speedtest.py [delay_ms=0] [rate=] [megabytes=64] [binary] [out]      e.g.  speedtest.py 20 300mbit 64
"""
import json, os, socket, struct, sys, threading, time

sys.path.insert(0, os.path.dirname(__file__))
import chaos as C
import lab as LAB

PORT = 8099


def server():
    s = socket.socket()
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("0.0.0.0", PORT))
    s.listen(64)
    chunk = os.urandom(1 << 20)

    def serve(c):
        try:
            hdr = c.recv(9)  # b"D"+u64 megabytes to send, or b"U"+u64 megabytes expected
            mode, mb = hdr[:1], struct.unpack(">Q", hdr[1:9])[0]
            if mode == b"D":
                for _ in range(mb):
                    c.sendall(chunk)
            else:
                got, need = 0, mb << 20
                while got < need:
                    d = c.recv(1 << 20)
                    if not d:
                        break
                    got += len(d)
                c.sendall(b"K")
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


def transfer(opener, mode, mb):
    s = opener()
    s.settimeout(120)
    s.sendall(mode + struct.pack(">Q", mb))
    t = time.time()
    if mode == b"D":
        got, need = 0, mb << 20
        while got < need:
            d = s.recv(1 << 20)
            if not d:
                break
            got += len(d)
    else:
        blob = os.urandom(1 << 20)
        for _ in range(mb):
            s.sendall(blob)
        s.recv(1)
    dt = time.time() - t
    s.close()
    return dt


def cpu_seconds(pid):
    try:
        f = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
        return (int(f[11]) + int(f[12])) / os.sysconf("SC_CLK_TCK")
    except OSError:
        return 0.0


NODES = []


def measure(label, opener, mb, parallel, out):
    for mode, name in ((b"D", "download"), (b"U", "upload")):
        res = []

        def one():
            res.append(transfer(opener, mode, mb))

        t = time.time()
        c0 = {n.k: cpu_seconds(n.proc.pid) for n in NODES}
        ths = [threading.Thread(target=one) for _ in range(parallel)]
        [x.start() for x in ths]; [x.join() for x in ths]
        wall = time.time() - t
        cpu = {n.k: round((cpu_seconds(n.proc.pid) - c0[n.k]) / wall * 100) for n in NODES}
        total = mb * parallel
        r = {"path": label, "dir": name, "streams": parallel, "MB": total, "seconds": round(wall, 2), "MB_per_s": round(total / wall, 1), "Mbit_per_s": round(total * 8 / wall), "cpu_percent_of_one_core": cpu}
        out.append(r)
        print(json.dumps(r), flush=True)


def main():
    delay = int(sys.argv[1]) if len(sys.argv) > 1 else 0
    rate = sys.argv[2] if len(sys.argv) > 2 and sys.argv[2] else None
    mb = int(sys.argv[3]) if len(sys.argv) > 3 else 64
    binary = os.path.abspath(sys.argv[4]) if len(sys.argv) > 4 else os.path.abspath("target/release/yandi")
    out_dir = sys.argv[5] if len(sys.argv) > 5 else "/tmp/speedtest"
    os.makedirs(out_dir, exist_ok=True)
    LAB.reexec_in_sandbox()
    lab = LAB.Lab()
    lab.base()
    # the client sits behind a port-preserving NAT: a node with a public address is an 'anchor' and keeps the proxy channels for its exit server role
    nets = {k: lab.public_node(k) for k in (1, 3)}
    lab.router(1, 'full')
    nets[2] = lab.natted_node(2, 1)
    for k in nets:
        if delay or rate:
            lab.netem(k, delay, 0, 0.0, 0.0, rate)
    threading.Thread(target=server, daemon=True).start()
    # the client must be a client-only node: a public node keeps the proxy channels for its own exit server role
    nodes = [C.Node(k, out_dir, binary, client=(k == 2), anchor=(k == 1), net=nets[k]) for k in (1, 2, 3)]
    X, Cl, E = nodes
    NODES.extend(nodes)
    for n in nodes:
        n.start()
    for n in nodes:
        assert n.login(120) and n.ready(120), n.k
    for n in nodes:
        for m in nodes:
            if m is not n:
                n.trust(m)
    time.sleep(30)
    # the node's own background probes may hold the proxy channels for a moment: try again
    for _ in range(40):
        st, r = Cl.api("POST", f"/api/socks5/start/{X.id[:16]}")
        if st == 200 and r.get("status") == "success":
            break
        time.sleep(3)
    assert st == 200 and r.get("status") == "success", (st, r)
    port, pw = r["local_port"], r["password"]
    ns = LAB.NsConnection  # noqa
    internet = f"{LAB.INTERNET_NET}.0.1"
    results = []

    def via_node():
        s = LAB.ns_socket(Cl_ns, "127.0.0.1", port, 20)
        socks_open(s, "yandi", pw, internet, PORT)
        return s

    def direct():
        return LAB.ns_socket(Cl_ns, internet, PORT, 20)

    Cl_ns = lab.nspath(2)
    # the proxy needs a moment to find its way through the exit
    for _ in range(30):
        try:
            s = via_node(); s.close(); break
        except Exception:
            time.sleep(1)
    print(f"--- lab link: delay {delay} ms each way per node, rate {rate or 'unlimited'}; {mb} MB per stream", flush=True)
    measure("direct (no node)", direct, mb, 1, results)
    measure("through the exit", via_node, mb, 1, results)
    measure("through the exit", via_node, mb, 4, results)
    json.dump({"delay_ms": delay, "rate": rate, "results": results}, open(os.path.join(out_dir, "result.json"), "w"), indent=1)
    for n in nodes:
        n.kill()


main()
