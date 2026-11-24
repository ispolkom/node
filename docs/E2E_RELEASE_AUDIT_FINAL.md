# YANDI Node: final E2EE audit and release gate

> This document records the pre-remediation audit at commit `2abf631`. Its R16 failure table and 951-test count are historical. See [R16 remediation](R16_REMEDIATION.md) for the verified current branch state (962 Rust tests passed, 17 ignored). No merge or release has occurred.

Date: 24 November 2026. Baseline: `origin/main` at `c91a743`. Audit worktree: `codex/e2e-roadmap`. This report closes the current extended test cycle; it does not certify untested routes or change protocol design.

## Current confirmed findings

| Finding | Status and evidence | Release implication |
| --- | --- | --- |
| Pending outbox deletion after a failed key migration | **FIXED on this branch.** `ChatStorage::save_outbox` now opens and parses the existing encrypted queue with the current key before deleting or replacing it. The enabled `bug_r16_failed_outbox_migration_preserves_source_on_save` test verifies both empty deletion and nonempty overwrite are refused, the original ciphertext remains byte-for-byte intact, and a retry with the old key restores the pending entry. Run: `cargo test --locked --lib bug_r16_failed_outbox_migration_preserves_source_on_save`. | This specific data-loss blocker is cleared. A failed migration still requires recovery of the old key; the fix does not make unavailable keys recoverable. |
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

`deploy/yandi-node.prerm` stops services only for `remove|purge`; `deploy/yandi-node.postinst` restarts the existing user instance on an upgrade and does not reinitialize its identity. `deploy/yandi-node.postrm` deletes profile data only on explicit `purge`. The Desktop package's maintenance scripts update desktop metadata and do not wipe node data.

**PASS, binary/profile rehearsal:** The published `yandi-node_0.2.0-g919_amd64.deb` (SHA-256 `dadbb60e331a763d8e3546516dff0bcb68474a31c6a1348e2ac2fd6790d462af`) was extracted into a disposable test directory. Its binary created two synthetic node profiles. The nodes exchanged two messages, created a contact, and queued a message while the recipient was offline. The new branch's `yandi-node_0.2.0-g937_amd64.deb` (SHA-256 `38e52241071d24dbbc5f4e2d91ac016969de365c2a88fbfaf7b042fc1f4e55dd`) was also extracted. Its binary opened the existing profile in a private mount and network namespace. Login, trusted peer, both chat messages, contact and pending outbox passed the API probe. SHA-256 of nine original profile files, including configuration, identity, device seal, contacts, encrypted chat/status files and outbox, did not change. Reproduce the new-binary check with `unshare -Urnmp --fork bash tests/upgrade_profile_namespace.sh /path/to/disposable-test-root`, after preparing `stand/node1` and extracting the new package to `newpkg/` in that root. The script provides the original hardware fingerprint inputs inside the namespace so the existing sealed profile can open; it does not touch installed services or real profiles.

**NOT TESTED:** An actual `dpkg` transaction with systemd on a clean Debian or Ubuntu VM was unavailable in this isolated worktree. Therefore package lifecycle and restart behavior have source-review evidence, not an end-to-end install PASS. The published release artifacts, live nodes and `release/SEQ` were not changed.

## Final checks on this branch

- **PASS:** final `CARGO_TARGET_DIR=/mnt/ssd480/yandi-e2e-target cargo test --locked`: 31 suites, 951 passed, 0 failed, 28 ignored. This includes the formerly ignored outbox test, the invariant-document parser, crash-atomic-write checks, integration tests and doctests.
- **PASS:** the previously ignored outbox migration regression now runs normally and verifies preservation and recovery with synthetic keys.
- **PASS:** the cross-version profile probe in a private network and mount namespace, including API login, trusted peer, two chat messages, contact, pending outbox and unchanged SHA-256 for nine original files.
- **PASS:** Flutter's standard `flutter test` run completed with 336 tests. `flutter analyze --no-pub --no-fatal-infos --no-fatal-warnings` found no errors; strict analysis still exits nonzero because it reports 43 existing warnings/information items, unrelated to these Rust and documentation changes.
- **PASS:** `cargo test --locked --test invariants` after teaching its documentation parser to recognize the English `waiting — phase` status. The first full Rust run exposed this pre-existing translation mismatch; this test-only correction does not change the invariant or production behavior. The I2 guard text was also corrected to describe the existing E4/YSE1 trust boundaries.
- **PASS:** `bash -n` for the probe scripts and `git diff --check`.
- **STYLE CHECK:** `cargo fmt --all -- --check` exits nonzero on existing formatting differences across unrelated files (including `src/communication/file_transfer.rs`); this audit did not reformat unrelated production code.
- **NOT TESTED:** full Debian package transaction with systemd and a physical Android run of this experimental package. These require a separate isolated VM and device respectively.

The preceding standard suite did not run tests marked `#[ignore]`. A subsequent release-gate check exposed unresolved R16 defects; the branch is **not cleared for merge or release**. Publication as a fully verified DEB upgrade also requires the isolated `dpkg`/systemd transaction above. The developer's earlier physical PC↔phone calls remain valid field evidence for the artifacts used then.

### Classification of 28 ignored checks

Command: `CARGO_TARGET_DIR=/mnt/ssd480/yandi-e2e-target cargo test --locked --lib bug_r16 -- --ignored --test-threads=1`. Among 11 R16 checks, **3 passed**, **4 reproduced product defects**, and **4 are inconclusive because their setup or trigger failed**:

| R16 check | Result | Evidence / release relevance |
| --- | --- | --- |
| Locked clear history and clear all (R16-5), locked proxy save (R16-9) | PASS (3) | The protected operations behaved as expected in their synthetic fixtures. |
| Failed outbox persistence during lock (R16-10) | FAIL, confirmed | A queued message disappeared after a failed write and unlock. **Data-loss blocker.** |
| Locked chat list (R16-6) | FAIL, confirmed | `list_chats()` returned a correspondent ID without an unlocked key. **Privacy blocker.** |
| Rekey unlocks storage (R16-7) | FAIL, confirmed | `rekey_to()` made a previously locked store readable. **Lock-boundary blocker.** |
| Rekey removes a distinct plaintext identity (R16-8) | FAIL, confirmed | A distinct sole plaintext key pair disappeared. **Data-loss blocker.** |
| Lock during KDF, cancelled lock, setup retry after crash (R16-1/2/4) | INCONCLUSIVE | Their `setup()` call was rejected before the intended race because the fixture omitted the now-required 12/24-word seed phrase. |
| Failed migration must not publish an open session (R16-3) | INCONCLUSIVE | The fixture's migration-failure trigger did not reach `migration_pending`; the asserted condition was never reproduced. |

The other **17 ignored checks** comprise 14 opt-in tests that start real node processes, one child decoder helper invoked by another test, and two deliberately skipped proxy documentation examples. Selected real-process tests were run earlier in this audit, but the default suite does not establish a pass for all 14. None of these 17 is a confirmed current blocker on the available evidence; that does not erase the four confirmed R16 blockers above. The normal suite's 951 PASS result must be read together with this classification.

Read-only merge check: GitHub `main` was `c91a743` when checked and is an ancestor of this experimental branch, so that ref can fast-forward. The separate local `main` ref is `832814f` and has diverged; `git merge-tree main HEAD` reports a conflict in `release/SEQ`. No merge or conflict resolution was performed, and the release sequence was not edited.

## Release gate and branch inventory

1. The failed-key-migration `outbox.enc` deletion is fixed and its regression test is enabled. **A separate R16-10 outbox loss on a failed persistence operation remains confirmed.** The synthetic populated-profile binary upgrade passed; the full package transaction remains untested here.
2. Public E2EE claims are now path-specific: paired devices have E4, PC nodes have YSE1, and an owner's node reads a phone's message to a remote ordinary contact. First-contact TOFU remains a trust limitation and needs independent fingerprint verification.
3. The developer reported successful physical PC↔phone calls and earlier screen-off calls/messages. Those observations are retained as field evidence, not relabeled as a test of this experimental package. No further exploratory matrix is required to close this audit.

Experimental commits: `8775888` roadmap and synthetic E4 test; `126268f` TOFU witness; `5f8d1da` corrected node boundary test; `09c97ac` test-fixture compile fix; `98b60bf` YSE1 through UDP relay sockets; `25649e4` relay outage/recovery; `2fe2615` communication/proxy separation; `ec5fac3` audit release gates; `b13baef` outbox preservation; `d1e3f0d` upgrade probe and invariant parser; the final documentation commit states public trust boundaries and test evidence. No production protocol, cryptographic format, release key, `release/SEQ`, or `main` merge was changed. An unrelated generated Flutter registrant modification remains unstaged in the worktree and is not part of these commits.
