# End-to-end encryption completion plan

Status: in progress. Baseline: `c91a743` (24 November 2026). This plan does not change a wire format or promise that every path is already end-to-end encrypted.

## Verified boundaries

| Path | Current state | Evidence | Remaining work |
| --- | --- | --- | --- |
| Paired phone ↔ paired phone and phone ↔ own PC | Signed E4 envelope with X25519, AEAD, recipient binding and replay ledger | `mobile/yandi_mobile/lib/crypto/e2e_crypto.dart`, `src/mobile_self.rs`, `tests/mobile_e4_reference.py` | Test a hostile intermediary on real processes; first-contact TOFU accepts the first supplied key without external proof (regression witness in `test/e2e_security_test.dart`); verify keys out of band and test crash consistency of replay acceptance |
| PC node ↔ another person's PC node | Authenticated YSE1 recipient envelope inside endpoint transport encryption | `src/communication/chat.rs::send_once`, `src/crypto/sealed.rs`, `tests/e2e_boundary_baseline.rs` | Verify offline key discovery, continuity, and recovery; test forced relay routing |
| Phone ↔ ordinary remote contact | Phone submits text to its owner's node; node seals it for the remote node with YSE1 | `src/mobile_api.rs::send`, `src/communication/chat.rs::send_once` | Decide if trust in the owner's node is acceptable; device-to-device E2E requires a separate client envelope |
| Group messages and calls | Group keys and E4 call signalling exist for paired devices | `docs/CLIENT_WIRE.md`, `mobile/yandi_mobile/lib/crypto/group_crypto.dart` | Test hostile key distribution, membership changes, and media identity binding on real devices |

## Work sequence and acceptance gates

1. **Baseline and threat model.** Run synthetic sender, receiver and relay tests. Record distinct outcomes for paired devices, ordinary contacts, files, groups and calls. The production YSE1 test passes; a live three-process stand delivered a message from node 1 to node 3, but no forced relay forwarding was observed on node 2. Live relay content isolation remains NOT TESTED. Gate: repeatable commands and retained redacted evidence.
2. **Identity verification.** Define how independently owned devices authenticate one another before first use and after key rotation. Preserve existing paired device IDs. Gate: a malicious first-contact node cannot substitute a key without a visible verification failure; recovery and revocation are tested.
3. **PC ↔ PC and phone ↔ ordinary contact.** Preserve the existing signed YSE1 node envelope. Specify key verification and decide whether phone ↔ remote contact needs device-to-device encryption. Gate: a forced relay route shows delivery without exposing plaintext; tamper, replay, wrong sender and wrong recipient fail closed. Legacy data remains readable until an explicit migration is complete.
4. **Files, receipts and edits.** Encrypt all content and metadata that should be private at the endpoints, and bind attachments to the message identity. Gate: intermediary cannot recover filenames or file keys; retries and crash recovery do not lose or duplicate accepted content.
5. **Groups and calls.** Verify member keys, rekey after removals, and bind the media channel to verified call signalling. Gate: removed members cannot read new content and a hostile relay cannot substitute a call endpoint.
6. **Migration and release.** Dual-version testing, downgrade refusal after migration, physical Android Doze tests, PC background-call tests and independent review. Gate: update documentation/site claims only after all supported communication paths pass.

No live keys or owner data may enter the stand. Keep `release/SEQ`, signed artifacts and production nodes untouched. A PASS applies to its exact scenario only. Physical device behavior remains NOT TESTED until exercised on the devices.

## Relay evidence (24 November 2026)

- PASS: `CARGO_TARGET_DIR=/mnt/ssd480/yandi-e2e-target cargo test --locked --lib a_sealed_chat_message_crosses_relay_without_exposing_content`. The test uses the production UDP relay forwarding loop on four relay sockets, production YSE1 and an endpoint transport session. The receiver gets the original chat message through the relay sockets; the intermediary's signing key cannot open YSE1; transport rejects replay of the forwarded datagram. The relay forwarding loop copies the endpoint ciphertext without opening it.
- PASS: `CARGO_TARGET_DIR=/mnt/ssd480/yandi-e2e-target cargo test --locked --test e2e_boundary_baseline`. A distinct intermediary key cannot open a sealed node message.
- PASS: three real node processes exchanged a synthetic marker from node 1 to node 3, with node 2 running as a relay and nodes 1 and 3 in client mode. Delivery was observed in node 3's history. No relay allocation or forwarding was observed in node 2's log for this message, so the selected path may have been direct on loopback.
- NOT TESTED: forced relay routing between separate node processes in isolated network namespaces, hostile relay packet modification in the live route, Android physical lifecycle, and end-to-end confidentiality between a phone and a remote contact's phone.

The first relay socket test run dropped the sender's datagram before the receiver address was pinned. The test now waits for the receiver's pin datagram to be processed before sending. This was a stand setup race, not evidence of lost production chat traffic.

## Follow-up results and decisions pending

- PASS: `CARGO_TARGET_DIR=/mnt/ssd480/yandi-e2e-target cargo test --locked --lib relay_outage_drops_datagrams_and_recovery_restores_forwarding`. With the forwarding task stopped, a datagram is not delivered. A fresh relay allocation restores forwarding. This checks the UDP relay mechanism, not the application outbox or a live node route switch.
- NOT TESTED: preservation and eventual delivery of a chat message across a relay outage between three separate node processes. The training network binds all three nodes to `127.0.0.1`; its client mode does not enforce use of node 2, so loopback delivery cannot establish this property.
- Confirmed limitation: the Flutter TOFU test in `mobile/yandi_mobile/test/e2e_security_test.dart` shows that a substituted first key is pinned and a later genuine key is rejected. Key continuity after pinning works, but first-contact identity is not authenticated independently. Fingerprints already exist in chat for manual comparison (`docs/CLIENT_WIRE.md`). A product decision is needed on whether first-contact verification must be mandatory before sending sensitive content.
- Confirmed trust boundary: `src/mobile_api.rs::send` and the WebSocket message path pass plaintext from a phone to its owner's node for a remote contact. YSE1 then protects node-to-node delivery. Whether the owner's node is an allowed endpoint must be decided explicitly before designing an additional phone-to-phone envelope.
