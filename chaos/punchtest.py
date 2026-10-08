#!/usr/bin/env python3
"""Measurement of UDP hole punching between two hosts behind NATs of given kinds, in the rootless lab.
For every pair of kinds it runs N independent attempts (fresh sockets, so fresh NAT mappings). One attempt:
  both sides tell a public rendezvous host their outside address (the one the NAT shows); the rendezvous host gives each the other's;
  then both send numbered probes to the other's outside address every 50 ms and listen. Success = both sides received a probe of the other.
Recorded per attempt: success, milliseconds from the rendezvous answer to the first received probe, probes sent until then, and the outside
address/port each side was shown (to see whether a mapping depends on the destination).
Usage: python3 chaos/punchtest.py [attempts=100] [out=/tmp/punch.json]
"""
import json, os, socket, sys, threading, time, statistics

sys.path.insert(0, os.path.dirname(__file__))
import lab as LAB
import attackers as ATK

RDV_PORT = 7000
PROBE_EVERY = 0.05
LISTEN_FOR = 3.0


class Rendezvous(threading.Thread):
    """Answers each side with the other side's observed address once both have registered under the same attempt id."""

    def __init__(self):
        super().__init__(daemon=True)
        self.s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.s.bind(("0.0.0.0", RDV_PORT))
        self.wait = {}

    def run(self):
        while True:
            data, src = self.s.recvfrom(256)
            try:
                aid, side = data.decode().split(":")
            except ValueError:
                continue
            ent = self.wait.setdefault(aid, {})
            ent[side] = src
            if len(ent) == 2:
                for me, other in (("A", "B"), ("B", "A")):
                    self.s.sendto(f"{ent[other][0]}:{ent[other][1]}|{ent[me][0]}:{ent[me][1]}".encode(), ent[me])
                del self.wait[aid]


def side(nspath, rdv_ip, aid, me, out):
    """One side of one attempt, run inside the host's network namespace."""
    def run():
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.bind(("0.0.0.0", 0))
        s.settimeout(0.05)
        for _ in range(40):  # the registration may be lost: repeat until answered
            s.sendto(f"{aid}:{me}".encode(), (rdv_ip, RDV_PORT))
            try:
                data, _ = s.recvfrom(256)
                break
            except socket.timeout:
                continue
        else:
            return {"ok": False, "why": "no rendezvous answer"}
        peer, mine = data.decode().split("|")
        pip, pport = peer.split(":")
        t0 = time.time()
        sent, first = 0, None
        end = t0 + LISTEN_FOR
        next_send = t0
        while time.time() < end:
            now = time.time()
            if now >= next_send and (first is None or sent < 200):
                s.sendto(f"P:{aid}:{me}:{sent}".encode(), (pip, int(pport)))
                sent += 1
                next_send = now + PROBE_EVERY
            try:
                data, src = s.recvfrom(256)
            except socket.timeout:
                continue
            if data.startswith(f"P:{aid}:".encode()) and not data.startswith(f"P:{aid}:{me}:".encode()) and first is None:
                first = (time.time() - t0) * 1000
                sent_at_first = sent
                # keep answering a moment so the other side can also receive
                end = min(end, time.time() + 0.6)
        return {"ok": first is not None, "ms": first, "probes": sent_at_first if first is not None else sent, "mine": mine, "peer": peer}
    out[me] = ATK._in_ns(nspath, run).get("r", {"ok": False, "why": "error"})


def main():
    attempts = int(sys.argv[1]) if len(sys.argv) > 1 else 100
    outf = sys.argv[2] if len(sys.argv) > 2 else "/tmp/punch.json"
    LAB.reexec_in_sandbox()
    lab = LAB.Lab()
    lab.base()
    # one router and one host per kind-slot: two hosts of each kind sit behind DIFFERENT routers
    kinds = {"full": [1, 2], "symmetric": [3, 4]}
    hosts = {}
    k = 1
    for kind, gs in kinds.items():
        for g in gs:
            lab.router(g, kind)
            lab.natted_node(k, g)
            hosts[(kind, g)] = k
            k += 1
    rdv_ip = f"{LAB.INTERNET_NET}.0.1"
    Rendezvous().start()
    pairs = [("full", "full", (1, 2)), ("full", "symmetric", (1, 3)), ("symmetric", "symmetric", (3, 4))]
    results = {}
    for ka, kb, (ga, gb) in pairs:
        a, b = hosts[(ka, ga)], hosts[(kb, gb)]
        rows = []
        for i in range(attempts):
            aid = f"{ka[0]}{kb[0]}{i}-{int(time.time()*1000)%100000}"
            out = {}
            ths = [threading.Thread(target=side, args=(lab.nspath(a), rdv_ip, aid, "A", out)), threading.Thread(target=side, args=(lab.nspath(b), rdv_ip, aid, "B", out))]
            for t in ths: t.start()
            for t in ths: t.join()
            ra, rb = out.get("A", {}), out.get("B", {})
            rows.append({"ok": bool(ra.get("ok") and rb.get("ok")), "a": ra, "b": rb})
        ok = [r for r in rows if r["ok"]]
        ms = sorted(max(r["a"]["ms"], r["b"]["ms"]) for r in ok)
        probes = sorted(max(r["a"]["probes"], r["b"]["probes"]) for r in ok)
        # does the outside port depend on the destination? compare the port the rendezvous saw with the port the peer saw us on
        res = {"pair": f"{ka}<->{kb}", "attempts": attempts, "success": len(ok), "rate": round(len(ok) / attempts, 3)}
        if ms:
            res.update({"median_ms": round(statistics.median(ms)), "p95_ms": round(ms[int(0.95 * (len(ms) - 1))]), "median_probes": statistics.median(probes)})
        results[res["pair"]] = res
        print(json.dumps(res), flush=True)
        results[res["pair"] + "#sample"] = rows[:3]
    json.dump(results, open(outf, "w"), indent=1)


if __name__ == "__main__":
    main()
