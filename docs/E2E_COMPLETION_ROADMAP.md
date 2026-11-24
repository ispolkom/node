# End-to-end encryption completion plan

Status: in progress. Baseline: `c91a743` (24 November 2026). This plan does not change a wire format or promise that every path is already end-to-end encrypted.

## Verified boundaries

| Path | Current state | Evidence | Remaining work |
| --- | --- | --- | --- |
| Paired phone ↔ paired phone and phone ↔ own PC | Signed E4 envelope with X25519, AEAD, recipient binding and replay ledger | `mobile/yandi_mobile/lib/crypto/e2e_crypto.dart`, `src/mobile_self.rs`, `tests/mobile_e4_reference.py` | Test a hostile intermediary on real processes; first-contact TOFU accepts the first supplied key without external proof (regression witness in `test/e2e_security_test.dart`); verify keys out of band and test crash consistency of replay acceptance |
| PC node ↔ another person's PC node | Transport session encryption; application `ChatMessage` is serialized as JSON before that transport | `src/communication/chat.rs::send_once`, `src/communication/types.rs::ChatMessage` | Device keys independent of routing node; authenticated E2E envelope for messages and attachments, with a versioned migration |
| Phone ↔ ordinary remote contact | The mobile API can submit ordinary text to the owner's node | `src/mobile_api.rs::send` | Replace this path with an end-to-end client-to-client protocol; do not silently claim E4 covers it |
| Group messages and calls | Group keys and E4 call signalling exist for paired devices | `docs/CLIENT_WIRE.md`, `mobile/yandi_mobile/lib/crypto/group_crypto.dart` | Test hostile key distribution, membership changes, and media identity binding on real devices |

## Work sequence and acceptance gates

1. **Baseline and threat model.** Run synthetic sender, receiver and relay tests. Prove what each relay can read and change. Record distinct outcomes for paired devices, ordinary contacts, files, groups and calls. Gate: repeatable commands and retained redacted evidence.
2. **Identity verification.** Define how independently owned devices authenticate one another before first use and after key rotation. Preserve existing paired device IDs. Gate: a malicious first-contact node cannot substitute a key without a visible verification failure; recovery and revocation are tested.
3. **PC ↔ PC and phone ↔ ordinary contact.** Add a versioned, signed recipient-bound client envelope. Keep node routing and delivery semantics. Gate: three-node stand shows delivery while relay cannot recover plaintext; tamper, replay, wrong sender and wrong recipient fail closed. Legacy data remains readable by its original software until an explicit migration is complete.
4. **Files, receipts and edits.** Encrypt all content and metadata that should be private at the endpoints, and bind attachments to the message identity. Gate: intermediary cannot recover filenames or file keys; retries and crash recovery do not lose or duplicate accepted content.
5. **Groups and calls.** Verify member keys, rekey after removals, and bind the media channel to verified call signalling. Gate: removed members cannot read new content and a hostile relay cannot substitute a call endpoint.
6. **Migration and release.** Dual-version testing, downgrade refusal after migration, physical Android Doze tests, PC background-call tests and independent review. Gate: update documentation/site claims only after all supported communication paths pass.

No live keys or owner data may enter the stand. Keep `release/SEQ`, signed artifacts and production nodes untouched. A PASS applies to its exact scenario only. Physical device behavior remains NOT TESTED until exercised on the devices.
