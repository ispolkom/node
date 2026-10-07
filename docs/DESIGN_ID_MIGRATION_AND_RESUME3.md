<!-- Design proposal produced by a read-only analysis of the code on 2026-10-07; nothing here is implemented yet. -->
DESIGN: bound identity migration and fresh-key session resume

All changes below are proposed. The repository remains unchanged.

1. Existing behavior and implementation constraints

Identity

- New identities already derive their address from the Ed25519 signing public key. Loaded identities retain their stored address; loading does not convert a legacy address. See src/core/identity.rs:167, src/core/identity.rs:370 and src/core/identity.rs:462.
- The bound address is SHA-256(signing public key)[0..28] followed by `59 42 31 00`. Untagged IDs are accepted only before 2027-04-01 00:00:00 UTC, and `YANDI_REQUIRE_BOUND_IDS` disables that acceptance earlier. `id_acceptable` itself does not implement first-claim pinning. See src/util/types.rs:148, src/util/types.rs:151 and src/util/types.rs:171.
- `NodeName` is the complete SHA-256 of the signing key, a different value from the tagged node ID. It remains unchanged during migration. See src/util/types.rs:71 and src/core/identity.rs:186.
- Editing an encrypted identity’s address in place is unsafe. The legacy fallback passphrase incorporates its first 16 address bytes; the key-root v3 format authenticates the address as AEAD associated data. Both formats require opening and resealing the identity. See src/core/identity.rs:99, key_root/src/identity_store.rs:184 and key_root/src/identity_store.rs:73.
- Unix identity loading uses key_root. Its loader refuses initialization when another port’s identity artifacts exist, and it fails rather than replacing an unreadable identity. Migration must use the existing port and key directory. See src/core/identity.rs:480 and key_root/src/identity_store.rs:310.
- The generated identity’s stored X25519 private bytes are generated separately from the ephemeral secret producing its public key. Migration must preserve these bytes and must not introduce a new public/private consistency requirement that would reject such identities. Use fresh X25519 keys for RESUME. See src/core/identity.rs:159–175.

Persistent and runtime references

- Trusted peers contain the node ID, signing key, names and addresses. Applying that directory rebuilds the exit-policy trusted set and transport pins. See src/web/peers.rs:30 and src/web/peers.rs:255.
- Web contacts currently contain a UUID, name and short ID, without a signing-key binding. These are insufficient evidence for automatic identity migration. See src/web/server.rs:1928.
- Chat encryption depends on the local node ID, even with a master key. History filenames use the remote full ID, with an opportunistic rename from an eight-byte-prefix filename. Decryption failures are silently skipped. See src/communication/storage.rs:82, src/communication/storage.rs:103 and src/communication/storage.rs:192.
- Chat messages contain `from` and `to`; groups contain `created_by` and an ID-keyed membership map. These need identity resolution, beyond filename changes. See src/communication/types.rs:19 and src/communication/groups/group.rs:125.
- Avatar filenames use short IDs. Reciprocity accounting uses full-ID hex keys. See src/communication/profile_manager.rs:65, src/communication/profile_manager.rs:99 and src/reciprocity.rs:48.
- Offers sign their node ID and other fields, pin keys by ID, and maintain ID-keyed reachability state. ViaRecords also sign their node ID and relay IDs, but their store can retain records from multiple keys for an untagged ID. See src/network_offers.rs:85, src/network_offers.rs:133, src/network_offers.rs:151, src/relay_net.rs:52 and src/relay_net.rs:104.
- Pairing clients are keyed by signing public key, but tokens may remember an exact node ID. Paired anchors are found and updated by exact anchor ID. The QR payload has an X25519 key and TLS fingerprint, without an Ed25519 signing key. See src/netlayer/pairing.rs:50, src/netlayer/pairing.rs:76, src/netlayer/pairing.rs:337 and src/netlayer/pairing.rs:420.
- Existing TLS certificates are loaded from fixed filenames, irrespective of the supplied node ID. Preserve them to preserve phone fingerprints. See src/netlayer/tls_cert.rs:49.
- Transport, DHT and background publishers capture the identity’s ID at initialization. Changing one live identity field would leave inconsistent components. See src/netlayer/transport.rs:609, src/netlayer/transport.rs:707, src/main.rs:662 and src/main.rs:678.

RESUME

- `restore_session` installs the persisted master again. Installation constructs fresh session crypto with counter zero and an empty replay window. Directional traffic keys depend on the master and ID ordering; the random five-byte nonce salt does not change those keys. See src/crypto/handshake.rs:112, src/crypto/store.rs:80 and src/crypto/session.rs:125, src/crypto/session.rs:166.
- Replay checking considers the packet counter, not its salt. Thus captured old ciphertext can authenticate after restoration, while restarted counters can collide with a surviving receiver’s replay window. See src/crypto/session.rs:211.
- Legacy RESUME authenticates only `session_id || address`. It contains no challenge and does not authenticate the embedded node ID. Its encoder truncates addresses to 255 bytes, and its decoder permits trailing data. See src/netlayer/pairing.rs:160, src/netlayer/pairing.rs:194 and src/netlayer/pairing.rs:207.
- The WebSocket fast path checks the token owner’s key/ID binding and remembered ID, then refreshes the token and installs the persisted traffic master before sending ACK. See src/netlayer/transport.rs:1009 and src/netlayer/transport.rs:1034.
- `handle_resume_packet` checks embedded ID against the authenticated sender, but its token lookup verifies only the MAC; it omits the fast path’s owner-key and remembered-ID checks. It can therefore apply another token’s master to that sender. See src/netlayer/transport.rs:5400, src/netlayer/transport.rs:5428 and src/netlayer/transport.rs:5472.
- The WS client installs the old master before receiving ACK. It accepts a decoded plaintext success ACK after decryption fails, and its rejection path asks for reconnection without invalidating the resume candidate. See src/netlayer/transport.rs:4571, src/netlayer/transport.rs:4599 and src/netlayer/transport.rs:4649.
- Existing in-memory rekeying retains old crypto objects for 30 seconds. That is safe only when their replay windows survive. See src/crypto/store.rs:17 and src/crypto/store.rs:86.
- File-transfer keys derive from the session master, so changing that master also affects continuing encrypted transfers. See src/crypto/handshake.rs:122.
- The Flutter app has a separate token-authenticated `/mobile/ws` connection. It must not be confused with the Rust transport’s pre-Hello binary RESUME path. See mobile/yandi_mobile/lib/services/ws_service.dart:102.

2. Shared identity model

Introduce an IdentityRegistry with three distinct concepts:

- Stable principal: Ed25519 signing public key.
- Canonical address: `derive_node_id(signing_public_key)`.
- Wire address: the particular old or bound ID authenticated for a connection.

Application authorization, trust, chat ownership, pairing and accounting resolve to the stable principal. Network routing retains exact wire addresses.

Do not globally replace HashId values. Group IDs, message IDs, file IDs and content hashes are not node addresses.

Persist:

- `identity_aliases`: old ID, bound ID, signing key, migration statement, evidence establishing the old key binding, and verification status.
- Local identity metadata: original ID, canonical ID, preferred outbound ID, immutable storage context, migration generation and phase.
- Durable first-claim bindings for authenticated legacy peers. Conflicts cannot silently replace them.
- Per-principal protocol minimums, including whether RESUME3 has succeeded.

Resolution must require a verified key binding. A short prefix alone never establishes an alias.

3. CHANGE 1: migration protocol

3.1 Signed statement

Define IdentityMigrationV1 as fixed canonical binary data:

- Domain: ASCII `yandi-id-migration-v1` followed by NUL.
- Network: u16 big-endian byte length, then UTF-8 network name; maximum 64 bytes.
- Version: u8, value 1.
- Old ID: 32 bytes.
- New ID: 32 bytes.
- Signing public key: 32 bytes.
- Sequence: u64 big-endian, value 1 for this one-way address conversion.
- Issued time: u64 Unix seconds.
- Legacy wire expiry: u64, exactly `LEGACY_ID_SUNSET`.
- Signature: Ed25519, 64 bytes over every preceding byte.

Reject trailing bytes, unknown versions, invalid key encodings, tagged old IDs, equal IDs, and a new ID different from `derive_node_id(key)`.

The statement establishes an irrevocable equivalence, not a revocable instruction to discard the old address. Rollback changes routing preference; it does not revoke identity continuity.

Transport:

- Add signed Hello capability `IDENTITY_MIGRATION = 0x1000`.
- Send the statement over an authenticated control channel after Hello, on both node and chat transports.
- For post-sunset recovery, add a bounded pre-Hello proof envelope containing the statement followed by a signed Hello under the bound ID.
- Publish the statement at the old-ID discovery lookup and attach it to versioned card/alias responses.
- Relay or gossip delivery is allowed because the statement is independently signed. Forwarding does not grant authority.

Hello currently signs capabilities and IDs, so capability stripping changes the signature. See src/netlayer/packet.rs:913.

3.2 Peer verification and storage

A peer automatically applies the statement only when:

1. Its signature and new-ID derivation verify.
2. The old ID is already bound to that exact signing key through a durable pin, authenticated first claim, or authoritative owner-imported/bootstrap evidence.
3. Neither address conflicts with another established principal.
4. The statement matches any previously accepted conversion for that old ID.

A signature alone cannot prove ownership of an arbitrary random old ID.

For previously unknown old IDs, before sunset an authenticated old-ID Hello can establish the ordinary first claim. Apply the statement afterward. After sunset, an unknown old ID cannot acquire authority merely by presenting this statement.

Persist the statement and binding evidence before updating application references. Receiving the same statement repeatedly is idempotent. Conflicting statements are quarantined and leave existing relationships unchanged.

A contact containing only a short ID switches automatically only after that prefix is uniquely resolved to an authenticated, pinned full ID/key. Otherwise retain the contact and historical lookup; do not guess.

3.3 Exact data changes

Local identity:

- Change the active stored address to the derived bound ID.
- Preserve signing seed, signing public key, X25519 stored bytes, root key, password selection and creation time.
- Reseal private material with fresh encryption randomness.
- Preserve the original identity as a private recovery artifact.
- Persist original address, preferred address, storage context and migration statement separately from legacy identity formats.

References to the migrating principal:

- Trusted peers: canonical ID plus alias, same key, same names and addresses.
- Contacts: retain UUID and user name; add full ID, signing key and old short-ID alias; change displayed short ID after verified resolution.
- Paired anchors: change canonical anchor ID, preserve URL, preference, TLS fingerprint and tokens; retain old wire-ID alias.
- Paired clients: preserve signing-key map keys. Replace exact-ID authorization with verified principal/alias authorization.
- Pins and exit-policy trust: both wire IDs resolve to one authorized principal until wire sunset.
- Chat history: preserve original storage-key context and expose both peer IDs as one conversation.
- Avatars: maintain a principal-to-existing-file mapping, including old short-ID names.
- Groups: resolve creator/member identities through aliases; do not change group or message IDs.
- Accounting: combine distinct old/new ledger entries with saturating addition in one transaction; do not reset quotas.
- Runtime peers, connections, relay registrations, admission/quota keys, pending deliveries and transfer ownership: resolve to principal while retaining exact connection wire IDs.
- DHT routing: rebuild around the preferred address; retain an old-address migration lookup. Key-derived NodeName records retain their identity. NodeRecord uses NodeName, not the tagged address. See src/dht/record.rs:18.
- Offers and ViaRecords: generate freshly signed records for the bound address. Never edit signed records in place. Treat old records as separate immutable objects until expiry.
- Relay IDs inside other nodes’ signed ViaRecords: resolve verified aliases at routing time and request fresh records from their authors.
- Cards: keep `YANDI-NODE-1` structurally unchanged for old software. Add `YANDI-NODE-2` carrying the bound card and statement. The current card parser rejects unknown fields. See src/web/peers.rs:127.
- TLS: preserve certificate and private key; a stale certificate CN is preferable to breaking fingerprint pairing.

For chat storage, choose preservation over immediate bulk re-encryption:

- Record `storage_context_id = original_local_id`.
- Continue deriving existing chat encryption keys with that immutable context.
- Add an explicit key/context identifier for future formats.
- Use a conversation manifest to associate old and new peer filenames with one principal.
- Preserve historical `from`/`to` bytes; resolve them for display and authorization.
- Merge separate histories by message ID, preserving originals and deterministic status precedence.
- Never adopt a short-name history when more than one known peer matches its prefix.

Any later compaction/re-encryption is a separate atomic operation with strict record-count verification.

3.4 Command and transaction order

Proposed commands:

- `yandi identity migrate-bound --port 9000 --dry-run`
- `yandi identity migrate-bound --port 9000`
- `yandi identity migration-status --port 9000`
- `yandi identity rollback-bound --port 9000`

Web:

- Owner-authenticated POST `/api/identity/migrate-bound`.
- GET `/api/identity/migration-status`.
- Owner-authenticated POST `/api/identity/rollback-bound`.
- Same transaction engine as CLI, with request idempotency keys and CSRF protection.

Use a controlled restart. Do not mutate the running identity object.

Order:

1. Acquire a process-wide migration lock and block concurrent identity/pairing/directory edits.
2. Open the existing identity with its existing credential context. Verify the signing seed produces the stored signing public key.
3. Compute the new ID and inventory all registered persistence adapters and configured storage roots. Include the working-directory contacts file, key directory, data directory and anchor-store override.
4. Detect conflicts, corrupt stores, ambiguous short filenames and unsupported schemas. Report them during dry-run; abort without modification on unresolved conflicts.
5. Create private, checksummed recovery artifacts and a journal containing transaction ID, source generation and intended target.
6. Prepare a new generation containing the resealed identity, alias registry, reference manifests and generated public artifacts. Existing history and TLS files remain shared and unchanged.
7. Read back the prepared identity and verify exact key continuity. Strictly validate every prepared store; do not use loaders that turn corruption into an empty default.
8. Start a candidate runtime against the prepared generation, with announcements and user-data writes disabled. Validate initialization; port ownership passes during the controlled restart.
9. Atomically replace and fsync one active-generation selector. Startup must consult it before initializing any identity-dependent component.
10. Activate the new runtime with bound-ID preference and old-ID compatibility. If activation fails, atomically select the old generation and restart the old-ID runtime.
11. Announce only after durable commit and successful activation. Failed deliveries enter a durable retry queue and do not undo the usable identity.
12. Regenerate offers/cards/ViaRecords, rebuild DHT routing and reconnect peers. Keep offline peers’ migration notices pending.

Use immutable generation directories and a single commit selector because independent renames across home, data and working directories do not form one atomic transaction. Conventional identity/card filenames are compatibility exports, not competing authorities.

Every persistence adapter declares node-address fields and migration rules. Unsupported node-address storage blocks migration until an adapter exists; blind search-and-replace is prohibited.

3.5 Dual-address period

Before 2027-04-01 00:00:00 UTC:

- Accept either wire ID only with the same authenticated signing key.
- Prefer the bound ID for capable peers.
- Preserve old-ID connections for unupgraded peers.
- Use distinct connection/crypto contexts for each wire-ID pair.
- Generate fresh traffic keys when changing wire IDs.

This last rule matters because current directional-key selection depends on ID ordering and sender ID is AEAD associated data. See src/crypto/session.rs:131 and src/crypto/store.rs:241.

At sunset:

- Stop accepting untagged IDs as authenticated network identities.
- Close remaining old-ID sessions.
- Keep historical aliases, old cards and migration proofs indefinitely as local references.
- Resolve established old references to the bound ID before connecting.
- Accept late migration proofs against an existing old-ID/key pin, followed by bound-ID authentication.
- Do not accept old offers, old Hello identities or old RESUME claims through an alias exception.

`YANDI_REQUIRE_BOUND_IDS` applies these wire restrictions immediately. Historical lookup remains available.

An offline peer with an established pin can migrate automatically after sunset. An unupgraded peer or an unpinned short-ID contact cannot be guaranteed automatic recovery; the existing identity model does not contain enough evidence.

3.6 Rollback and crash recovery

Before commit, discard the prepared generation and continue under the old ID.

After commit, rollback selects old-ID preference in the upgraded runtime while retaining:

- The verified alias.
- Stable storage context.
- New messages and user edits.
- Pairing relationships.
- Fresh-key resume support.

Do not restore stale chat/contact snapshots over live data.

Peers that already switched still know both addresses belong to the same principal. The rollback runtime must continue accepting the bound address, while preferring old-ID compatibility until sunset.

Startup recovery:

- PREPARED without selector commit: start old generation.
- COMMITTED: finish activation or select old generation if validation fails.
- ANNOUNCED: retain the alias permanently; select routing preference without deleting proof.

After sunset, rollback can preserve local data and operation but cannot restore old-ID network acceptance. That is an explicit limit imposed by the sunset requirement.

3.7 Live bootstrap entry

The shipped entry has old ID:

`8b9bf100e7b830935e119d0ebd04d1f613037e08f8dceedbb716c2c62c0a3225`

Its current key and endpoint are in bootstrap/bootstrap.json:9–12.

Derived bound ID for that exact key:

`0b7ce6e489b3963c5da023ce6fb61d33b3f8fe01cbd62f7643f370b459423100`

Rollout:

1. Upgrade the live entry with both changes and dual-address support.
2. Migrate it using its actual signing seed.
3. Verify bound and old connections externally, including paired-phone reconnects.
4. Replace only the entry ID in the format-1 bootstrap list; retain key, endpoint and roles.
5. Increment sequence above the current value 2; set fresh issue/expiry times.
6. Sign the complete document with the bootstrap publisher’s signing key.
7. Publish the entry’s independently signed migration statement as a separate artifact.
8. Publish a fresh bootstrap document before the existing document expires and remove old wire advertisement at sunset.

The entry key cannot sign the bootstrap list on behalf of the publisher. Bootstrap verification pins the publisher key and checks document signature, expiry and sequence. See src/bootstrap/mod.rs:152.

Keep format 1 during rollout: Entry and Doc reject unknown fields. See src/bootstrap/mod.rs:39 and src/bootstrap/mod.rs:56.

The existing conversion into BootstrapConfig retains endpoint and signing key but omits entry ID; old software using that path can therefore authenticate the migrated entry by its unchanged key. See src/bootstrap/mod.rs:265.

4. CHANGE 2: RESUME3

4.1 Security and compatibility policy

RESUME3 is PSK-authenticated ephemeral X25519:

- Persist a resume secret, never an active traffic master.
- Both sides generate fresh 32-byte nonces and ephemeral X25519 keys.
- Authenticate the complete exchange, including both principals, wire IDs, protocol choice and migration proofs.
- Install traffic keys only after explicit key confirmation.
- Never restore traffic crypto from disk, including fallback keys.

Add signed Hello capability `RESUME3 = 0x2000`.

Legacy RESUME sunset: 2027-01-01 00:00:00 UTC.

Until that date, new servers may support old clients’ legacy RESUME, using the unified identity verifier and clearly recording legacy use. That compatibility mode retains the original replay/PFS weakness.

New clients always prefer a fresh full Hello handshake over legacy RESUME fallback. Once a principal successfully uses RESUME3, persist a minimum version and refuse subsequent legacy RESUME for it.

After the date, reject `0xC0` restoration and require RESUME3 or a fresh Hello. Full Hello remains available; this sunset does not ban old nodes outright.

4.2 Ticket storage and issuance

ResumeTicketV3:

- Format version.
- Random 128-bit ticket ID.
- Network.
- Both Ed25519 signing public keys.
- Issuer principal.
- 32-byte resume secret, encrypted at rest.
- Fixed expiry, default seven days.
- Authorized wire-ID aliases or references to verified alias records.
- Protocol minimum.
- Revocation state.

Use the same ticket mechanism for paired mobiles and ordinary authenticated node peers. Index by ticket ID and principal; do not scan every paired client.

Issue tickets only after authenticated full Hello or completed RESUME3. Ordinary-peer tickets require a verified peer key and local policy permission.

Legacy token upgrade:

- Reuse its resume secret only after establishing both signing-key bindings.
- Ignore `session_key_hex` for RESUME3.
- Missing peer signing key, malformed secret or ambiguous identity forces full Hello.
- After successful upgrade, delete the persisted legacy traffic master and disable legacy resume for that principal.

Do not extend ticket expiry merely on receipt of a resume request. Issue a replacement ticket over the confirmed encrypted channel. Persist before acknowledging installation; retain at most two valid tickets during replacement, with the older one expiring at its original deadline.

This avoids making resume-secret rotation a prerequisite for crash-safe traffic-key renewal.

4.3 Message formats

Reserve an unambiguous pre-session envelope:

`"YAR3" | version:u8=3 | kind:u8 | body_length:u16 BE | body`

It is distinct from `0xC0` and ordinary encrypted frames. UDP/TCP demultiplexing and the WS first-message path recognize it before attempting session decryption.

All variable fields have explicit lengths. Reject duplicate fields, trailing bytes, unknown mandatory flags and messages over 1,200 bytes. IdentityMigrationV1 proofs are bounded fields.

REQUEST body:

- Network.
- Ticket ID: 16 bytes.
- Attempt ID: 16 random bytes.
- Initiator and responder signing keys: 32 bytes each.
- Initiator and responder wire IDs: 32 bytes each.
- Offered protocol/suite set: fixed bitmask.
- Initiator nonce Ni: 32 bytes.
- Initiator ephemeral X25519 public key Xi: 32 bytes.
- Optional identity-migration proofs for either participant.
- Initiator Ed25519 signature: 64 bytes.
- Resume-secret MAC: 32 bytes.

RESPONSE body:

- Ticket ID and attempt ID.
- SHA-256 of complete REQUEST.
- Selected version 3 and suite `X25519/HKDF-SHA256/AES-256-GCM`.
- Both final wire IDs.
- Responder nonce Nr: 32 bytes.
- Responder ephemeral public key Xr: 32 bytes.
- Status: OK.
- Responder Ed25519 signature: 64 bytes.
- Resume-secret MAC: 32 bytes.

FINISH body:

- Ticket ID, attempt ID.
- Transcript hash T.
- Initiator key-confirmation MAC.

CONFIRMED body:

- Ticket ID, attempt ID.
- Transcript hash T.
- Responder key-confirmation MAC.

Signatures and MACs use distinct domain strings for each message kind. REQUEST signs its fields before signature/MAC; its MAC includes the signature. RESPONSE signs its fields including REQUEST hash; its MAC includes the signature.

Authenticated ERROR:

- Ticket/attempt IDs, REQUEST hash, status and resume-secret MAC.
- Status: expired, revoked, identity conflict, busy or unsupported suite.

For unknown tickets, use a small unauthenticated generic refusal or silence. Such a response never authorizes downgrade or identity changes.

A claimed network address is unnecessary. Send replies to the observed transport source. For UDP, require a stateless return-path cookie before expensive work; bind it to source address and request hash. Commit a new endpoint only after FINISH.

4.4 Key schedule

Let R be the persisted 32-byte resume secret.

Authentication key:

`Kauth = HKDF-SHA256(salt=network-domain, IKM=R, info="yandi-resume3-auth")`

Compute REQUEST/RESPONSE MACs using HMAC-SHA256 and constant-time verification.

After both authenticated messages:

- `D = X25519(xi, Xr) = X25519(xr, Xi)`
- Reject non-contributory DH results, using the existing checked X25519 primitive. See src/crypto/x25519.rs:25.
- `T = SHA256(complete REQUEST || complete RESPONSE)`
- `PRK = HKDF-Extract(salt=T, IKM=R || D || Ni || Nr)`

Expand independent values, each including T:

- Initiator-to-responder traffic key.
- Responder-to-initiator traffic key.
- Initiator-to-responder nonce prefix.
- Responder-to-initiator nonce prefix.
- Initiator confirmation key.
- Responder confirmation key.
- Session/file exporter.
- 128-bit epoch ID.

No resumed traffic key equals the persisted master or a prior resume’s key.

Erase ephemeral private keys, D, PRK and confirmation keys after completion or timeout. Keep traffic keys in memory only.

This provides forward secrecy for completed RESUME3 traffic against later disclosure of R, provided ephemeral secrets were erased. It cannot retroactively protect traffic whose master was previously persisted.

4.5 Traffic frame

RESUME3 sessions use frame version 3:

- Explicit frame discriminator/version.
- Sender wire ID: 32 bytes.
- Receiver wire ID: 32 bytes.
- Epoch ID: 16 bytes.
- Counter: u64 big-endian, starting at 1.
- Ciphertext and 16-byte GCM tag.

Nonce: four-byte derived directional prefix followed by the counter.

AAD: complete frame header.

Maintain one replay window per receive epoch, using the existing 2,048-packet policy. Update it only after successful authentication. Preserve existing soft/hard message limits; exhaustion requires new keys, never counter wrapping.

Route by exact wire IDs and epoch, then verify AEAD. Resolve to the principal only after authentication.

Remove independently usable SessionCrypto clones: one epoch must have one serialized transmit counter and one receive window. Current Clone copies the counter and window, so parallel use would duplicate state. See src/crypto/session.rs:156.

4.6 State machine

States:

- DISCONNECTED / ACTIVE_OLD.
- REQUEST_SENT.
- RESPONSE_SENT with pending keys.
- FINISH_SENT.
- ACTIVE_NEW.
- FAILED.

Initiator:

1. Load a valid ticket and verified principal bindings.
2. Generate attempt ID, Ni and ephemeral key.
3. Send REQUEST; leave any live session untouched.
4. Verify RESPONSE against the exact outstanding request, ticket, identities, selected suite, signature and MAC.
5. Derive pending keys and send FINISH.
6. Verify CONFIRMED, then enable application transmission under the new epoch.

Responder:

1. Validate framing, cookie where needed, ticket, identities, signature and MAC.
2. Generate Nr and ephemeral key; send RESPONSE.
3. Hold pending keys without replacing the active session or endpoint.
4. Verify FINISH and activate the new epoch.
5. Send CONFIRMED; application packets follow confirmation on WS.

The initiator may buffer a small amount of new-epoch traffic arriving before CONFIRMED, but must not deliver it before confirmation.

Retransmissions reuse the identical attempt bytes and ephemeral state:

- Duplicate REQUEST returns the cached RESPONSE.
- Duplicate FINISH returns cached CONFIRMED.
- Never reinstall crypto or reset counters on a duplicate.
- After completion, retain only bounded response/confirmation metadata, not erased DH secrets.

Use a 10-second attempt deadline, bounded retransmission backoff, one pending attempt per principal and at most 256 pending attempts globally.

Concurrent resumes:

- Select the attempt initiated by the lexicographically smaller signing public key.
- The larger-key node abandons its own pending attempt when it receives the authenticated competing request.
- A single uncontested attempt can be initiated by either side.
- Resolve simultaneous full Hello and RESUME3 through the same per-principal session lock.

Use signing keys for this ordering so identity migration does not change the winner.

4.7 Restart, mobile and in-flight behavior

Normal restart:

- Reload tickets and identity bindings only.
- Start with no active traffic epochs.
- RESUME3 is accepted as a pre-session packet on UDP, TCP and binary WS.
- A surviving peer retains its old active epoch while the new attempt is pending.
- A full Hello replaces resume when tickets are absent, expired or incompatible.

Mobile binary WS:

- Accept REQUEST as the first WS message.
- Keep the socket pending and unregistered as an active peer until FINISH.
- Both sides use the same state machine and verifier as other transports.
- Preserve TLS fingerprint pinning.
- Bind socket cleanup to connection generation/epoch so an old pump cannot remove a newer connection’s registration.

Flutter `/mobile/ws`:

- Its existing API token remains API authorization.
- Upgrade both client and server separately to negotiate RESUME3 before session-encrypted application frames.
- Obtain a cryptographic ticket and signing-key binding over the authenticated pairing/API flow.
- Do not reinterpret its existing API token as the resume secret.

In-flight packets:

- A restarted side never reloads old traffic keys. Old packets reaching it are dropped.
- An uninterrupted side may retain old receive-only epochs for 30 seconds, with their existing replay windows intact.
- Old-epoch traffic cannot activate a new connection or change routing.
- Delete old epochs after the grace period; never persist them.
- Retransmit application messages under new keys with the same application message IDs and durable duplicate suppression.
- File transfers retain their per-transfer key/context across a live rekey. After a restart without that context, restart transfer negotiation using the same transfer ID and explicit new encryption context; do not mix chunks from incompatible keys.
- Real-time media drops late frames; interrupted streams reconnect.

Fresh resume secures the channel. It does not guarantee delivery of packets lost during restart.

4.8 Failure and fallback behavior

- Invalid signature/MAC, wrong owner, revoked token, conflicting alias or weak DH key: reject without changing active session, endpoint, trust or expiry.
- RNG failure: abort; never substitute timestamps or counters for randomness.
- Timeout or disconnect: erase pending secrets and retain the prior live session.
- Failed resume: mark that candidate attempted for the connection cycle, close the socket if necessary, and reconnect once using full Hello. Do not repeatedly choose the same failed token.
- Lost CONFIRMED: resend identical FINISH. If the responder restarted, its pending state is gone; start a fresh attempt with new randomness.
- Crash at any handshake point: start a new attempt; never restore pending traffic keys.
- Ticket persistence failure: retain the usable live session but do not advertise durable ticket installation.
- Successful RESUME3 followed by malformed/plaintext ACK: never accept it as confirmation.
- Unknown capability: perform authenticated full Hello; cache its signed capability result.
- A remembered RESUME3 minimum permits full Hello recovery, but forbids legacy master restoration.
- At migration sunset, old-ID ticket references resolve to the bound principal, but wire messages must use bound IDs.

5. Test plan

Identity migration tests

- Deterministic derivation and tag verification; exact sunset boundary; strict-bound environment.
- Statement signature vectors, network separation, mutation of every field, trailing bytes, replay/idempotency and conflicting old-key pins.
- A valid signature for an arbitrary victim old ID must not migrate the victim.
- v1/v2/key-root-v3 identity loading and resealing, password and machine fallback, exact signing-key continuity, preserved creation time and unchanged TLS fingerprint.
- Chat history under both encryption modes; verify every record and count, including old/new filenames, ambiguous short prefixes and both histories already existing.
- Contacts, trusted peers, exit authorization, paired-client tokens, paired anchors, avatars, groups and quota accounting survive repeated migration and rollback.
- Crash/failure injection after every prepare, fsync, selector commit, activation and announcement step; include disk full and corrupt stores.
- Concurrent web/CLI migration and concurrent contact/pairing writes.
- Mixed upgraded/unupgraded peers, offline pinned peer returning after sunset, strict-bound peers and both node/chat transports.
- Signed offers/ViaRecords remain valid immutable objects; fresh bound records replace them without changing reachability proof ownership.
- Bootstrap rehearsal with the exact shipped public key; new ID authentication, stale cached documents, increasing sequence and publisher re-signing.

RESUME tests

- Rust interoperability vectors for transcript encoding, signatures, MACs, DH, HKDF, epoch IDs and traffic frames; equivalent mobile vectors.
- Different keys on repeated resumes, including restored tickets and identical principal pairs.
- Capture packets, restart either side or both, complete resume, then replay captured packets: reject every old epoch.
- One-sided restart accepts new counter 1 under new keys while the surviving peer’s old window remains intact.
- Replayed REQUEST cannot replace a live epoch; duplicate REQUEST/FINISH never resets counters.
- Loss, duplication, reordering, simultaneous resumes, full-Hello races and crash at every handshake state.
- Cross-token attack against `handle_resume_packet`; wrong embedded ID, wrong signing key and unauthorized migration proof.
- Plaintext success ACK injection and capability downgrade attempts.
- UDP NAT rebinding/cookies, TCP reconnect, Rust binary WS and Flutter API WS as separate integration suites.
- Token absence, expiry, revocation, malformed secrets, storage failure and clock boundaries.
- Legacy compatibility before 2027-01-01, rejection afterward, and successful fresh-Hello recovery.
- Old-ID acceptance before identity sunset and rejection afterward.
- Replay-window integrity, key exhaustion, forbidden crypto cloning and bounded pending-state resource use.
- Chat duplicate suppression, file-transfer restart/context separation and dropped late media.

6. Risks and limits

- Random legacy IDs are not cryptographic ownership proofs. Migration cannot repair a poisoned first claim, missing signing-key evidence or ambiguous short-ID contact automatically.
- Unmodified peers cannot consume a new migration statement. Dual wire identities bridge them only until the identity sunset.
- Publication is irreversible knowledge: rollback can preserve reachability and data, but cannot make peers forget an announced alias.
- Multi-file migration is unsafe without a shared startup selector and persistence adapters covering every node-address store.
- Retaining the original chat context preserves existing weak legacy encryption; strengthening it requires a separate verified re-encryption.
- Legacy RESUME compatibility retains the known vulnerability until its earlier sunset.
- Clock errors can affect both sunsets and ticket expiry. Store observed deadlines and surface clock anomalies; do not silently extend compatibility.
- Compromise of signing keys compromises identity continuity. Compromise of ticket secrets plus signing keys permits future impersonation until revocation; ephemeral DH protects completed RESUME3 traffic after secret erasure.
- Restarts lose unpersisted packets and transfer crypto state. Application retransmission and duplicate suppression are required.
- Bootstrap migration requires both the live node operator and the bootstrap publisher; possessing either signing key alone is insufficient for the complete rollout.