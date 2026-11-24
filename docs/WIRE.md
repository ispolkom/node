# Wire Format (Frozen Part)

This document describes the parts already fixed by golden tests and not allowed to change silently. Any format change requires a version increase and an explicit decision. Other packet types (DHT, relay, tunnel, proxy, call, and file-transfer packets) are not frozen yet and will be documented as they become fixed.

## 1. Session packet (encrypted)

Format version: **2**. Packet:

```text
[sender number: 32] [one-time number: 12] [ciphertext ‖ GCM tag: n+16]
```

* One-time number: `[version: 1 = 0x02] [session salt: 5] [counter: 6, big-endian]`. The counter starts at 1 and increases by 1 for every packet from the sender. The salt is 40 random bits chosen when the session is created.
* Cipher: AES-256-GCM. The directional key is derived from the shared session key: `HKDF-SHA256(salt = none, ikm = shared key, info = "yandi-v2 key low->high")` for lower node number → higher node number and `"yandi-v2 key high->low"` for the reverse.
* AAD: version byte (`0x02`) ‖ sender number (32 bytes).
* Inside the ciphertext: `[data length: 4, little-endian] [data] [zero padding]`; padding is 8–32 bytes for data shorter than 64 bytes, otherwise 0–8 bytes.
* The receiver accepts a packet only when version `0x02` and authentication are valid, the counter has not appeared before, and it is not older than the 2048-packet sliding window. The window changes only after successful authentication.
* Ceiling: 2^32 packets under one key, after which packets are rejected. After one hour or 2^28 packets, nodes negotiate a new key (`YANDI_REKEY_AFTER_SECS` changes the one-hour value). The previous key accepts in-flight packets for another 30 seconds, with no more than two previous keys retained.
* Golden vector: shared key `07…07` (32 bytes), node numbers `01…01` and `02…02`, salt `aa bb cc dd ee`, counter 1, data `golden`, AAD `02 ‖ 01…01` → ciphertext with tag `4bd3c4d90dd64958679aa40615886a59cc95f45f606e`. Generated independently with Node.js/OpenSSL. Test: `src/crypto/session.rs::golden_vector`.

## 2. One-time-key handshake

1. The initiator creates an ephemeral X25519 pair and sends the public half in an invitation hello (`x25519_public`). The invitation number is the hello `nonce`. The secret remains until confirmation, with no more than 64 unanswered invitations, and is erased when used.
2. The responder creates its own ephemeral pair, computes `shared = X25519(e_r, E_i)`, stores the session without storing the secret, and sends its public half in the confirmation (`x25519_public`, `nonce` equal to the invitation number).
3. The initiator computes `shared = X25519(e_i, E_r)` and stores the session.
4. Session key: `HKDF-SHA256(salt = none, ikm = shared, info = "YANDI session key")`, 32 bytes. A zero shared secret, a weak point, is rejected.
5. Simultaneous discovery, where both nodes invite each other, ends with one key: the handshake started by the lower-numbered node remains; the losing key is accepted for decryption only for 30 seconds.

## 3. Core-network hello (UDP discovery port)

```text
"NET1" [type: 1 (0=request, 1=confirmation)] [version: 1] [node number: 32] [node name: 32 = SHA-256(public key)]
[Ed25519 public key: 32] [x25519_public: 32] [signature: 64] [cid: 8] [capabilities: 2 BE] [time: 8 BE] [nonce: 8 BE]
[wan: length 1 + bytes] [lan: length 1 + bytes] [discovery endpoint: length 1 + bytes]
[ipv6_virtual: flag 1 (+16 bytes)] [ipv6_external: length 1 + bytes] [p2p_data_addr: length 1 + bytes]
[p2p_x25519_public: flag 1 (+32 bytes)] [jurisdiction: length 1 (≤ 8) + bytes]
```

* Strings are 1–255 bytes; an empty string is forbidden because it is indistinguishable from “no value” on the wire. Long strings are not truncated; serialization rejects them.
* **Signature:** Ed25519 over the following bytes (**all fields except the signature**): `"yandi-hello-v2" ‖ 0x00 ‖ type ‖ version ‖ node number ‖ node name ‖ public key ‖ x25519_public ‖ cid ‖ capabilities ‖ time ‖ nonce ‖` followed by wan, lan, discovery endpoint, ipv6_virtual, ipv6_external, p2p_data_addr, p2p_x25519_public, and jurisdiction in that order. An optional value is `0x00` for absent or `0x01 ‖ [length: 4 BE for a string] ‖ bytes`. Tests: `src/netlayer/packet.rs::the_signed_bytes_are_frozen` (the SHA-256 of signed bytes was independently generated in Python) and `changing_any_single_field_breaks_the_signature` (25 field changes).
* The receiver first checks the signature and that the name matches the key, then the time window (±5 minutes) and admission rules (section 5), then the `(node number, nonce)` replay pair.

## 4. Messaging-channel hello (UDP)

Serialization uses the `bincode` `P2PHelloPacket` structure. Ed25519 signs `canonical_bytes()` (version, node number, `x25519_public`, time, nonce, length and bytes of `p2p_data_addr`, and the Ed25519 public key). Reception rules are the same: signature, replay, and admission checks.

## 5. Admission rules for new nodes

* Known node, already in the table: no more than 20 hellos per minute.
* New node: no more than 10 per address per minute and 300 across the node per minute. The peer table cannot exceed `20 × max_connections(capacity)` (200 / 1000 / 3000).
* When the table is full, the oldest unattached non-anchor node that has been silent for at least two minutes is evicted. If there is no such node, the request is rejected.
* Tests: `src/netlayer/admission.rs`, `tests/hello_flood_test.rs`, and `tests/p2p_hello_flood_test.rs`.

## Not frozen yet

DHT, relay, tunnel, proxy, call, and file-transfer packets; the “client — node” protocol (a separate `CLIENT_WIRE.md` will be created); and QR-pairing session resumption, which will be replaced by the client protocol.
