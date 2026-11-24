# Encryption Map (Audit 2026-10-05, Updated After Fixes)

How each traffic type is currently protected, what remains weak, and the order of fixes. This is based on reading the code, not speculation; line numbers refer to the audit snapshot.

## What protects what

| Traffic | Implementation | Key | Forward secrecy | Assessment |
|---|---|---|---|---|
| Messages, files, and calls (`p2p::P2PTransport`) | `p2p/encryption_manager.rs`, AES-256-GCM | Handshake with **one-time** X25519 keys for every hello | yes | Good by design; minor defects remain below. |
| Core node network: DHT, relaying, proxies/tunnels, station/train (`netlayer::P2PTransport`, `protocol::*`) | `netlayer/encryption.rs` → `crypto/handshake.rs`, AES-256-GCM | One-time keys for every handshake (**fixed**; previously one key per process) | yes | Good by design. |
| Multi-node circuits (`hops.rs`, `hops_net.rs`) | X25519 handshake per link using a telescope; the node's reply is **signed with its long-term card key**; layer keys use HKDF-SHA256; layers use BLAKE3 stream encryption by cell number with an internal `MAC`; cells have equal size | One-time keys on both sides | yes | Good by design; no cover traffic or entry pinning (see I21). |
| End-to-end encryption between people | `communication/encryption.rs`, `p2p/encryption.rs` | — | — | **Stubs** (“transport encryption is used for the prototype”). |
| Resumption-session keys (QR pairing) | **Removed** (stage 2a, `docs/RESUME_REDESIGN.md`): v2 `paired_clients.json` / `paired_anchors.json` contain permission records without secrets | Session key stored in plaintext on disk | no | Defect: the file was first created with ordinary permissions and only then changed to `0600`. |
| Node identity on disk | Argon2id + AES-GCM | Owner password | — | Good (SEC-04 closed). |
| Hello | Ed25519 signature | Identity key | — | Good (SEC-03 closed). |

## Defects in descending severity

### Critical

- **C1. Circuit keys derived from public data — CLOSED.** The circuit prototype (`derive_hop_key` from public data) was removed. `hops.rs` now uses real key exchange and signed handshakes. It is still not correct to call the network anonymous (I21: no cover traffic or entry pinning).
- **C2. No end-to-end encryption between people.** Direct node connections benefit from transport encryption with forward secrecy, but a relay or a “client separate from node” intermediary can read the traffic. Fix: a separate E2E layer between people's devices (phases 3 and 5).
- **C3. Core network without one-time keys.** `netlayer/encryption.rs` used `HKDF(ECDH(process key, neighbor key))` as the session key. A process-secret leak could reveal all packets recorded during its lifetime. Fix: one shared encryption implementation for both transports with one-time keys for every handshake.

### High

- **H1. Secrets in logs.** `println!` printed eight bytes of the shared secret (`Shared: …`), eight bytes of the session key (`Session key derived: …`), four bytes of the per-packet key (`key=…`), and public-key prefixes. Fix: remove all of them and add a guard test.
- **H2. Secret files created with ordinary permissions.** `fs::write` followed by `set_permissions(0o600)` created a window in which another user could read the file; `chmod` errors were silently ignored (`let _ =`). Fix: create with `0600` and treat errors as errors.
- **H3. Session key stored on disk in plaintext.** *Fixed in code (stage 2a, `docs/RESUME_REDESIGN.md`):* resumption through saved secrets and the secrets themselves are gone; v2 connection files contain no secrets and old files are rewritten. Independent review (round 36) found cleanup weaknesses; their fix is being rechecked (`docs/SECURITY_STATUS.md`, P16-*). Hardening the handshake itself is stage 2b and is not complete.
- **H4. Replay protection window — the last 1024 random one-time numbers.** An old packet could be replayed after 1024 new packets; the window was empty after a restart. Fix: counters instead of random numbers and a counter-based sliding window.
- **H5. Session key lives for the entire process lifetime.** Random 96-bit GCM nonces under one key are safe only up to roughly 2^32 packets; a server running for months approaches that limit. Fix: rotate the key by packet count and time.

### Medium

- **M1. Sender number in a plaintext unauthenticated header.** It was not part of the encryption associated data. Fix: include the header in AAD; later, encrypt the sender identifier on the path.
- **M2. Two copies of the encryption manager** (`netlayer/encryption.rs` and `p2p/encryption_manager.rs`; the second file was named after the first in its header) had diverged. Fix: one `crypto` module and one place to make changes.
- **M3. Session private key stored in `Session` without clearing memory on drop.**

## Fix status

| Item | Status |
|---|---|
| H1 secrets in logs | **fixed**; guard test `tests/no_secrets_in_logs.rs` checks the previously returned leak. |
| H2 secret files with ordinary permissions | **fixed**: `util::private_file::write_private` uses `0600` at creation, a temporary file and rename, and does not follow links; guard `tests/private_files_guard.rs`. |
| C1 circuit keys from public data | **closed**: the prototype was removed and replaced by `hops.rs` with real per-link key exchange and signed handshakes (I10). |
| M2 two encryption-manager copies | **fixed**: shared `src/crypto/` (`session`, `store`, `x25519`); both wrappers are thin. |
| H4 replay window | **fixed**: counters and a 2048-packet window; forged packets cannot corrupt the window. |
| H5 key lives forever | **fixed**: 2^32 packet ceiling; a key older than one hour or 2^28 packets (configurable through `YANDI_REKEY_AFTER_SECS`, 10 seconds to one day) rotates through a new handshake. The previous key accepts in-flight packets for another 30 seconds. Verified on a live network with 60 messages and a 15-second key age, with none lost. |
| M1 header outside authentication | **fixed**: sender number is part of AAD. |
| M3 keys in memory | **fixed**: keys are cleared with `zeroize`. |
| Weak X25519 points | **added**: reject a zero shared secret. |
| Core-network hello signature did not cover every field | **fixed**: ALL fields (type, version, capabilities, addresses, jurisdiction, and so on) are signed with the `yandi-hello-v2` label. Changing any field breaks the signature; the golden vector was independently generated in Python. Previously an on-path attacker could change addresses and declared capabilities. |
| Hello parsing lost fields | **fixed**: after `ipv6_external` and `p2p_data_addr`, the parser position no longer shifted, so the messaging-channel key and jurisdiction are not silently lost. Serialized and parsed values now match; long and empty strings are rejected during serialization, and jurisdiction is normalized at creation. |
| C3 core network without one-time keys | **fixed**: both networks use shared `crypto::handshake` with one-time keys per handshake, invitation → confirmation, and simultaneous discovery resolved by the lower node number. The process-static key was removed; verified on a real three-node network. |
| H3 session key on disk (old QR phone pairing) | **fixed in code** (stage 2a); independent review (round 36) found cleanup weaknesses and is rechecking them. The handshake is stage 2b. |
| Key rotation in a live network lost messages | **fixed** (2026-10-06): responder pending key, winner rule for simultaneous rotation, and fallback-key ordering; see the roadmap log. |
| C2 end-to-end encryption between people | **open** (phases 3 and 5). |

Packet format (version 2): `[sender number:32][version:1 | salt:5 | counter:6][ciphertext with tag]`; inside the ciphertext: `[length:4][data][padding]`; directional keys from HKDF-SHA256 (`yandi-v2 key low->high / high->low`); AAD = `[version] ‖ sender number`. The golden vector was checked by an independent implementation (Node.js/OpenSSL).

Known behavior: when two nodes discover each other simultaneously, the first probe packet may use the losing key and fail to decrypt (one log error per node). Keys match immediately afterward and communication continues; this was checked in the logs of two live nodes. Future improvement: do not send probe packets until a single key is confirmed.

## Fix order

1. H1 and H2 (quick, safe, and checked by guard tests).
2. C1 (circuits): **done** — `hops.rs`; cover traffic and entry pinning remain (I21).
3. M2 + C3 + H4 + H5 + M1 + M3: one shared encryption module with one-time keys, counters, a window, key rotation, and AAD; migrate both transports to it.
4. H3: completed in code and under recheck; the on-disk key is removed and the handshake is stage 2b.
5. C2: end-to-end encryption between devices, including the “client — node” protocol.
