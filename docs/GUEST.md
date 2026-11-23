# Node Guest Level

A guest is a device paired with the node without QR, through automatic pairing. A trusted device is paired by the node owner through QR.

## What a guest can and cannot do

Guest intake on the node is **enabled by default**; the owner disables it in Settings (`guest_pairing.json`). Closed features are not hidden: the application shows them, and selecting one opens a yellow “Available after full authorisation” notice with an option to request status.

| | Guest |
|---|---|
| Any device without QR: phone, tablet, or PC (`yandi-desktop --guest`) | R9 |
| Messages with node users, other guests, and the **node computer (PC)**, voice messages, audio, and video in chat | allowed: **10 messages per day** (the application counts them; the node caps messaging at 60 frames per day); file limits are below |
| Node people list: **only people currently online** (guests and users), with role, nickname, and picture, without addresses; a hidden profile hides only nickname and picture. Users and the owner see everyone, online or offline; the application hides anyone offline for more than one day until they return | allowed |
| Profile nickname and picture, complaints | allowed; the profile is erased with the guest, while complaints are stored normally |
| User-status request with up to 300 characters | allowed: the administrator sees “Guest X: text” and can accept or reject it |
| Audio calls | allowed: **15 minutes** per call, **60 minutes per day**, and no more than four simultaneous guest calls per node; **a guest cannot call the node computer (PC) yet** because the PC call page does not enforce guest limits |
| Video calls | **not allowed**; the application enforces this because call signals are encrypted and the node cannot distinguish video from audio |
| Internet access through the node (proxy or VPN) | **not allowed** |
| Groups | **not allowed** |
| Inviting devices | **not allowed** |
| Reading the owner's conversations and contacts with others (`/mobile/chat/*`, `/mobile/contacts`) | **not allowed** |
| Guest account | lives **one day from creation**, or until one day of inactivity, then is erased; **three complaints** from different devices within one day remove it immediately |
| Guest files across the node | no more than **2 GB per day** across all guests; above that, “node busy” |
| “Nodes” directory | only nodes from the signed list with the `guest` role that are currently online |
| File larger than 30 MB | **not allowed** |
| More than 20 files per day | **not allowed** |
| More than 300 MB of files per day | **not allowed** |

The node checks restrictions on every request. The role comes from the node's pairing record, not from the client's claims.

### Call limit accounting

Call-server (TURN) access is issued to a guest for 15 minutes and no more than four times in a rolling day (4 × 15 = 60 minutes). The fifth issue is rejected with `call_day_limit`; if four guest calls are already active on the node, the result is `call_busy`. The guest application ends a call at 15 minutes and warns one minute before the limit.

The counter is based on issued access, not actual duration: a short or failed call still uses one of the four grants. The guest application calls only through the node call server in relay mode and rejects calls when the server is not configured. The node cannot break a direct connection from a modified client.

## “No logs”: what it means

There is no permanent guest history. The IP address is not written or shown in the interface, logs, responses, or backup. The following **temporary** state is allowed and explicitly limited:

- Guest record (token hash, role, and counters) is kept in memory and erased 24 hours after creation, after 24 hours of inactivity, or when the node restarts.
- Guest queue and files remain no longer than 24 hours.
- Pairing-request queue remains until the owner decides, for up to 24 hours, without IP addresses.

## Quota boundaries

Quotas are counted per guest key and per node. A new key, node restart, or move to another node resets the counter. There is therefore a **shared budget for all guests** (guest count, issue frequency, storage, and traffic). It protects node resources but does not provide fairness between people. “20 files per person” is not promised.

## IP visibility

In a direct connection the node technically receives the source address. The rule “the owner does not see the guest IP” is fully enforceable only through a circuit whose entry is not the owner node; this is planned. Until then, the honest wording is: “The IP is not written or shown, but it is technically visible to the node you connect to.”

## Phone gateway endpoints for guests and invitations

| Endpoint | Who | What it does |
|---|---|---|
| `POST /mobile/pair/guest` `{device_name}` | Anyone, if the operator allows it | Issues a `guest` token and limits; the address is not read. |
| `POST /mobile/pair/request` `{text?}`, `GET /mobile/pair/request` | Guest | Sends an “accept me” request with up to 300 characters and returns its state (`pending/accepted/rejected/none`). |
| `POST /mobile/invite` `{invite, signature}` | Device with loaded keys | Registers an invitation signed by the device key. |
| `POST /mobile/invite/revoke` `{id?}` | Same device | Revokes one or all of its invitations. |
| `POST /mobile/invite/redeem` `{invite, invite_signature, token, claim, claim_signature}` | Device B on the same node | Atomically consumes the invitation and queues a claim for device A. |
| `GET /mobile/invite/claims`, `POST /mobile/invite/claims/ack` `{ids}` | Device A | Retrieves claims and acknowledges receipt. |

Invitation format: JSON whose bytes are signed with Ed25519 using the `yandi-invite-v1\0` label; claims use `yandi-claim-v1\0`. All binary fields are unpadded base64url. Unlike the Codex recommendation, the received bytes are signed instead of deterministic CBOR: the node has no CBOR library, and canonicalization is unnecessary because the node verifies the same bytes it parses. The phase-one redemption receipt is not signed by the node.

All invitation state is held only in node memory. Restarting the node erases it and invalidates the invitation.
