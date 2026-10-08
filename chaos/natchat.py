#!/usr/bin/env python3
"""Chat between two nodes behind NATs of different routers, with public nodes as common acquaintances (the introducers).
Real nodes in the rootless lab. Phases (each prints one line):
  first    - first contact: how long until the first message of A to B arrives (nobody has a way to the other yet)
  reply    - B answers A
  silence  - 75 s of silence on the application level (keepalive keeps the NAT open), then a message
  ipchange - the outside address of A's router changes; seconds until messages flow again
Usage: python3 chaos/natchat.py [kind-of-router-B: full|symmetric] [binary] [out] [kind-of-router-A: full|symmetric] [relay-loss: 0|1]
  with relay-loss=1 the node that holds the relay allocation is killed after the first phases and the time until the chat works again is measured
"""
import json, os, sys, time, subprocess, threading

sys.path.insert(0, os.path.dirname(__file__))
import chaos as C
import lab as LAB


def history_has(r, s, text):
    st, body = r.api("GET", f"/api/chat/history/{s.id}")
    return st == 200 and text in json.dumps(body, ensure_ascii=False)


def send_and_wait(s, r, text, limit):
    t = time.time()
    s.api("POST", f"/api/chat/send/{r.id}", {"text": text})
    while time.time() - t < limit:
        if history_has(r, s, text):
            return round(time.time() - t, 1)
        time.sleep(0.5)
    return None


def grep_count(node, needle):
    try:
        return sum(1 for l in open(os.path.join(node.dir, "node.log"), errors="replace") if needle in l)
    except OSError:
        return 0


def main():
    kind_b = sys.argv[1] if len(sys.argv) > 1 else "full"
    binary = os.path.abspath(sys.argv[2]) if len(sys.argv) > 2 else os.path.abspath("target/release/yandi")
    out = sys.argv[3] if len(sys.argv) > 3 else "/tmp/natchat"
    os.makedirs(out, exist_ok=True)
    LAB.reexec_in_sandbox()
    lab = LAB.Lab()
    lab.base()
    nets = {k: lab.public_node(k) for k in (1, 2, 3)}
    kind_a = sys.argv[4] if len(sys.argv) > 4 else "full"
    lose_relay = len(sys.argv) > 5 and sys.argv[5] == "1"
    lab.router(1, kind_a)
    lab.router(2, kind_b)
    nets[4] = lab.natted_node(4, 1)
    nets[5] = lab.natted_node(5, 2)
    for k in nets:
        lab.netem(k, 20, 5, 0.5)
    nodes = [C.Node(k, out, binary, anchor=(k == 1), net=nets[k]) for k in range(1, 6)]
    pub, A, B = nodes[:3], nodes[3], nodes[4]
    for n in nodes:
        n.start()
    for n in nodes:
        assert n.login(120) and n.ready(120), n.k
    # everyone knows the public nodes; A and B are contacts of each other (their cards carry only their private addresses)
    for n in nodes:
        for m in nodes:
            if m is not n:
                n.trust(m)
    time.sleep(35)
    res = {"router_A": kind_a, "router_B": kind_b}
    res["first_s"] = send_and_wait(A, B, "first-1", 60)
    print("first contact A->B (s):", res["first_s"], flush=True)
    res["reply_s"] = send_and_wait(B, A, "reply-1", 30)
    print("reply B->A (s):", res["reply_s"], flush=True)
    time.sleep(75)
    res["after_silence_s"] = send_and_wait(A, B, "silence-1", 30)
    print("after 75 s of silence A->B (s):", res["after_silence_s"], flush=True)
    if lose_relay:
        holder = None
        for n in pub:
            if grep_count(n, "allocated ports") > 0:
                holder = n
        res["relay_holder"] = holder.k if holder else None
        if holder:
            holder.kill()
            t = time.time()
            rec = None
            n_ = 0
            while time.time() - t < 150:
                n_ += 1
                d = send_and_wait(A, B, f"relayloss-{n_}", 8)
                if d is not None:
                    rec = round(time.time() - t, 1)
                    break
            res["relay_loss_recovery_s"] = rec
            print("the relay node was killed; the chat works again after (s):", rec, flush=True)
            res["allocations_after"] = sum(grep_count(n, "allocated ports") for n in pub if n is not holder)
    # the outside address of A's router changes
    LAB.sh([LAB.IP, "-n", "r1", "addr", "del", f"{LAB.INTERNET_NET}.254.1/16", "dev", "w0"])
    LAB.sh([LAB.IP, "-n", "r1", "addr", "add", f"{LAB.INTERNET_NET}.253.1/16", "dev", "w0"])
    t = time.time()
    gone = None
    n = 0
    while time.time() - t < 120:
        n += 1
        d = send_and_wait(A, B, f"ipchange-{n}", 6)
        if d is not None:
            gone = round(time.time() - t, 1)
            break
    res["ipchange_recovery_s"] = gone
    print("after the address change, messages flow again after (s):", gone, flush=True)
    res["punch_lines_A"] = grep_count(A, "[punch] ✅")
    res["punch_lines_B"] = grep_count(B, "[punch] ✅")
    res["introduced_A"] = grep_count(A, "[punch] 🤝")
    res["introduced_B"] = grep_count(B, "[punch] 🤝")
    print(json.dumps(res), flush=True)
    json.dump(res, open(os.path.join(out, "result.json"), "w"), indent=1)
    for x in nodes:
        x.kill()


main()
