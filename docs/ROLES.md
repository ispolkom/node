# Roles, Administration Panel, and Notifications (Specification, Owner Decisions 2026-10-18)

## Model

The node is like a computer with an operating system: it has an **administrator (root)** and **users**.

| Who | What they do | What they do not do |
|---|---|---|
| **Node owner = administrator** | Manage the node: handle complaints, delete groups, warn and remove users, issue pairing codes, approve pairing, and configure the node. | Read anyone's messages, including other people's messages; write in personal chats or groups **as admin**. |
| **Node user** (including the owner's personal account) | Communicate, create groups, transfer group rights (creator/group owner), use proxy exits, and make calls. | Manage the node. |
| **Invited device** (using a code from a trusted device) | Act as a user: use its own email, profile, groups, complaints, and Internet access through the node. | **Invite others** until the node administrator promotes it to a user. |
| **Guest** (without QR; phone, tablet, or PC) | Communicate with limits (see `docs/GUEST.md`): online people and the node PC, 10 messages per day, audio calls, and complaints. | Groups, Internet, video, and reading the owner's messages. |

When the owner communicates, they do so **as a user** through the personal PC account; the personal account has no administrative rights.

## Secrets

- **One seed for everything** (the seed is written for the owner; it restores both the node and the personal account's closed environment).
- **Administrator password** — opens the administration panel and administrator session. It does **not** open chats.
- **User password** — opens the personal account's closed environment (chats, groups, contacts, and files). The administrator password does not open it.
- Node working data (device registry, message-reception keys, proxy pool, and complaints) remains under the machine key because the node must work without the owner. We do not create a nested key “matryoshka”.
- First startup: (1) administrator — seed, login, and administrator password; (2) user — login, password, and confirmation.

## Desktop shell

- Personal-account top menu: Home, Chat, Search, Groups, Settings (personal language, notifications, profile, and closed environment), About, and **Administration panel** with a notification counter.
- **The administration panel** opens with the administrator password for 15 minutes. The password must be entered again after expiry. Its first menu item is “Personal account” (return); the other items are Node status, Gateways, Complaints, Node settings (guest access, email lifetime, external proxies, devices and pairing codes, backups, updates, and emergency erase).
- The visual style is the same as on the other pages.
- The server checks the administrator session on every administration route, rather than relying on the interface.

## Notifications in the personal account

Administrator events are duplicated in the personal account so the user does not need to open the administration panel without a reason:

- complaints (number of new complaints), pairing requests, an available update, and unresponsive proxies;
- **type, label, count, and time only**: no complaint text, attachments, tokens, or codes;
- identical events are merged into one counter; read state is remembered;
- acting on a notification opens the administration panel at the relevant location and asks for the administrator password;
- notifications are calculated on the node from its current state, with nothing unnecessary stored; they are not sent to the phone by default.

## Stages

1. (done) “Administration panel” tab with the pages moved into it, administrator session by login password (at this stage the administrator password equalled the login password), and notifications.
2. (done) Separate administrator password: `admin_auth.json` next to `auth.json` (Argon2id). If the file is absent, the administrator password equals the login password for migration without user action. It is set in the wizard (optional) or in Node settings, where an active session and the current administrator password are required. It must differ from the login password. A forgotten administrator password is reset by signing in with the seed phrase. A corrupt file causes rejection instead of falling back to the login password. Failed-attempt counting is shared with login.
3. (done) Remove unnecessary restrictions from the user role (proxy and chats) and separate the PC personal account from node administration; Codex checks each step.

Step 3 in code: the `user` role can access `/mobile/proxy/info` (Internet access through the node); the PC personal chats and contacts (`/mobile/chat/`, `/mobile/contacts`) remain closed to `user` and guest because they belong to the owner's personal account. Groups and proxy exit are already available to users; guests are denied them.

R9 (2026-10-21): **The PC is a participant.** Users and guests see “Computer” in the people list and can message it (the PC key at `/mobile/pubkey/<node number>` is open to every role). Their messages appear in the PC web chat under the device number, and the owner answers them like any other contact. The owner's other conversations remain closed (`/mobile/chat/*`, `/mobile/contacts`, and `Node` events over WS).

## Clarifications after audit 34 (P14-1…11)

- Administrator access is determined on the server by method and path (`admin::is_admin_route`). Besides prefix lists, this includes `/api/settings`, `/api/network/relay`, `/api/node/start|stop`, `/api/backup/export|config`, pairing secrets (`/api/devices/pairing`, `/pair/qr`), all-node groups, and their deletion (`/api/mobile/groups…`).
- A session belongs to the client: on login the node issues a random token and stores only its hash; the client sends it in the `x-yandi-admin` header. Another client of the same user receives no access. Logout, locking the personal environment, signing out, and seed-phrase recovery close the session.
- Deleting another user's group requires the session and the administrator password again.
- Failed-attempt budgets are separate: five for the login password and five for the separate administrator password. A successful login with one does not reset the other. While both passwords are the same, there is one budget.
- Notifications remember seen events by identifier fingerprints on disk, without text, rather than storing a count. “Read” is marked by the page that displays the event.
- The administration panel does not require the personal environment to be open.
- The administrator password is written before keys are created; a write failure cancels setup. Reset during seed-phrase recovery returns an error if the file cannot be deleted.
- Node web pages over TCP, when enabled, do not have administrator-session forms; node management is done through the desktop shell.

## Device invitations (the “invited” role)

Network growth follows a chain of trust rather than an open door.

- **Who invites.** A device paired by an administrator through QR, with role `owner` or `user`, selects “Settings → Invite device” and receives a one-time 8-digit code and QR (`YANDI-PAIR-1:…`, the same format as a pairing code). The code lasts 60 minutes and can be used once. It also provides a 4-digit PIN, which the inviter tells the other person through another channel; three incorrect PINs burn the code.
- **Who is invited.** A device connected with this code receives the `invited` role: user rights, but no ability to issue invitations. The node rejects such requests and the application hides the section until the administrator presses “Promote user” in the device panel or runs `yandi admin device-promote ID`. The panel shows the nickname and picture chosen by the person, as well as those of the inviter. A nickname is not proof of identity, so the technical name and inviter are always shown beside it.
- **Record.** The node stores who invited whom (`invited_by`). The device panel and `yandi admin devices` show the role and inviter. “Revoke with invited branch” (`device-revoke-branch`) removes the device, all devices it invited, and all devices invited by those devices.
- **Limits** are set by the administrator through the panel or `yandi admin invites set`, and stored in `mobile_invites.json`: enabled or disabled (enabled by default), codes per day per person (10), and simultaneously live codes (3). The whole node allows at most 64 live codes and 48 invited devices. Invitations **never evict anyone**; when there is no room, the code is rejected.
- **Code guessing.** Twenty incorrect attempts within five minutes across the node block invitation intake for that period. A code is one-time; if the inviter loses the right or is removed, their code stops working.
- **What is not included.** An invitation grants no access to the inviter's messages, and the inviter cannot see what the invited person is doing. The daily invitation and live-code counters are kept in node memory; restarting resets the daily counter, which is a known limitation.
