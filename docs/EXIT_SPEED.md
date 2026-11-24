# Exit Speed: Analysis (2026-10-08)

Roadmap stage C. Measurements used `chaos/speedtest.py`: a client behind NAT with port preservation, a public exit node, and “Internet” in a laboratory sandbox; one and four streams; download and upload; with the same transfer directly, without nodes, for comparison. The laboratory had no latency or loss, so the limit came from the node rather than the link. **Owner constraint: full traffic duplication between nodes (two paths per wagon) remains; redundancy is more important than speed.** The ceiling is therefore intentionally close to half the link capacity, and all figures below include duplication.

## Measurements

| Path | Speed |
|---|---|
| Directly in the laboratory, without nodes | 6–30 Gbit/s; the laboratory link is not the limit. |
| Through an exit, original code, one and four streams, down and up | **about 45 Mbit/s**, the same for any number of streams; CPU used only 50–60% of one core. |
| The same, with node logs discarded | The same; logging is not the cause. |
| Experiment: remove `sleep` pauses before sending a wagon | **127 Mbit/s** download, CPU at 100% of one core, but exit upload stalls at 2–8 Mbit/s. The pauses were the only thing preventing the sender from overflowing the receive buffer. |

The last result shows the main issue: **there was no real flow control between nodes**. Speed was limited by an accidental side effect, a pause before each wagon; the Tokio timer is not more precise than 1 ms, so a nominal 92-microsecond pause lasts milliseconds. Simply removing the pauses is not a safe speed fix.

## Findings in the original data path

The path was `socks5/exit_node.rs` → `protocol/station.rs`:

1. The exit read the destination in 16 KiB chunks and encoded each chunk **as JSON containing a number array** (`serde_json` for `Vec<u8>`). Wire size grew roughly three to four times, and the CPU spent time encoding it.
2. Every “train” was sent under the **shared** `train_send_mutex`; the next train could not start until every wagon in the previous train was sent, so each chunk caused a stop and wait.
3. Every wagon used a separate Tokio task, a pause, and a semaphore; the pause was later replaced by a shared limiter. Each wagon was sent twice by design.
4. A **wagon could be up to 60 KiB** (`Wagon::MAX_CARGO_SIZE`), so one UDP datagram was split into about 40 IP fragments. Losing one fragment destroyed the datagram; on real networks fragmented UDP is often dropped as a whole. This likely explains poor field speed (70–80 of 300 Mbit/s); it was invisible in the lossless laboratory and required a loss test.
5. Automatic rate adjustment (`RateController::adjust_rate`) was commented out. The rate always started at 1000 Mbit/s, so there was no actual limit.
6. Every wagon generated several `println!` lines. This did not limit speed, but produced a 23 MB log during one experiment.

## Changes and results (2026-10-08)

Project principle, by owner decision: transport is “send and forget”, **without acknowledgements, retransmission requests, or feedback-based flow control**. Reliability comes only from full duplication over two paths. Therefore all improvements below are one-way.

| Step | Exit speed in the laboratory, with duplication |
|---|---|
| Original: at least 1 ms pause per wagon, wagon up to 60 KiB, JSON | 45 Mbit/s |
| 1200-byte wagon (`Wagon::PAYLOAD_SIZE`, one datagram without IP fragmentation) and compact tunnel-data packing (`Socks5TunnelData::encode`, three to four times smaller than JSON) | 70 Mbit/s, lower CPU use |
| Real rate limiter (`protocol/rate_controller.rs::Pacer`) instead of a pause per wagon, wire ceiling 300 Mbit/s including both copies | **122 Mbit/s stable**, one and four streams in both directions |
| Same, ceiling 450 (`YANDI_PACE_MBPS=450`) | 180 Mbit/s stable |
| Same, ceiling 700 | Upload breaks up: the receiver, with one processing worker and about 215 Mbit/s useful throughput, cannot keep up; the kernel drops datagrams (11,000 during the experiment). |

Additional experiments with one build and the stated wire ceiling:

- Printing every wagon in the send and receive hot paths was changed to trace level (`tracing::trace!`). It no longer appears during normal operation and does not occupy the CPU or the shared output lock.
- Multiple readers on one socket (`YANDI_DATA_WORKERS=4`, default 1) made downloading slightly faster (250 Mbit/s), **but exit upload stalled at a ceiling of 550**, where one reader was stable. Parallel processing of wagons in one stream therefore breaks train assembly; it remains disabled until the cause is understood.
- One reader with ceiling 550 delivered 207–229 Mbit/s useful throughput in both directions, stably, with one and four streams. At ceiling 700, single streams reached 240–253 Mbit/s, but four concurrent uploads stalled. The safe limit for this build in the laboratory is about 550 Mbit/s on the wire.

**Conclusions.**

1. Set a speed ceiling in advance, below the receiver and link capacity. The default is 300 Mbit/s on the wire, about 120 Mbit/s useful throughput; this is conservative because delivery is more important than speed. On a strong machine and good link, the ceiling can be raised to about 550 (`YANDI_PACE_MBPS=550`; laboratory result: 200–230 Mbit/s useful). Use a lower ceiling on weak machines.
2. The next speed reserve is on receive: it currently uses one worker. Parallel readers break sending as described above (`netlayer/transport.rs::data_listener` holds the encryption lock and processes packets serially). Separating socket reads from processing workers and removing per-wagon output could raise the safe ceiling.
3. **Channel loss:** with 2% loss on the path, an exit stream still breaks. One of 256 chunks of 16 KiB is lost, and the receiving server waits forever. This directly follows from “send and forget”: duplication reduces a wagon's loss probability from p to p², but not to zero. A possible option that preserves the principles is additional redundancy without feedback, such as one XOR parity wagon for every eight, about 12% extra traffic and roughly 500 times greater train reliability at 2% loss. **This is an owner decision and has not been implemented.**
4. Later work: reduce packet overhead, including one task per wagon and the lock around sending an entire train.

## Reproduction

```text
python3 chaos/speedtest.py DELAY_MS SPEED (for example 300mbit) MEGABYTES BINARY FOLDER
```

Run it inside the normal shell; the laboratory is created automatically. `CHAOS_NOLOG=1` discards node logs.
