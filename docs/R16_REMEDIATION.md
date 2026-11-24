# R16 remediation and release gate

Date: 24 November 2026. Branch: `codex/e2e-roadmap`. All tests use synthetic identities and temporary profiles. This document supersedes the R16 findings in `E2E_RELEASE_AUDIT_FINAL.md`.

## Confirmed defects and corrections

| Finding | Correction | Evidence |
| --- | --- | --- |
| R16-10: failed outbox write followed by lock dropped the only in-memory queue | `ChatManager` seals an encrypted in-process snapshot before clearing plaintext. Unlock restores it and marks the queue dirty so a subsequent successful disk write persists it. Queue mutation and the final write are serialized with the pending-ack mutex. | `bug_r16_lock_preserves_outbox_when_persist_fails` reproduces the write failure, checks lock revocation, restores the message, removes the obstruction, and verifies disk persistence. |
| R16-6: locked chat storage exposed correspondent identifiers | `list_chats` requires an available storage key. | `bug_r16_locked_list_chats_refuses_metadata`. |
| R16-7: rekey implicitly unlocked a locked store | `rekey_to` preserves the locked state; unlocking is a separate explicit operation. | `bug_r16_rekey_does_not_unlock_storage` and the updated migration recovery tests. |
| R16-8: rekey erased a different plaintext device identity | Rekey compares both key pairs, rejects conflicts, and leaves both files untouched. An unreadable encrypted identity cannot be overwritten by a plaintext copy. | `bug_r16_rekey_preserves_distinct_plain_identity` and the existing identity migration tests. |
| R16-2: cancelling a lock task could permanently skip consumer key revocation | Consumer revocation runs in a spawned task that continues if the caller is cancelled. New installs wait for any pending revocation before issuing keys. | `bug_r16_cancelled_lock_must_revoke_consumers` pauses the consumer, aborts the caller, resumes it, and checks revocation. |
| R16-3: failed migration published an open owner session | `ClosedEnv` publishes its vault only after migration and consumer installation complete. A migration error returns an error, leaves the marker for retry, and leaves the owner session closed. Active service consumers are restored to their service key. | `bug_r16_failed_migration_must_not_publish_open_session` checks the session while migration is paused and after failure; `unfinished_migration_is_retried_and_lock_wins_over_late_unlock` checks retry. |

R16-1 was a test expectation error: a lock during KDF correctly cancels the old unlock, which returns an error. R16-4 needed a valid synthetic seed phrase before it could exercise interrupted setup; it passes. R16-5 and R16-9 already passed. All 11 R16 checks are now enabled in the ordinary test run.

## Verification

Run from this branch:

```sh
CARGO_TARGET_DIR=/mnt/ssd480/yandi-e2e-target cargo test --locked -- --test-threads=4
git diff --check
```

Result: 31 Rust suites, **962 passed, 0 failed, 17 ignored**. The 17 ignored checks are opt-in process/network tests, one child decoder helper, and two proxy documentation examples. Flutter code was not changed in this remediation; its earlier 336-test result belongs to the preceding audit, not a new Flutter run.

## Remaining limits and release decision

- The encrypted outbox snapshot protects a lock/unlock cycle while a disk write is failing. It is in process memory: a process crash before a successful disk write can still lose those unsaved messages. No software can promise disk durability while all writes fail. The test proves recovery after disk access returns, not crash survival during disk failure.
- Async consumer revocation is guaranteed to continue after caller cancellation, but a consumer that never completes its own `lock()` can delay revocation. The owner vault is removed synchronously before that work begins.
- The populated synthetic profile passed the earlier binary upgrade rehearsal, but a real `dpkg`/systemd upgrade on clean Debian or Ubuntu remains **NOT TESTED**. The developer's earlier physical PC and phone calls apply to previous artifacts, not this branch.
- The branch remains separate from `main`; no released package, signing key, `release/SEQ`, published site, or live profile was changed. Review the branch diff and complete the isolated package transaction before declaring a stable release.
