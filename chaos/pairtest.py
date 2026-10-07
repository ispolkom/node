#!/usr/bin/env python3
"""Small experiment: do chat messages between mutual contacts get through, and does a stuck pair heal on its own?"""
import sys, time, json, os, random, threading
sys.path.insert(0, os.path.dirname(__file__))
import chaos as C
import lab as LABMOD

def main():
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 4
    minutes = float(sys.argv[2]) if len(sys.argv) > 2 else 3
    out = sys.argv[3] if len(sys.argv) > 3 else "/tmp/chaos_pair"
    binary = os.path.abspath(sys.argv[4]) if len(sys.argv) > 4 else os.path.abspath("target/debug/yandi")
    os.makedirs(out, exist_ok=True)
    use_lab = os.environ.get("PAIR_LAB") == "1"
    lab = None
    if use_lab:
        LABMOD.reexec_in_sandbox()
        lab = LABMOD.Lab()
        lab.base()
        nets = {k: lab.public_node(k) for k in range(1, n + 1)}
        delay, loss = int(os.environ.get("PAIR_DELAY", "20")), float(os.environ.get("PAIR_LOSS", "0.5"))
        for k in nets:
            if delay or loss:
                lab.netem(k, delay, 5, loss)
        nodes = [C.Node(k, out, binary, anchor=(k == 1), net=nets[k]) for k in range(1, n + 1)]
    else:
        nodes = [C.Node(k, out, binary, anchor=(k == 1)) for k in range(1, n + 1)]
    for x in nodes:
        x.start()
    for x in nodes:
        assert x.login(120) and x.ready(120), x.k
    mode = os.environ.get("PAIR_MODE", "simultaneous")
    pairs = [(a, b) for i, a in enumerate(nodes) for b in nodes[i + 1:]]
    if mode == "simultaneous":
        ths = []
        for a, b in pairs:
            ths += [threading.Thread(target=a.trust, args=(b,)), threading.Thread(target=b.trust, args=(a,))]
        for t in ths: t.start()
        for t in ths: t.join()
    else:  # one side first, the other a little later
        for a, b in pairs:
            a.trust(b)
        time.sleep(20)
        for a, b in pairs:
            b.trust(a)
    t0 = time.time()
    stats = {}
    i = 0
    restarted = False
    while time.time() - t0 < minutes * 60:
        if os.environ.get("PAIR_RESTART") == "1" and not restarted and time.time() - t0 > 60:
            restarted = True
            victim = nodes[1]
            print(f"[{time.time()-t0:.0f}s] killing node {victim.k} and starting it again at once")
            victim.kill()
            victim.start()
            victim.login(120)
            victim.ready(120)
            print(f"[{time.time()-t0:.0f}s] node {victim.k} is back")
        for a, b in pairs:
            for s, r in ((a, b), (b, a)):
                i += 1
                text = f"m{i}"
                s.api("POST", f"/api/chat/send/{r.id}", {"text": text})
                stats.setdefault((s.k, r.k), []).append([time.time() - t0, text, None])
        time.sleep(4)
        # collect deliveries
        for (sk, rk), L in stats.items():
            r = nodes[rk - 1]; s = nodes[sk - 1]
            st, body = r.api("GET", f"/api/chat/history/{s.id}")
            hist = json.dumps(body) if st == 200 else ""
            for e in L:
                if e[2] is None and e[1] in hist:
                    e[2] = round(time.time() - t0 - e[0], 1)
    for (sk, rk), L in sorted(stats.items()):
        line = "".join("." if e[2] is not None else "x" for e in L)
        print(f"{sk}->{rk}: {line}")
    for x in nodes:
        x.kill()

main()
