# Chaos tests

Network-level tests with real node processes: start dozens of nodes, keep chat traffic running, kill / freeze / cut / restore them in waves
(up to 95% of the network), add packet loss and hostile participants, and write everything to log files plus a `REPORT.md`.
Nobody has to watch: a hard time limit and a memory watchdog stop and clean up the run.

```
cargo build --release
python3 chaos/chaos.py --nodes 30 --binary target/release/yandi --out /tmp/chaos_30            # loopback, no root
python3 chaos/chaos.py --nodes 60 --lab --binary target/release/yandi --out /tmp/chaos_60       # sandbox with NAT, delays, loss, attackers
```

* `chaos.py` — the orchestrator and the scenarios (`--scenario full|quick`).
* `lab.py` — the network lab. It re-runs itself inside a private user+network+mount namespace (`unshare -Urnm`): **no root, the real
  network of the machine is never touched**, everything disappears when the run ends. Public nodes sit on a bridge ("the internet",
  11.77.0.0/16); NATed nodes sit behind routers with `full` (cone-like) or `symmetric` NAT; `tc netem` adds delay/jitter/loss.
* `attackers.py` — hostile traffic (garbage and mutated UDP, TCP floods, slow TLS, broken HTTP/WebSocket) fired at public nodes from a
  stranger's address during the run.
* `pairtest.py` — a small experiment: do chat messages between mutual contacts get through, and does a stuck pair heal?

Output directory: `events.jsonl` (what the test did and saw), `metrics.jsonl` (per-node memory / directory size / peers every few
seconds), `probes.json` (every chat probe), `kernel.json`, `node<k>/node.log` (each node's own log), `REPORT.md` (the summary).

Limits: everything runs on one machine, so latency and bandwidth are better than reality and an overloaded CPU can look like a failure.
