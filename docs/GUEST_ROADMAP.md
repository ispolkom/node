# Guest: mini-roadmap (2026-10-20)

Owner's decision: a guest is not a dummy and not a full user. Nothing is hidden: closed features are shown, and selecting one opens a yellow notice: “Available after full authorisation”.

## Rules
| | Guest |
|---|---|
| Guest intake on the node | **enabled by default**; the owner can disable it (`guest_pairing.json`). |
| Private messages with node users, voice messages, audio, and video in chat | allowed |
| Node people list: a guest sees only people currently online (nickname and picture, no addresses); guests see, message, and call each other; the application hides anyone offline for more than one day | allowed |
| Files | up to 30 MB, 20 files, and 300 MB per day (unchanged) |
| Audio calls | allowed: up to **15 minutes** per call and **60 minutes** per day |
| Video calls | **not allowed** (yellow notice) |
| Groups, VPN/proxy, and inviting devices | **not allowed** (yellow notice) |
| Nickname, picture, and complaints | allowed; the profile is erased with the guest |
| “Request user status” with guest text | allowed → the administrator sees “Guest X: text” and can accept or reject it |

## Stages
1. **Node**: guest intake by default; user list, nickname, complaints, audio signals to guests; call limits (15-minute TURN access, four calls per day = 60 minutes, no more than four simultaneous guest calls per node); status-request text; tests.
2. **Application**: guest role; yellow notice in Russian and English for closed features; status request in Settings (pending/accepted/rejected); video button closed; tests.
3. **Desktop shell**: the guest's request text is visible; “Guest” role in the device list; tests.
4. **Documents and site**: `GUEST.md`, download pages, release of node/shell/APK, and phone verification.

## Honest limits
* Call signals are encrypted between devices, so the node cannot see whether a call is audio or video. The application enforces the guest video restriction; the node enforces call time and count through the TURN-access lifetime.
* The guest application calls only through the node's call server (relay) and rejects calls without it; the node cannot break a direct connection from a modified client.
* Guest quotas live in memory: restarting the node or creating a new guest resets them, which is why the node also has a global guest and call ceiling.
