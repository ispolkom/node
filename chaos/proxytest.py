#!/usr/bin/env python3
"""Stage J check in the rootless lab: an exit node that is NOT allowed to reach the target directly (its firewall drops it and counts the attempts)
reaches it through an outside SOCKS5 proxy with a password, set on the node's settings page (API). Checks:
  1. a name that only the proxy can resolve is reached through the exit      (the name went to the proxy, not to this machine's resolver)
  2. the firewall counter of direct attempts to the target stays 0           (nothing went around the proxy)
  3. the proxy saw the login and the name                                      (it was used)
  4. the settings API never returns the password; the node's log never contains it
  5. the proxy is stopped: the exit CLOSES (connection fails), the counter stays 0   (fail closed)
  6. the proxy is switched off in the settings and the lab target is still blocked: the exit fails the other way (no proxy) and the counter counts the direct try
Usage: python3 chaos/proxytest.py [binary] [out]
"""
import json, os, socket, struct, subprocess, sys, threading, time

sys.path.insert(0, os.path.dirname(__file__))
import chaos as C
import lab as LAB

TARGET_IP, TARGET_PORT = f"{LAB.INTERNET_NET}.0.1", 8099
PROXY_PORT = 1080
USER, PASSWORD = "buyer42", "S3cr3t-Pass-for-proxy"
NAME = "target.lab"  # exists only inside the proxy's own table
seen = {"logins": [], "targets": []}
proxy_sock = None


def pump(a, b):
    try:
        while True:
            d = a.recv(65536)
            if not d:
                break
            b.sendall(d)
    except OSError:
        pass
    finally:
        for s in (a, b):
            try: s.shutdown(socket.SHUT_RDWR)
            except OSError: pass


def socks_server():
    global proxy_sock
    proxy_sock = socket.socket()
    proxy_sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    proxy_sock.bind(("0.0.0.0", PROXY_PORT))
    proxy_sock.listen(16)

    def handle(c):
        try:
            g = c.recv(2); c.recv(g[1])
            c.sendall(b"\x05\x02")
            h = c.recv(2); user = c.recv(h[1]); pl = c.recv(1); pw = c.recv(pl[0])
            ok = user.decode() == USER and pw.decode() == PASSWORD
            c.sendall(b"\x01" + (b"\x00" if ok else b"\x01"))
            if not ok:
                return
            seen["logins"].append(user.decode())
            r = c.recv(4)
            if r[3] == 3:
                n = c.recv(1)[0]; host = c.recv(n).decode()
            elif r[3] == 1:
                host = socket.inet_ntoa(c.recv(4))
            else:
                return
            port = struct.unpack(">H", c.recv(2))[0]
            seen["targets"].append((host, port))
            real = TARGET_IP if host in (NAME, "1.1.1.1") else host
            if host == "1.1.1.1":
                port = TARGET_PORT  # the lab has no real internet: the "outside" host of the test button is the lab target
            try:
                up = socket.create_connection((real, port), 5)
            except OSError:
                c.sendall(b"\x05\x04\x00\x01\x00\x00\x00\x00\x00\x00")
                return
            c.sendall(b"\x05\x00\x00\x01\x00\x00\x00\x00\x00\x00")
            threading.Thread(target=pump, args=(up, c), daemon=True).start()
            pump(c, up)
        except Exception:
            pass
        finally:
            c.close()

    while True:
        try:
            c, _ = proxy_sock.accept()
        except OSError:
            return
        threading.Thread(target=handle, args=(c,), daemon=True).start()


def target_server():
    s = socket.socket()
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("0.0.0.0", TARGET_PORT)); s.listen(16)
    def serve(c):
        try:
            c.sendall(b"hello from the target")
            c.recv(1)
        except OSError:
            pass
        finally:
            c.close()
    while True:
        c, _ = s.accept()
        threading.Thread(target=serve, args=(c,), daemon=True).start()


def socks_via_node(ns, port, pw, host, dport):
    s = LAB.ns_socket(ns, "127.0.0.1", port, 15)
    s.settimeout(20)
    s.sendall(b"\x05\x01\x02")
    assert s.recv(2) == b"\x05\x02"
    s.sendall(b"\x01" + bytes([5]) + b"yandi" + bytes([len(pw)]) + pw.encode())
    assert s.recv(2)[1] == 0
    h = host.encode()
    s.sendall(b"\x05\x01\x00\x03" + bytes([len(h)]) + h + struct.pack(">H", dport))
    rep = b""
    while len(rep) < 10:
        d = s.recv(10 - len(rep))
        if not d: raise OSError("closed")
        rep += d
    if rep[1] != 0:
        raise OSError(f"socks error {rep[1]}")
    return s


def nft(ns, args, inp=None):
    return subprocess.run(["ip", "netns", "exec", ns, "/usr/sbin/nft"] + args, input=inp, text=True, capture_output=True)


def leak_counter(ns):
    out = nft(ns, ["list", "table", "inet", "leak"]).stdout
    for l in out.splitlines():
        if "counter packets" in l:
            return int(l.split("packets")[1].split()[0])
    return -1


def main():
    binary = os.path.abspath(sys.argv[1]) if len(sys.argv) > 1 else os.path.abspath("target/release/yandi")
    out = sys.argv[2] if len(sys.argv) > 2 else "/tmp/proxytest"
    os.makedirs(out, exist_ok=True)
    LAB.reexec_in_sandbox()
    lab = LAB.Lab(); lab.base()
    nets = {k: lab.public_node(k) for k in (1, 3)}
    lab.router(1, "full")
    nets[2] = lab.natted_node(2, 1)
    threading.Thread(target=socks_server, daemon=True).start()
    threading.Thread(target=target_server, daemon=True).start()
    nodes = [C.Node(k, out, binary, anchor=(k == 1), net=nets[k]) for k in (1, 2, 3)]
    X, Cl, E = nodes
    for n in nodes: n.start()
    for n in nodes: assert n.login(120) and n.ready(120), n.k
    for n in nodes:
        for m in nodes:
            if m is not n: n.trust(m)
    results = {}
    def check(name, ok, detail=""):
        results[name] = bool(ok)
        print(("PASS " if ok else "FAIL ") + name + (f" — {detail}" if detail else ""), flush=True)
    # the exit node's own firewall: it may NOT go to the target directly; every attempt is counted and dropped
    r = nft("n1", ["-f", "-"], f"table inet leak {{\n  chain out {{\n    type filter hook output priority 0; policy accept;\n    ip daddr {TARGET_IP} tcp dport {TARGET_PORT} counter drop\n  }}\n}}\n")
    assert r.returncode == 0, r.stderr
    time.sleep(25)
    # the settings, through the page's API
    st, resp = X.api("POST", "/api/upstream-proxy", {"enabled": True, "kind": "socks5", "host": TARGET_IP, "port": PROXY_PORT, "auth": "password", "user": USER, "password": PASSWORD})
    check("settings are accepted", st == 200 and resp.get("ok"), str(resp))
    st, view = X.api("GET", "/api/upstream-proxy")
    check("the API never returns the password", PASSWORD not in json.dumps(view) and view.get("has_password") is True and view.get("user") == USER, json.dumps(view))
    st, t = X.api("POST", "/api/upstream-proxy/test")
    check("the test button reaches the outside through the proxy", st == 200 and t.get("ok"), str(t))
    # the client opens its proxy through the exit
    for _ in range(40):
        st, r = Cl.api("POST", f"/api/socks5/start/{X.id[:16]}")
        if st == 200 and r.get("status") == "success": break
        time.sleep(3)
    port, pw = r["local_port"], r["password"]
    ns = lab.nspath(2)
    ok = False
    for _ in range(20):
        try:
            s = socks_via_node(ns, port, pw, NAME, TARGET_PORT)
            data = s.recv(100)
            ok = data == b"hello from the target"
            s.close(); break
        except Exception as e:
            err = str(e); time.sleep(2)
    check("a name only the proxy knows is reached through the exit", ok)
    check("the proxy saw the login and the NAME", USER in seen["logins"] and (NAME, TARGET_PORT) in seen["targets"], str(seen))
    check("nothing went around the proxy (firewall counter of direct attempts)", leak_counter("n1") == 0, f"counter={leak_counter('n1')}")
    # the proxy dies: fail closed
    try:
        proxy_sock.shutdown(socket.SHUT_RDWR)  # a plain close() would leave the blocked accept() serving
    except OSError:
        pass
    proxy_sock.close()
    time.sleep(1)
    failed = False
    try:
        s = socks_via_node(ns, port, pw, NAME, TARGET_PORT)
        failed = s.recv(100) != b"hello from the target"
    except Exception:
        failed = True
    check("with the proxy down the exit closes (no way out)", failed)
    check("and still nothing went around it", leak_counter("n1") == 0, f"counter={leak_counter('n1')}")
    # switched off in the settings: the exit tries the network path again, which this node's firewall forbids
    st, resp = X.api("POST", "/api/upstream-proxy", {"enabled": False, "kind": "socks5", "host": TARGET_IP, "port": PROXY_PORT, "auth": "password", "user": USER, "password": ""})
    check("switching off keeps the saved password", st == 200 and resp.get("ok"), str(resp))
    st, view = X.api("GET", "/api/upstream-proxy")
    check("the switch is visible", view.get("enabled") is False and view.get("has_password") is True)
    try:
        s = socks_via_node(ns, port, pw, TARGET_IP, TARGET_PORT)
        try: d = s.recv(100)
        except Exception: d = b""
        direct_ok = d == b"hello from the target"
    except Exception:
        direct_ok = False
    check("proxy off: the exit uses the direct way (which the lab firewall counts and drops)", (not direct_ok) and leak_counter("n1") >= 1, f"counter={leak_counter('n1')}")
    log = open(os.path.join(out, "node1", "node.log"), errors="replace").read()
    check("the password is not in the node's log", PASSWORD not in log)
    json.dump(results, open(os.path.join(out, "result.json"), "w"), indent=1)
    print("ALL PASS" if all(results.values()) else "SOME FAILED", flush=True)
    for n in nodes: n.kill()


try:
    main()
finally:
    for _p in subprocess.run(['pgrep', '-x', 'yandi'], capture_output=True, text=True).stdout.split():
        try:
            if any(e.startswith(b'YANDI_CONFIG=') and b'/proxytest' in e for e in open(f'/proc/{_p}/environ', 'rb').read().split(b'\0')):
                os.kill(int(_p), 9)
        except OSError:
            pass
