#!/usr/bin/env python3
"""NAT experiments after a successful punch (port-preserving NAT on both sides, different routers):
  idle     - how long can a punched path stay silent before it stops working, with and without a keepalive of KEEP seconds
  ipchange - the outside address of one router changes while the path is in use: how long to punch again with the same sockets
Usage: python3 chaos/punchtest2.py [out=/tmp/punch2.json]
"""
import json, os, socket, sys, threading, time, statistics

sys.path.insert(0, os.path.dirname(__file__))
import lab as LAB
import attackers as ATK
import punchtest as P

RDV = f"{LAB.INTERNET_NET}.0.1"
KEEP = 15


def make_sock(nspath):
    r = ATK._in_ns(nspath, lambda: _mk())
    return r["r"]


def _mk():
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind(("0.0.0.0", 0))
    s.settimeout(0.05)
    return s


def register(s, aid, me):
    for _ in range(60):
        s.sendto(f"{aid}:{me}".encode(), (RDV, P.RDV_PORT))
        try:
            data, _ = s.recvfrom(256)
            peer, mine = data.decode().split("|")
            ip, port = peer.split(":")
            return (ip, int(port)), mine
        except socket.timeout:
            continue
    return None, None


def punch_pair(sa, sb, aid, budget=3.0):
    """Both sides register and probe; returns ms until both heard the other, or None, plus the peers' addresses."""
    res = {}

    def one(s, me):
        peer, mine = register(s, aid, me)
        if peer is None:
            res[me] = None
            return
        t0 = time.time()
        got = None
        nxt = t0
        limit = budget
        while time.time() - t0 < limit:
            if time.time() >= nxt:
                s.sendto(f"P:{aid}:{me}".encode(), peer)
                nxt = time.time() + 0.05
            try:
                d, src = s.recvfrom(256)
            except socket.timeout:
                continue
            if d.startswith(f"P:{aid}:".encode()) and not d.startswith(f"P:{aid}:{me}".encode()):
                got = (time.time() - t0) * 1000
                limit = min(limit, time.time() - t0 + 0.4)
        res[me] = (got, peer)

    ts = [threading.Thread(target=one, args=(sa, "A")), threading.Thread(target=one, args=(sb, "B"))]
    [t.start() for t in ts]; [t.join() for t in ts]
    if res.get("A") and res.get("B") and res["A"][0] is not None and res["B"][0] is not None:
        return max(res["A"][0], res["B"][0]), res["A"][1], res["B"][1]
    return None, None, None


def drain(s):
    s.settimeout(0.01)
    try:
        while True:
            s.recvfrom(256)
    except socket.timeout:
        pass
    s.settimeout(0.05)


def heard(sender, receiver, to, tag, wait=1.0):
    drain(receiver)
    end = time.time() + wait
    while time.time() < end:
        sender.sendto(tag, to)
        try:
            d, _ = receiver.recvfrom(256)
            if d == tag:
                return True
        except socket.timeout:
            pass
    return False


def exp_idle(lab, na, nb, results):
    for keep in (False, True):
        for idle in (10, 20, 30, 40, 60, 90, 130):
            n = 6
            rows = [None] * n

            def attempt(i):
                sa, sb = make_sock(na), make_sock(nb)
                ms, pa, pb = punch_pair(sa, sb, f"idle{idle}{keep}{i}-{int(time.time()*1000)%100000}")
                if ms is None:
                    rows[i] = "nopunch"; return
                end = time.time() + idle
                while time.time() < end:
                    time.sleep(min(KEEP if keep else idle, max(0.0, end - time.time())))
                    if keep and time.time() < end:
                        sa.sendto(b"k", pa); sb.sendto(b"k", pb)
                ab = heard(sa, sb, pa, b"ab")
                ba = heard(sb, sa, pb, b"ba")
                rows[i] = (ab, ba)

            ts = [threading.Thread(target=attempt, args=(i,)) for i in range(n)]
            [t.start() for t in ts]; [t.join() for t in ts]
            ok = sum(1 for r in rows if isinstance(r, tuple) and r[0] and r[1])
            line = {"idle_s": idle, "keepalive": keep, "attempts": n, "path_alive_both_ways": ok}
            results.setdefault("idle", []).append(line)
            print(json.dumps(line), flush=True)


def exp_ipchange(lab, na, nb, results):
    rtimes, alive_after = [], 0
    sa, sb = make_sock(na), make_sock(nb)
    ms, pa, pb = punch_pair(sa, sb, "ipc0")
    assert ms is not None, "first punch failed"
    for i in range(1, 11):
        new = 253 if i % 2 == 1 else 254
        old = 254 if i % 2 == 1 else 253
        LAB.sh([LAB.IP, "-n", "r1", "addr", "del", f"{LAB.INTERNET_NET}.{old}.1/16", "dev", "w0"])
        LAB.sh([LAB.IP, "-n", "r1", "addr", "add", f"{LAB.INTERNET_NET}.{new}.1/16", "dev", "w0"])
        # does the old path still work? (it should not: the peer shoots at the old address)
        old_works = heard(sa, sb, pa, b"x1", wait=0.6) and heard(sb, sa, pb, b"x2", wait=0.6)
        alive_after += old_works
        t0 = time.time()
        ms, pa2, pb2 = punch_pair(sa, sb, f"ipc{i}")
        if ms is not None:
            rtimes.append((time.time() - t0) * 1000)
            pa, pb = pa2, pb2
        else:
            print("re-punch failed at", i, flush=True)
            break
    res = {"changes": i, "old_path_still_worked": alive_after, "repunched": len(rtimes), "median_ms": round(statistics.median(rtimes)) if rtimes else None, "max_ms": round(max(rtimes)) if rtimes else None}
    results["ipchange"] = res
    print(json.dumps(res), flush=True)


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "/tmp/punch2.json"
    LAB.reexec_in_sandbox()
    lab = LAB.Lab()
    lab.base()
    for g in (1, 2):
        lab.router(g, "full")
        lab.natted_node(g, g)
    P.Rendezvous().start()
    results = {}
    na, nb = lab.nspath(1), lab.nspath(2)
    exp_ipchange(lab, na, nb, results)
    exp_idle(lab, na, nb, results)
    json.dump(results, open(out, "w"), indent=1)


if __name__ == "__main__":
    main()
