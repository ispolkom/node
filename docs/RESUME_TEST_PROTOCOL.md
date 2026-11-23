# Stage 2a verification protocol (removal of RESUME and secrets)

Implementation commit: `9689280` (branch `claude/fix-rev5-6`). Machine: developer workstation (Linux 6.1, x86_64), stable `cargo`/`rustc` from `~/.rustup`. Date: 2026-10-18.
This document records only what was **actually tested**; everything else is marked “not tested”.

> **Independent review result (Sol, round 36, `ANALYSIS16.md`): stage 2a was NOT accepted. Fixes P16-1…10 were made in `f98f1fd`; the follow-up review (round 37) accepted them partially and found P17-1…10; the P17 fixes are in `7fe702` and await Sol's follow-up. Tests: 780 `cargo test --lib` tests passed.** Network RESUME was removed, but 10 issues (two high severity) were found: nested-field cleanup, read/write races, secrets retained in `.before-restore-*` files after errors, `--anchor-store` missing from backups, and weak test oracles. The “cleaned” claims below apply to the tested cases, not as a general guarantee; the next fix is still pending.

## 1. Automated tests (run)
| Command | Result |
|---|---|
| `cargo test --lib` (on `9689280`) | 764 passed, 0 failed |
| `cargo test --lib netlayer::pairing` | 10 passed: secret-free grant records; v2 reading; migration of old client and anchor files (including 128-bit session numbers); v2 with an old secret field is rejected without changing the file; corrupt/future/unreadable/link files are not replaced by empty state; copy cleanup; source guard (`PKT_RESUME`, `SessionToken`, `new_with_session_key`, and related symbols are absent) |
| `cargo test --lib node_backup` | 21 passed, including `old_resume_secrets_never_enter_a_backup_or_survive_a_restore` (the secret is not copied, restored, or left in the pre-restore file; an unparsed pairing file is not copied) |
| `cargo test --test robustness --test hello_replay_test --test hello_adversarial_test` | passed (apart from the test marked `ignore` in robustness) |

## 2. Tests with real processes (training network `yandi testnet`, 2–3 copies on one machine; run)
Command: `cargo test --offline --test <name> -- --ignored`; the binary was built from `9689280`.
| Test | What it checks | Result |
|---|---|---|
| `testnet_test` | two copies discover and see each other, survive a restart, and leave no state behind | passed, 88 s |
| `testnet_comms_test` | bidirectional messages, byte-for-byte file transfer, call signals, rejection of unencrypted forgery, and encrypted packets only in logs | passed, 104 s |
| `testnet_rekey_test` | key replacement in a running network without losing messages | passed, 141 s |

## 3. Explicitly not tested
- Physical devices (two phones, a PC node, real networks, and IP/interface changes).
- Routing through three and four nodes; loss and recovery during an active transfer; parallel connections; restarting one node mid-exchange (apart from the restart in `testnet_test`).
- WS connection of the lightweight client to an anchor (`--anchor-url`/`--anchor-fp`): no automated test exists and it was not run manually. The path uses the ordinary signed Hello; its cost was not measured separately.
- Calls on real devices.
- State-file theft as a separate experiment: only the absence of key material from v2 files and cleanup of old files was checked (sections 1–2); possessing `paired_*.json` cannot restore a session because resume code no longer exists.
- Independent review of the Sol (Codex) implementation — not yet performed.

## 3a. What the stage 2a tests do and do not prove (P18-7, P19)
Failure injection was tested (replacement and write failures during rollback are triggered deliberately): rollback restores what a concurrent writer wrote before replacement; a pairing file created by restore is removed during rollback; a failed rollback appears in warnings and in the “files changed” flag; an omitted pairing file is named in the report; the shell shows warnings after a post-write failure.
**Not tested:** directory `fsync` failure (EIO) and power loss, abrupt termination (`kill`) during a write, two processes that do not take the pairing-file lock (the lock binds YANDI code only; the node must be stopped during restore), non-Unix behavior, and unusual filesystems. A green read test does not imply the same for cleanup and restore. See `docs/AUDIT38_TRIAGE.md`.

## 4. What stage 2a does NOT cover
Hardening the handshake itself (binding `HelloAck` to the request, key confirmation, liveness when a recorded Hello is replayed, budgeting before signature verification, the “paired clients only” policy, and key wiping) belongs to stage 2b; findings P15-3…7 and P15-12 remain open.

## 5. Cleanup guarantee boundaries (P17-1)
The store guarantees that known secret fields from the old format are removed and that only the strict v2 schema with validated public-field shapes is accepted (the address must be `wss://host[:port]/`). The store **cannot** recognize a secret hidden in an allowed field that has the correct public shape (for example, 64 hex characters in a node name or `anchor_x25519_hex`): cryptographic validity of a public key comes from its signed binding to an identity, not from shape validation. Older external backups, SSD/COW blocks, and snapshots are outside the scope of cleanup. Non-Unix platforms have no inter-process store lock (only an in-process lock works), so running multiple processes against one directory is unsupported there.
