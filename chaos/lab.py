#!/usr/bin/env python3
"""Network lab for the chaos test: virtual networks with real NAT, delays and packet loss, built from Linux network namespaces.

Everything lives inside ONE user+network+mount namespace that this program creates for itself (`unshare -Urnm`): no root is needed, the
real network of the machine is never touched, and when the program ends the kernel removes everything. The "internet" is a bridge
(11.77.0.0/16, an address range that exists nowhere inside the sandbox but looks public to the node's own address checks);
nodes are either connected to it directly (public nodes) or sit behind a small router with a private LAN (192.168.G.0/24) and NAT of a
chosen kind:
    full      - endpoint-independent mapping, any outside host that was contacted may answer (port preserved when possible)
    symmetric - a different outside port for every destination (hole punching cannot work)
The program reaches the web page of a node inside its namespace through setns(), so no port is exposed anywhere.
"""
import ctypes, http.client, json, os, socket, subprocess, threading

CLONE_NEWNET = 0x40000000
_libc = ctypes.CDLL(None, use_errno=True)

IP = "/usr/sbin/ip" if os.path.exists("/usr/sbin/ip") else "ip"
NFT = "/usr/sbin/nft"
TC = "/sbin/tc" if os.path.exists("/sbin/tc") else "/usr/sbin/tc"
INTERNET_NET = "11.77"


def sh(cmd, check=True):
    r = subprocess.run(cmd, shell=isinstance(cmd, str), capture_output=True, text=True)
    if check and r.returncode != 0:
        raise RuntimeError(f"{cmd}: {r.stderr.strip()}")
    return r


def in_sandbox():
    return os.environ.get("CHAOS_SANDBOX") == "1"


def reexec_in_sandbox():
    """Re-run the current program inside a fresh user+net+mount namespace (once)."""
    if in_sandbox():
        return
    env = dict(os.environ, CHAOS_SANDBOX="1")
    import sys
    os.execvpe("unshare", ["unshare", "-Urnm", "--", sys.executable] + sys.argv, env)


def ns_socket(nspath, host, port, timeout=10):
    """Open a TCP connection from inside the network namespace `nspath` (a thread enters it, connects, and goes back)."""
    out = {}

    def work():
        cur = os.open("/proc/thread-self/ns/net", os.O_RDONLY)
        target = os.open(nspath, os.O_RDONLY)
        try:
            if _libc.setns(target, CLONE_NEWNET) != 0:
                raise OSError(ctypes.get_errno(), "setns " + nspath)
            out["s"] = socket.create_connection((host, port), timeout)
        except Exception as e:
            out["e"] = e
        finally:
            _libc.setns(cur, CLONE_NEWNET)
            os.close(target)
            os.close(cur)

    t = threading.Thread(target=work)
    t.start()
    t.join()
    if "e" in out:
        raise out["e"]
    return out["s"]


class NsConnection(http.client.HTTPConnection):
    def __init__(self, nspath, host, port, timeout):
        super().__init__(host, port, timeout=timeout)
        self.nspath = nspath

    def connect(self):
        self.sock = ns_socket(self.nspath, self.host, self.port, self.timeout)


def ns_http(nspath, method, port, path, body=None, cookie=None, timeout=15):
    conn = NsConnection(nspath, "127.0.0.1", port, timeout)
    try:
        headers = {"Host": f"127.0.0.1:{port}"}
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        if cookie:
            headers["Cookie"] = cookie
        conn.request(method, path, body=data, headers=headers)
        r = conn.getresponse()
        raw = r.read()
        try:
            parsed = json.loads(raw)
        except Exception:
            parsed = raw
        return r.status, parsed, r.msg
    except Exception as e:
        return 0, str(e), {}
    finally:
        conn.close()


class Lab:
    def __init__(self, log=print):
        self.log = log
        self.nets = {}      # node k -> dict(ns, ip, group, kind)
        self.routers = {}   # group -> dict(ns, wan_ip, kind)
        self.up_ready = False

    # ---- construction
    def base(self):
        sh("mount -t tmpfs tmpfs /run")
        sh("mkdir -p /run/netns")
        sh([IP, "link", "set", "lo", "up"])
        sh([IP, "link", "add", "br0", "type", "bridge"])
        sh([IP, "addr", "add", f"{INTERNET_NET}.0.1/16", "dev", "br0"])
        sh([IP, "link", "set", "br0", "up"])
        # a host on the "internet" that answers (for proxy/exit tests): the sandbox itself, address 11.77.0.1
        self.up_ready = True

    def _ns(self, name):
        sh([IP, "netns", "add", name])
        sh([IP, "-n", name, "link", "set", "lo", "up"])

    def public_node(self, k):
        ns = f"n{k}"
        ip = f"{INTERNET_NET}.{1 + k // 200}.{10 + k % 200}"
        self._ns(ns)
        host, peer = f"vp{k}", f"ve{k}"
        sh([IP, "link", "add", host, "type", "veth", "peer", "name", peer])
        sh([IP, "link", "set", peer, "netns", ns])
        sh([IP, "-n", ns, "link", "set", peer, "name", "e0"])
        sh([IP, "-n", ns, "addr", "add", f"{ip}/16", "dev", "e0"])
        sh([IP, "-n", ns, "link", "set", "e0", "up"])
        sh([IP, "link", "set", host, "master", "br0"])
        sh([IP, "link", "set", host, "up"])
        self.nets[k] = {"ns": ns, "ip": ip, "group": None, "kind": "public"}
        return self.nets[k]

    def router(self, g, kind):
        ns = f"r{g}"
        wan_ip = f"{INTERNET_NET}.254.{g}"
        self._ns(ns)
        host, peer = f"vr{g}", f"vw{g}"
        sh([IP, "link", "add", host, "type", "veth", "peer", "name", peer])
        sh([IP, "link", "set", peer, "netns", ns])
        sh([IP, "-n", ns, "link", "set", peer, "name", "w0"])
        sh([IP, "-n", ns, "addr", "add", f"{wan_ip}/16", "dev", "w0"])
        sh([IP, "-n", ns, "link", "set", "w0", "up"])
        sh([IP, "link", "set", host, "master", "br0"])
        sh([IP, "link", "set", host, "up"])
        sh([IP, "-n", ns, "link", "add", "lan", "type", "bridge"])
        sh([IP, "-n", ns, "addr", "add", f"192.168.{g}.1/24", "dev", "lan"])
        sh([IP, "-n", ns, "link", "set", "lan", "up"])
        sh(["ip", "netns", "exec", ns, "/sbin/sysctl", "-qw", "net.ipv4.ip_forward=1"])
        # NAT: nothing from outside gets in unless an inside host spoke first; "symmetric" randomises the outside port per destination
        masq = "masquerade" if kind == "full" else "masquerade random,fully-random"
        rules = f"""
table ip nat {{
  chain post {{
    type nat hook postrouting priority 100;
    oifname "w0" {masq}
  }}
}}
table ip filter {{
  chain forwarding {{
    type filter hook forward priority 0; policy drop;
    iifname "lan" accept
    ct state established,related accept
  }}
}}
"""
        r = subprocess.run(["ip", "netns", "exec", ns, NFT, "-f", "-"], input=rules, text=True, capture_output=True)
        if r.returncode != 0:
            raise RuntimeError("nft: " + r.stderr)
        self.routers[g] = {"ns": ns, "wan_ip": wan_ip, "kind": kind, "count": 0}
        return self.routers[g]

    def natted_node(self, k, g):
        r = self.routers[g]
        r["count"] += 1
        ns = f"n{k}"
        ip = f"192.168.{g}.{10 + r['count']}"
        self._ns(ns)
        host, peer = f"vl{k}", f"ve{k}"
        sh([IP, "link", "add", host, "type", "veth", "peer", "name", peer])
        sh([IP, "link", "set", peer, "netns", ns])
        sh([IP, "-n", ns, "link", "set", peer, "name", "e0"])
        sh([IP, "-n", ns, "addr", "add", f"{ip}/24", "dev", "e0"])
        sh([IP, "-n", ns, "link", "set", "e0", "up"])
        sh([IP, "-n", ns, "route", "add", "default", "via", f"192.168.{g}.1"])
        sh([IP, "link", "set", host, "netns", r["ns"]])
        sh([IP, "-n", r["ns"], "link", "set", host, "master", "lan"])
        sh([IP, "-n", r["ns"], "link", "set", host, "up"])
        self.nets[k] = {"ns": ns, "ip": ip, "group": g, "kind": r["kind"]}
        return self.nets[k]

    # ---- network conditions
    def netem(self, k, delay_ms=0, jitter_ms=0, loss_pct=0.0, reorder_pct=0.0, rate=None):
        ns = self.nets[k]["ns"]
        args = ["ip", "netns", "exec", ns, TC, "qdisc", "replace", "dev", "e0", "root", "netem"]
        if delay_ms:
            args += ["delay", f"{delay_ms}ms"] + ([f"{jitter_ms}ms"] if jitter_ms else [])
        if loss_pct:
            args += ["loss", f"{loss_pct}%"]
        if reorder_pct:
            args += ["reorder", f"{reorder_pct}%"]
        if rate:
            args += ["rate", rate]
        if len(args) == 11:  # no impairment requested: remove it
            sh(["ip", "netns", "exec", ns, TC, "qdisc", "del", "dev", "e0", "root"], check=False)
        else:
            sh(args)

    def link(self, k, up):
        """Cut or restore the network cable of node k (the process keeps running: a pure network outage)."""
        sh([IP, "-n", self.nets[k]["ns"], "link", "set", "e0", "up" if up else "down"])
        n = self.nets[k]
        if up and n["group"] is not None:
            # the kernel drops the routes of an interface that goes down; a home router's address comes back with the cable, so the way out must too
            sh([IP, "-n", n["ns"], "route", "replace", "default", "via", f"192.168.{n['group']}.1"])

    def nspath(self, k):
        return f"/run/netns/{self.nets[k]['ns']}"
