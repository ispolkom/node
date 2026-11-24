# YANDI Node: final E2EE audit and release gate

Date: 24 November 2026. Baseline: `origin/main` at `c91a743`. Audit worktree: `codex/e2e-roadmap`. This report closes the current extended test cycle; it does not certify untested routes or change protocol design.

## Current confirmed findings

| Finding | Status and evidence | Release implication |
| --- | --- | --- |
| Pending outbox can be deleted after a failed key migration | **FAIL**, reproducible with `CARGO_TARGET_DIR=/mnt/ssd480/yandi-e2e-target cargo test --locked --lib bug_r16_failed_outbox_migration_preserves_source_on_save -- --ignored`. `ChatStorage::rekey_to(None, new)` leaves an old encrypted `outbox.enc` unchanged and returns one failed file. Subsequent `ChatStorage::save_outbox(&[])` removes it. The test uses a temporary directory and synthetic keys. | **Blocker for any release that invokes this migration or claims lossless recovery from a missing old key.** Fix and enable the regression test before such a release. This test does not demonstrate loss during an ordinary `.deb` upgrade. |
| First-contact TOFU can pin a substituted key | Confirmed by the Flutter test `first contact TOFU accepts the first supplied key without external proof`; later genuine key is refused. Already pinned keys reject changes. | A trust-model limitation, not a demonstrated break of established E4 sessions. Do not promise authenticated first contact without out-of-band fingerprint comparison. Decide whether verification must be mandatory before sensitive first messages. |
| Phone to ordinary remote contact passes plaintext through the owner's node | Confirmed by `src/mobile_api.rs::send` and the WebSocket message branch, which hand text to `ChatService`. The node then seals node-to-node traffic with YSE1. | The owner node is inside this path's trust boundary. A claim of phone-to-phone E2EE for this path would be inaccurate. Product trust policy must be stated before changing it. |

## Historical findings and existing protections

- The prior claim that a UDP relay reads ordinary PC-to-PC chat is incorrect for the inspected path. `ChatService::send_once` applies YSE1 before transport; the recipient-bound test and real UDP relay socket test pass. The relay cannot open the tested content with its own key.
- The `communication/encryption.rs` and `p2p/encryption.rs` stubs fail closed, but they are not the active PC chat encryption path. Treating these stubs alone as proof that PC chat is plaintext would be a false positive.
- Earlier plaintext transport acceptance, call signal replay, key rekey, and Android background LIVE issues have code and regression tests in the baseline. This audit found no new reproducible failure in those paths. Their historical descriptions do not establish a current release blocker without a matching failure on this baseline.
- The previously reported physical PC↔phone calls were successful according to the developer. The developer also reported successful calls and messages with the screen off in an earlier build. These are valuable field observations, not a fresh instrumented test of every current artifact. `docs/history/STABILITY_REPORT.md` describes older builds and should not override the newer developer observations.

## Completed checks and limits

- **PASS:** `cargo test --locked --test e2e_boundary_baseline` on isolated target data: YSE1 content opens for the recipient and not a distinct intermediary.
- **PASS:** production UDP relay socket tests `a_sealed_chat_message_crosses_relay_without_exposing_content` (three consecutive passes) and `relay_outage_drops_datagrams_and_recovery_restores_forwarding`. The second test verifies forwarding restoration, not application outbox delivery.
- **PASS:** `cargo test --locked --test testnet_comms_test -- --ignored`: two real local node processes exchange chat, file and call signalling under its assertions (67.86 s).
- **PASS:** local proxy tests: 12 `hops::tests`, 4 `hops_net::tests`, 14 `socks5::`, 1 `route_rules::tests`. Proxy and messenger guarantees are separate.
- **PASS:** synthetic E4 relay tests and the Flutter E2E security test were run earlier in this branch's cycle. The Flutter first-contact TOFU witness confirms the limitation above.
- **NOT TESTED:** a forced UDP relay route among three separate node processes, route switch during an outage, preservation of a real queued chat during that outage, physical Android lifecycle on the current artifacts, and real proxy exit/hops routes in a private network. The stock testnet binds every process to loopback and does not force the relay. Proxy integration fixtures bind an echo server to the host's public IP, so they were not run during this isolated audit. These gaps are not observed failures.

## Update and data preservation

`deploy/yandi-node.prerm` stops services only for `remove|purge`; `deploy/yandi-node.postinst` restarts the existing user instance on an upgrade and does not reinitialize its identity. `deploy/yandi-node.postrm` deletes profile data only on explicit `purge`. The Desktop package's maintenance scripts update desktop metadata and do not wipe node data. Thus source inspection supports data retention on an ordinary package upgrade. **A before/after upgrade of a populated profile with the exact release packages was not performed here (NOT TESTED).** The failed outbox migration above is a separate, confirmed data-loss condition and prevents a blanket data-preservation guarantee.

## Release gate and branch inventory

1. Fix the confirmed outbox migration failure and run its formerly ignored regression test. For a release that cannot invoke key migration, explicitly verify that scope and still perform a populated-profile upgrade rehearsal before claiming data preservation.
2. Make public E2EE claims path-specific: paired devices have E4, PC nodes have YSE1, and an owner's node reads a phone's message to a remote ordinary contact. State the first-contact TOFU limitation.
3. Record the exact physical-device build and outcome already tested by the developer; perform only the focused upgrade rehearsal needed for the release candidate. No further exploratory matrix is required to close this audit.

Experimental commits: `8775888` roadmap and synthetic E4 test; `126268f` TOFU witness; `5f8d1da` corrected node boundary test; `09c97ac` unrelated test-fixture compile fix; `98b60bf` YSE1 through UDP relay sockets; `25649e4` relay outage/recovery; `2fe2615` separates communication and proxy verification. No production protocol, cryptographic format, release key, `release/SEQ`, or `main` merge was changed. An unrelated generated Flutter registrant modification remains unstaged in the worktree and is not part of these commits.
