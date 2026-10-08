#!/usr/bin/env python3
"""Chaos test for a network of real node processes.

Starts N nodes on this machine, lets them find each other through a few entry nodes, keeps chat traffic running, then kills and
restores nodes in waves (SIGKILL, SIGSTOP, partial and 95% outages, gradual and mass restore) and writes everything it sees to
log files plus a REPORT.md. Nothing needs to be watched: a hard time limit and a watchdog stop and clean up everything.

    python3 chaos/chaos.py --nodes 30 --binary target/release/yandi --out /tmp/chaos_run [--scenario full|quick]

Only the Python standard library is used. Stage 1 runs on the loopback interface (no root needed); the nodes behind a simulated NAT
are "client" nodes that talk through relays. A network-namespace layer with real NAT and packet loss is added by chaos/netns.sh.
"""
import argparse, base64, json, os, random, signal, subprocess, sys, threading, time, urllib.request, urllib.error
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import lab as L
import attackers as ATK

OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))
LOGIN = "chaos-login-password-1"
MASTER = "chaos master phrase of several words"


def now():
    return time.time()


class Log:
    def __init__(self, out):
        self.out = out
        self.lock = threading.Lock()
        self.ev = open(os.path.join(out, "events.jsonl"), "a", buffering=1)
        self.mt = open(os.path.join(out, "metrics.jsonl"), "a", buffering=1)

    def event(self, kind, **kw):
        rec = {"t": round(now(), 2), "kind": kind, **kw}
        with self.lock:
            self.ev.write(json.dumps(rec, ensure_ascii=False) + "\n")
        print(f"[{time.strftime('%H:%M:%S')}] {kind} " + " ".join(f"{k}={v}" for k, v in kw.items()), flush=True)

    def metric(self, **kw):
        with self.lock:
            self.mt.write(json.dumps({"t": round(now(), 2), **kw}, ensure_ascii=False) + "\n")


def http(method, url, body=None, cookie=None, timeout=15):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    if body is not None:
        req.add_header("Content-Type", "application/json")
    if cookie:
        req.add_header("Cookie", cookie)
    try:
        with OPENER.open(req, timeout=timeout) as r:
            raw = r.read()
            try:
                return r.status, json.loads(raw), r.headers
            except Exception:
                return r.status, raw, r.headers
    except urllib.error.HTTPError as e:
        return e.code, e.read(), e.headers
    except Exception as e:  # connection refused, timeout ...
        return 0, str(e), {}


class Node:
    def __init__(self, k, root, binary, base_port=26000, client=False, anchor=False, net=None):
        self.k = k
        self.dir = os.path.join(root, f"node{k}")
        self.binary = binary
        self.net = net  # lab mode: {"ns", "ip", "group", "kind"}; every node then uses the same ports (its own network)
        b = base_port + (100 if net else 100 * k)
        self.web, self.disc, self.data, self.p2pd, self.p2pdata, self.ws, self.httpp, self.gw, self.mp2p = b, b + 1, b + 2, b + 3, b + 4, b + 6, b + 7, b + 8, b + 9
        self.client = client
        self.anchor = anchor
        self.proc = None
        self.cookie = None
        self.card = None
        self.id = None
        self.frozen = False
        self.starts = 0
        self.friends = set()
        self.up_since = None

    # ---- process control
    def config(self):
        if self.net:
            return (f"server: {{bind_address: 0.0.0.0, log_level: info}}\n"
                    f"ports: {{discovery: {self.disc}, data: {self.data}, mobile_gateway: {self.gw}, mobile_p2p: {self.mp2p}, http_proxy: {self.httpp}, web_ui: {self.web}}}\n"
                    f"network: {{public_ip: {self.net['ip']}}}\nws: {{bind: \"127.0.0.1:{self.ws}\"}}\n")
        return (f"server: {{bind_address: 127.0.0.1, log_level: info}}\n"
                f"ports: {{discovery: {self.disc}, data: {self.data}, mobile_gateway: {self.gw}, mobile_p2p: {self.mp2p}, http_proxy: {self.httpp}, web_ui: {self.web}}}\n"
                f"network: {{public_ip: 127.0.0.1}}\nws: {{bind: \"127.0.0.1:{self.ws}\"}}\n")

    def env(self):
        d = self.dir
        e = dict(os.environ)
        for v in ("HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "ALL_PROXY", "all_proxy"):
            e.pop(v, None)
        e.update({
            "HOME": f"{d}/home", "XDG_DATA_HOME": f"{d}/home/.local/share", "XDG_CONFIG_HOME": f"{d}/home/.config",
            "YANDI_CONFIG": f"{d}/config.yaml", "YANDI_P2P_DISCOVERY_PORT": str(self.p2pd), "YANDI_P2P_DATA_PORT": str(self.p2pdata),
            "YANDI_CORE_BIN": "self", "NO_PROXY": "127.0.0.1,localhost,::1", "no_proxy": "127.0.0.1,localhost,::1",
        })
        if not self.net:
            e["YANDI_TESTNET"] = "1"  # on the loopback the node needs the relaxed address rules; in the lab it plays by the real ones
        if self.client:
            e["YANDI_CLIENT_ONLY"] = "1"
        return e

    def start(self):
        os.makedirs(os.path.join(self.dir, "home"), exist_ok=True)
        with open(os.path.join(self.dir, "config.yaml"), "w") as f:
            f.write(self.config())
        # CHAOS_NOLOG=1: the node's output goes nowhere (to see what the per-packet printing costs)
        log = subprocess.DEVNULL if os.environ.get("CHAOS_NOLOG") == "1" else open(os.path.join(self.dir, "node.log"), "ab")
        args = [self.binary] + (["--anchor"] if self.anchor else [])
        if self.net:
            args = ["ip", "netns", "exec", self.net["ns"]] + args
        self.proc = subprocess.Popen(args, cwd=self.dir, env=self.env(), stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
        self.starts += 1
        self.cookie = None
        self.frozen = False

    def alive(self):
        return self.proc is not None and self.proc.poll() is None

    def kill(self, sig=signal.SIGKILL):
        self.up_since = None
        if self.proc is not None and self.proc.poll() is None:
            try:
                os.killpg(self.proc.pid, sig)
            except ProcessLookupError:
                pass
            if sig == signal.SIGKILL:
                try:
                    self.proc.wait(timeout=10)
                except Exception:
                    pass
        self.cookie = None

    def freeze(self):
        if self.alive():
            self.up_since = None
            os.killpg(self.proc.pid, signal.SIGSTOP)
            self.frozen = True

    def thaw(self):
        if self.alive() and self.frozen:
            os.killpg(self.proc.pid, signal.SIGCONT)
            self.frozen = False

    # ---- web api
    def url(self, path):
        return f"http://127.0.0.1:{self.web}{path}"

    def _http(self, method, path, body=None, cookie=None, timeout=15):
        if self.net:
            return L.ns_http(f"/run/netns/{self.net['ns']}", method, self.web, path, body, cookie, timeout)
        return http(method, self.url(path), body, cookie, timeout)

    def login(self, timeout=90):
        first = not os.path.exists(os.path.join(self.dir, "home/.yandi_keys/auth.json"))
        end = now() + timeout
        while now() < end:
            if first:
                st, body, hd = self._http("POST", "/api/auth/setup", {"login_password": LOGIN, "login_password_repeat": LOGIN, "master_password": MASTER, "master_password_repeat": MASTER})
            else:
                st, body, hd = self._http("POST", "/api/auth/login", {"login_password": LOGIN})
            if st == 200:
                for h in hd.get_all("set-cookie") or []:
                    if h.startswith("yandi_session="):
                        self.cookie = h.split(";")[0]
                        return True
            time.sleep(0.5)
        return False

    def api(self, method, path, body=None, timeout=15):
        if not self.cookie:
            return 0, None
        st, b, _ = self._http(method, path, body, self.cookie, timeout)
        return st, b

    def ready(self, timeout=120):
        end = now() + timeout
        while now() < end:
            st, b = self.api("GET", "/api/peers/trusted")
            if st == 200 and isinstance(b, dict) and isinstance(b.get("card"), str):
                self.card = b["card"]
                self.up_since = now()
                raw = self.card.split(":", 1)[1]
                self.id = json.loads(base64.urlsafe_b64decode(raw + "=" * (-len(raw) % 4)))["id"]
                return True
            time.sleep(0.5)
        return False

    def trust(self, other):
        return self.api("POST", "/api/peers/trusted", {"card": other.card, "name": f"chaos {other.k}"})[0] == 200

    # ---- observation
    def rss_mb(self):
        if not self.alive():
            return None
        try:
            with open(f"/proc/{self.proc.pid}/statm") as f:
                return int(f.read().split()[1]) * os.sysconf("SC_PAGE_SIZE") / 1048576
        except Exception:
            return None

    def peers_online(self):
        st, b = self.api("GET", "/api/peers/trusted", timeout=8)
        if st == 200 and isinstance(b, dict):
            return sum(1 for p in b.get("peers", []) if p.get("online"))
        return None

    def connected(self):
        st, b = self.api("GET", "/api/nodes", timeout=8)
        if st == 200:
            if isinstance(b, dict):
                for key in ("nodes", "peers"):
                    if isinstance(b.get(key), list):
                        return len(b[key])
            if isinstance(b, list):
                return len(b)
        return None

    def directory(self):
        """(number of cards this node knows, number of those proved by a connect-back)"""
        st, b = self.api("GET", "/api/network/offers", timeout=8)
        if st == 200 and isinstance(b, dict):
            offers = b.get("offers") or []
            return len(offers), sum(1 for o in offers if o.get("verified"))
        return None

    def kernel(self):
        st, b = self.api("GET", "/api/kernel/status", timeout=8)
        return b if st == 200 and isinstance(b, (dict, list)) else None


class Chaos:
    def __init__(self, a):
        self.a = a
        self.out = os.path.abspath(a.out)
        os.makedirs(self.out, exist_ok=True)
        self.log = Log(self.out)
        self.rnd = random.Random(a.seed)
        self.deadline = now() + a.max_minutes * 60
        self.nodes = []
        self.stop = threading.Event()
        self.probes = []  # (t_sent, a, b, text, delivered_at or None)
        self.plock = threading.Lock()
        self.problems = []
        self.lab = None
        self.cut = []

    def problem(self, text):
        self.problems.append(text)
        self.log.event("PROBLEM", text=text)

    # ---- helpers
    def alive_nodes(self):
        return [n for n in self.nodes if n.alive() and not n.frozen]

    def up(self, nodes, spacing=0.3, login_timeout=120):
        def one(n):
            n.start()
            ok = n.login(login_timeout) and n.ready(login_timeout)
            return n, ok
        with ThreadPoolExecutor(max_workers=8) as ex:
            futs = []
            for n in nodes:
                futs.append(ex.submit(one, n))
                time.sleep(spacing)
            for f in futs:
                n, ok = f.result()
                if not ok:
                    self.problem(f"node {n.k} did not come up")

    def introduce(self, nodes, entries, per_node=2):
        for n in nodes:
            if n in entries:
                for e in entries:
                    if e is not n:
                        n.trust(e)
            else:
                for e in self.rnd.sample(entries, min(per_node, len(entries))):
                    n.trust(e)

    def make_friends(self, nodes, per_node=3):
        """Chat works between trusted contacts: every node adds a few others as contacts (both ways), like people do."""
        for n in nodes:
            for f in self.rnd.sample([x for x in nodes if x is not n], min(per_node, len(nodes) - 1)):
                if f in n.friends:
                    continue
                n.trust(f)
                f.trust(n)
                n.friends.add(f)
                f.friends.add(n)
        self.log.event("friends", pairs=sum(len(n.friends) for n in nodes) // 2)

    # ---- background: metrics and traffic
    def sampler(self):
        while not self.stop.wait(self.a.sample_every):
            alive = [n for n in self.nodes if n.alive() and not n.frozen and n.cookie]
            def sample(n):
                d = n.directory()
                return {"k": n.k, "rss": round(n.rss_mb() or 0, 1), "peers": n.peers_online(), "conn": n.connected(), "dir": d[0] if d else None, "dir_ok": d[1] if d else None}
            with ThreadPoolExecutor(max_workers=12) as ex:
                rows = list(ex.map(sample, alive))
            peers = sorted(r["dir"] for r in rows if r["dir"] is not None)
            rss = [r["rss"] for r in rows]
            self.log.metric(alive=len([n for n in self.nodes if n.alive()]), frozen=len([n for n in self.nodes if n.frozen]), total=len(self.nodes),
                            rss_total=round(sum(rss), 0), rss_max=max(rss) if rss else 0,
                            dir_median=peers[len(peers) // 2] if peers else None, dir_min=peers[0] if peers else None, rows=rows)

    def traffic(self):
        i = 0
        while not self.stop.wait(self.a.probe_every):
            alive = [n for n in self.alive_nodes() if n.cookie and n.id]
            if len(alive) < 2:
                continue
            a = self.rnd.choice(alive)
            fr = [f for f in a.friends if f.alive() and not f.frozen and f.cookie and f.id]
            if not fr:
                continue
            b = self.rnd.choice(fr)
            i += 1
            text = f"probe-{i}-{int(now())}"
            t = now()
            st, body = a.api("POST", f"/api/chat/send/{b.id}", {"text": text}, timeout=20)
            ok = st == 200 and isinstance(body, dict) and body.get("status") in ("success", "ok")
            with self.plock:
                # was the recipient (and the sender) up for a while? A message to a node that was just restarted or is down is
                # a different question from a message between two settled nodes.
                settled = bool(a.up_since and b.up_since and now() - a.up_since > 60 and now() - b.up_since > 60)
                # alive for a minute is not enough: the network under the process must be back too
                if settled and self.lab is not None and a.net and b.net:
                    settled = self.lab.net_ready(a.k) and self.lab.net_ready(b.k)
                self.probes.append({"t": t, "a": a.k, "b": b.k, "text": text, "sent_ok": ok, "delivered": None, "settled": settled})
            threading.Thread(target=self.check_probe, args=(self.probes[-1], a, b), daemon=True).start()

    def check_probe(self, pr, a, b):
        end = now() + self.a.probe_wait
        while now() < end and not self.stop.is_set():
            if b.alive() and not b.frozen and b.cookie:
                st, body = b.api("GET", f"/api/chat/history/{a.id}", timeout=10)
                if st == 200 and isinstance(body, dict):
                    msgs = body.get("messages") or body.get("history") or []
                    if any(pr["text"] in json.dumps(m, ensure_ascii=False) for m in msgs):
                        pr["delivered"] = round(now() - pr["t"], 2)
                        return
            time.sleep(2)

    def attack_round(self, seconds=45):
        """Hostile traffic from a stranger on the 'internet' at three public nodes at once; then check the victims are fine."""
        pool = [n for n in self.nodes if n.alive() and not n.frozen and n.cookie and not n.client and (not n.net or n.net["group"] is None)]
        if len(pool) < 3:
            return
        victims = self.rnd.sample(pool, 3)
        nsp = f"/run/netns/{self.atk_net['ns']}"
        before = {v.k: {"rss": v.rss_mb(), "dir": v.directory(), "kernel": v.kernel()} for v in victims}
        self.log.event("attack", victims=[v.k for v in victims], seconds=seconds, kinds=["udp_garbage", "udp_mutated", "tcp_flood", "slow_tls", "ws_garbage"])
        jobs = []
        for v in victims:
            ip, base = v.net["ip"], v.disc
            udp_ports = [v.disc, v.data, v.p2pd, v.p2pdata]
            jobs += [
                threading.Thread(target=lambda ip=ip, p=udp_ports: ATK.udp_garbage(nsp, ip, p, seconds)),
                threading.Thread(target=lambda ip=ip, p=udp_ports: ATK.udp_mutated(nsp, ip, p, seconds)),
                threading.Thread(target=lambda ip=ip, b=base: ATK.tcp_flood(nsp, ip, b, seconds)),
                threading.Thread(target=lambda ip=ip, b=base: ATK.slow_tls(nsp, ip, b, seconds)),
                threading.Thread(target=lambda ip=ip, b=base: ATK.ws_garbage(nsp, ip, b, seconds)),
            ]
        for j in jobs:
            j.start()
        # while the attack runs, the victims must keep answering
        slow = {v.k: 0 for v in victims}
        end = now() + seconds
        while now() < end and not self.stop.is_set():
            for v in victims:
                t = now()
                st, _ = v.api("GET", "/api/network/offers", timeout=10)
                if st != 200 or now() - t > 3:
                    slow[v.k] += 1
            time.sleep(3)
        for j in jobs:
            j.join(timeout=seconds + 30)
        time.sleep(10)
        for v in victims:
            after = {"rss": v.rss_mb(), "dir": v.directory(), "kernel": v.kernel(), "alive": v.alive()}
            ok = v.alive() and after["dir"] is not None
            self.log.event("attack_result", victim=v.k, alive=v.alive(), answers_after=after["dir"] is not None, slow_or_failed_checks=slow[v.k],
                           rss_before=round(before[v.k]["rss"] or 0, 1), rss_after=round(after["rss"] or 0, 1))
            if not ok:
                self.problem(f"victim {v.k} is not healthy after the attack")
            elif (after["rss"] or 0) > (before[v.k]["rss"] or 0) * 2 + 50:
                self.problem(f"victim {v.k} grew from {before[v.k]['rss']:.0f} to {after['rss']:.0f} MB during the attack")

    def watchdog(self):
        while not self.stop.wait(5):
            if now() > self.deadline:
                self.problem("hard time limit reached — stopping the run")
                self.stop.set()
                return
            total = sum((n.rss_mb() or 0) for n in self.nodes)
            if total > self.a.max_rss_gb * 1024:
                self.problem(f"memory limit exceeded ({total/1024:.1f} GB) — stopping the run")
                self.stop.set()
                return

    # ---- fault actions
    def wave_kill(self, nodes, how="kill"):
        self.log.event("wave", action=how, count=len(nodes), nodes=[n.k for n in nodes])
        for n in nodes:
            if how == "freeze":
                n.freeze()
            else:
                n.kill(signal.SIGKILL)

    def restore(self, nodes, rate_per_10s=None):
        self.log.event("restore", count=len(nodes), mode="gradual" if rate_per_10s else "mass")
        def one(n):
            if n.frozen:
                n.thaw()
                n.up_since = now()
                return True
            n.start()
            return n.login(120) and n.ready(120)
        if rate_per_10s:
            for i in range(0, len(nodes), rate_per_10s):
                for n in nodes[i:i + rate_per_10s]:
                    threading.Thread(target=one, args=(n,), daemon=True).start()
                if self.stop.wait(10):
                    return
        else:
            with ThreadPoolExecutor(max_workers=len(nodes) or 1) as ex:
                list(ex.map(one, nodes))

    def converged(self, min_frac=0.9, timeout=300):
        """Converged = most running nodes know most of the running public nodes (directory size), and answer the web API."""
        end = now() + timeout
        t0 = now()
        while now() < end and not self.stop.is_set():
            alive = [n for n in self.nodes if n.alive() and not n.frozen]
            logged = [n for n in alive if n.cookie]
            public = [n for n in alive if not n.client]
            if len(logged) >= 0.9 * len(alive) and alive:
                with ThreadPoolExecutor(max_workers=12) as ex:
                    dirs = list(ex.map(lambda n: n.directory(), logged))
                need = max(1, int(0.8 * (len(public) - 1)))
                good = sum(1 for d in dirs if d and d[0] >= need)
                if good >= min_frac * len(logged):
                    return round(now() - t0, 1)
            time.sleep(5)
        return None

    # ---- scenario
    def run(self):
        a = self.a
        threading.Thread(target=self.watchdog, daemon=True).start()
        n_entries = max(2, min(a.entries, a.nodes))
        client_ids = set(self.rnd.sample(range(n_entries + 1, a.nodes + 1), int((a.nodes - n_entries) * a.nat_fraction))) if a.nat_fraction > 0 else set()
        if a.lab:
            self.lab = L.Lab()
            self.lab.base()
            groups = max(1, (len(client_ids) + 5) // 6)
            kinds = ["full", "symmetric"]
            for g in range(1, groups + 1):
                self.lab.router(g, kinds[(g - 1) % 2])
            nat_order = sorted(client_ids)
            for k in range(1, a.nodes + 1):
                if k in client_ids:
                    g = 1 + nat_order.index(k) % groups
                    net = self.lab.natted_node(k, g)
                else:
                    net = self.lab.public_node(k)
                if a.delay_ms or a.loss_pct:
                    self.lab.netem(k, a.delay_ms, a.jitter_ms, a.loss_pct)
                self.nodes.append(Node(k, self.out, a.binary, a.base_port, client=False, anchor=(k == 1), net=net))
            self.atk_net = self.lab.public_node(900)
            self.log.event("lab", routers={g: r["kind"] for g, r in self.lab.routers.items()}, nat_nodes=sorted(client_ids), delay_ms=a.delay_ms, loss_pct=a.loss_pct)
        else:
            for k in range(1, a.nodes + 1):
                self.nodes.append(Node(k, self.out, a.binary, a.base_port, client=k in client_ids, anchor=(k == 1)))
        entries = self.nodes[:n_entries]
        self.log.event("setup", nodes=a.nodes, entries=n_entries, client_nodes=sorted(client_ids), binary=a.binary)

        self.log.event("phase", name="start entries")
        self.up(entries)
        self.introduce(entries, entries)
        self.log.event("phase", name="start the rest")
        rest = self.nodes[n_entries:]
        for i in range(0, len(rest), 10):
            batch = rest[i:i + 10]
            self.up(batch)
            self.introduce(batch, entries)
            if self.stop.is_set():
                break
        self.make_friends(self.nodes)
        threading.Thread(target=self.sampler, daemon=True).start()
        t = self.converged(timeout=a.converge_timeout)
        self.log.event("converged", seconds=t)
        if t is None:
            self.problem("the network did not converge after start")
        threading.Thread(target=self.traffic, daemon=True).start()
        self.stop.wait(a.steady)

        regular = [n for n in self.nodes if n not in entries]
        if a.scenario == "quick":
            plan = [("kill", 0.3), ("restore_all", None)]
        else:
            plan = [("kill", 0.3), ("hold", None), ("kill", 0.6), ("hold", None), ("kill", 0.95), ("hold", None), ("gradual", None), ("hold", None),
                    ("freeze", 0.4), ("hold", None), ("thaw_all", None)]
            if a.lab:
                plan += [("cut", 0.3), ("hold", None), ("uncut", None), ("hold", None), ("storm", 0.25), ("hold", None), ("calm", None), ("hold", None),
                         ("attack", None), ("hold", None)]
            plan += [("kill_all_but_one_entry", None), ("hold", None), ("mass", None), ("hold", None)]
        down = []
        for step, arg in plan:
            if self.stop.is_set():
                break
            if step == "kill":
                target = int(len(self.nodes) * arg) if arg < 0.9 else len(self.nodes) - max(2, int(len(self.nodes) * 0.05))
                pool = [n for n in self.nodes if n.alive() and not n.frozen]
                victims = self.rnd.sample(pool, max(0, min(len(pool), target - len(down))))
                self.wave_kill(victims)
                down += victims
            elif step == "freeze":
                pool = [n for n in self.nodes if n.alive() and not n.frozen]
                victims = self.rnd.sample(pool, int(len(pool) * arg))
                self.wave_kill(victims, "freeze")
            elif step == "cut":
                pool = [n for n in self.nodes if n.alive() and not n.frozen]
                self.cut = self.rnd.sample(pool, int(len(pool) * arg))
                self.log.event("wave", action="cable-cut", count=len(self.cut), nodes=[n.k for n in self.cut])
                for n in self.cut:
                    self.lab.link(n.k, False)
            elif step == "uncut":
                self.log.event("restore", count=len(self.cut), mode="cables back")
                for n in self.cut:
                    self.lab.link(n.k, True)
                self.cut = []
            elif step == "storm":
                self.log.event("wave", action="loss-storm", loss_pct=int(arg * 100))
                for n in self.nodes:
                    self.lab.netem(n.k, 80, 40, int(arg * 100), 5)
            elif step == "calm":
                self.log.event("restore", mode="loss storm over")
                for n in self.nodes:
                    self.lab.netem(n.k, a.delay_ms, a.jitter_ms, a.loss_pct)
            elif step == "attack":
                self.attack_round(a.attack_seconds)
            elif step == "thaw_all":
                self.restore([n for n in self.nodes if n.frozen])
            elif step == "kill_all_but_one_entry":
                victims = [n for n in self.nodes if n.alive() and n is not entries[0]]
                self.wave_kill(victims)
                down = [n for n in self.nodes if not n.alive()]
            elif step == "hold":
                self.stop.wait(a.hold)
            elif step == "gradual":
                self.restore(list(down), rate_per_10s=max(1, a.nodes // 12))
                down = []
                t = self.converged(timeout=a.converge_timeout)
                self.log.event("recovered_gradual", seconds=t)
            elif step in ("mass", "restore_all"):
                down = [n for n in self.nodes if not n.alive()]
                self.restore(down)
                down = []
                t = self.converged(timeout=a.converge_timeout)
                self.log.event("recovered_mass", seconds=t)
                if t is None:
                    self.problem("no convergence after the mass restore")
        self.stop.wait(a.steady)
        self.log.event("phase", name="final check")
        self.final_check()
        self.stop.set()

    def final_check(self):
        for n in self.nodes:
            if not n.alive():
                self.problem(f"node {n.k} is not running at the end")
        k = []
        for n in self.alive_nodes():
            ks = n.kernel()
            if isinstance(ks, dict):
                k.append((n.k, ks))
        self.log.event("kernel_snapshot", nodes=len(k))
        with open(os.path.join(self.out, "kernel.json"), "w") as f:
            json.dump(k, f, ensure_ascii=False, indent=1)

    def shutdown(self):
        self.stop.set()
        for n in self.nodes:
            try:
                n.kill(signal.SIGKILL)
            except Exception:
                pass
        self.log.event("shutdown", processes_left=len([n for n in self.nodes if n.alive()]))

    def report(self):
        with self.plock:
            probes = list(self.probes)
        sent = [p for p in probes if p["sent_ok"]]
        delivered = [p for p in sent if p["delivered"] is not None]
        settled = [p for p in sent if p.get("settled")]
        unsettled = [p for p in sent if not p.get("settled")]
        # while a recipient is down a message cannot arrive (there is no outbox yet): keep these apart
        lat = sorted(p["delivered"] for p in delivered)
        mt = [json.loads(l) for l in open(os.path.join(self.out, "metrics.jsonl"))] if os.path.exists(os.path.join(self.out, "metrics.jsonl")) else []
        ev = [json.loads(l) for l in open(os.path.join(self.out, "events.jsonl"))]
        rss_series = [m["rss_total"] for m in mt if m.get("rss_total")]
        lines = ["# Chaos run report", "", f"- nodes: {self.a.nodes}, entries: {self.a.entries}, scenario: {self.a.scenario}, seed: {self.a.seed}",
                 f"- chat probes: sent ok {len(sent)} of {len(probes)}, delivered {len(delivered)}"
                 + (f" ({100*len(delivered)/len(sent):.1f}%)" if sent else ""),
                 f"- between SETTLED nodes (both up > 60 s): {len([p for p in settled if p['delivered'] is not None])} of {len(settled)} delivered"
                 + (f" ({100*len([p for p in settled if p['delivered'] is not None])/len(settled):.1f}%)" if settled else ""),
                 f"- involving a node that was down or just restarted: {len([p for p in unsettled if p['delivered'] is not None])} of {len(unsettled)} delivered",
                 f"- delivery time: median {lat[len(lat)//2]:.1f}s, 95th {lat[int(len(lat)*0.95)-1 if len(lat)>1 else 0]:.1f}s" if lat else "- delivery time: n/a",
                 f"- total memory of all nodes: start {rss_series[0]:.0f} MB, max {max(rss_series):.0f} MB, end {rss_series[-1]:.0f} MB" if rss_series else "- memory: n/a",
                 "", "## Events", ""]
        for e in ev:
            if e["kind"] in ("setup", "lab", "phase", "converged", "recovered_gradual", "recovered_mass", "wave", "restore", "attack", "attack_result", "PROBLEM", "shutdown"):
                extra = {k: v for k, v in e.items() if k not in ("t", "kind", "nodes")}
                lines.append(f"- {time.strftime('%H:%M:%S', time.localtime(e['t']))} **{e['kind']}** {extra}")
        lines += ["", "## Verdict", ""]
        lines.append("PROBLEMS: " + "; ".join(self.problems) if self.problems else "No problems detected.")
        # delivery in windows
        if sent:
            lines += ["", "## Chat delivery by minute", "", "| minute | sent | delivered |", "|---|---|---|"]
            t0 = sent[0]["t"]
            byminute = {}
            for p in sent:
                m = int((p["t"] - t0) // 60)
                s = byminute.setdefault(m, [0, 0])
                s[0] += 1
                s[1] += 1 if p["delivered"] is not None else 0
            for m, (s, d) in sorted(byminute.items()):
                lines.append(f"| {m} | {s} | {d} |")
        with open(os.path.join(self.out, "REPORT.md"), "w") as f:
            f.write("\n".join(lines) + "\n")
        with open(os.path.join(self.out, "probes.json"), "w") as f:
            json.dump(probes, f, ensure_ascii=False)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--nodes", type=int, default=30)
    p.add_argument("--entries", type=int, default=4)
    p.add_argument("--binary", default="target/release/yandi")
    p.add_argument("--out", default="/tmp/chaos_run")
    p.add_argument("--scenario", choices=["full", "quick"], default="full")
    p.add_argument("--nat-fraction", type=float, default=0.3, help="share of non-entry nodes that play nodes behind NAT (client mode)")
    p.add_argument("--hold", type=int, default=120)
    p.add_argument("--steady", type=int, default=120)
    p.add_argument("--converge-timeout", type=int, default=300)
    p.add_argument("--sample-every", type=float, default=10)
    p.add_argument("--probe-every", type=float, default=3)
    p.add_argument("--probe-wait", type=float, default=60)
    p.add_argument("--max-minutes", type=int, default=240)
    p.add_argument("--max-rss-gb", type=float, default=16)
    p.add_argument("--base-port", type=int, default=26000)
    p.add_argument("--seed", type=int, default=1)
    p.add_argument("--lab", action="store_true", help="run every node in its own network namespace with real NAT (rootless sandbox, see chaos/lab.py)")
    p.add_argument("--attack-seconds", type=int, default=45)
    p.add_argument("--delay-ms", type=int, default=20)
    p.add_argument("--jitter-ms", type=int, default=5)
    p.add_argument("--loss-pct", type=float, default=0.5)
    a = p.parse_args()
    if a.lab:
        L.reexec_in_sandbox()
    a.binary = os.path.abspath(a.binary)
    c = Chaos(a)
    def bye(*_):
        c.stop.set()
    signal.signal(signal.SIGTERM, bye)
    try:
        c.run()
    except Exception as e:
        import traceback
        c.problem("harness error: " + repr(e))
        traceback.print_exc()
    finally:
        time.sleep(1)
        c.shutdown()
        c.report()
        print("report:", os.path.join(c.out, "REPORT.md"))


if __name__ == "__main__":
    main()
