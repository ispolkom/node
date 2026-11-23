# What the Node, Relay, ISP, and Device Seizure Can See

This document describes the **actual** situation without promises the system does not make. YANDI does not provide complete anonymity.

## What is protected

- **Messages and files between phones** whose keys are registered on the node, and **phone groups**, use end-to-end encryption between the devices themselves (E4 format; tests are in `mobile/yandi_mobile/test/e2e_security_test.dart` and the group tests). The node and relays see ciphertext.
- **What is NOT covered by end-to-end encryption:** the session between nodes is encrypted between endpoint nodes (the UDP relay does not have the internal-session key according to the code; multi-node operation has not been tested). An anchor is the endpoint of its WebSocket channel, an exit node sees traffic to the Internet destination, and the node through which a phone writes to an external computer reads that message. Node-to-node messaging has no asynchronous sealed-envelope layer, no forward secrecy for stored content, and no human key verification. History on endpoint nodes is stored in the closed environment. Calls: the WebRTC media stream uses DTLS-SRTP between participants and a TURN relay forwards ciphertext; phone signaling is encrypted, while computer signaling is not authenticated by endpoint participants everywhere. There have been no physical-device tests. See `docs/E2E_ARCHITECTURE.md`.
- **The closed environment** (PC messages and files, external proxy pool, and local contact names) is sealed with a password-derived key and cannot be read without the password or seed phrase.
- **Node service data** (PC groups, “Computer” keys used as a phone contact, and complaints) works while the owner is away from the screen, because the node must receive messages overnight. It is therefore sealed with the machine's device key rather than a password. Anyone who seizes an **powered-on or running** computer can access it like the node itself; the password does not protect it. Emergency erase removes it together with the closed environment.
- **Data on a powered-off and locked device** is protected by the password. The limits of five incorrect passwords and 20 incorrect seeds work **inside the application**; someone who copies the files and tries them offline does not encounter those limits. Only password strength protects against that attacker; a 128-bit seed phrase is not practically guessable.

## What is visible

| Who | What they see |
|---|---|
| The node you connect to | That the device is online, exchange time and volume, and correspondent numbers in mail (ciphertext) until retention expires; the source IP as a property of the network connection. |
| Relays and nodes in a circuit | Circuit neighbors, time, and volume; not contents. |
| A call correspondent | Your IP when the call is direct WebRTC; through TURN, the correspondent sees the TURN address of your node. |
| ISP and network observer | That you connect to YANDI and the traffic volume, not the contents. |
| Someone who takes a **powered-on and unlocked** device | Everything currently open in the application. |
| Phone spyware such as Pegasus | Practically everything visible to the owner: end-to-end encryption protects the channel, not a compromised device. |

## Logs

- The application and node **do not write** correspondent, client, or destination-site addresses to their logs. Such values pass through `util::logsafe::a` and are printed as “address hidden”. The node owner can enable display for debugging with `YANDI_LOG_ADDRS=1`. The `log_lines_do_not_print_peer_client_or_destination_addresses_raw` test prevents these lines from returning.
- The operating system, firewall, and ISP logs are outside the application's control.
- The phone system log receives only error type names, without addresses.

## What is stored and for how long

- Device-to-device mail (ciphertext and “from/to” metadata) remains on the node until the owner-selected retention period: seven days by default, from one to 30 days; guest mail is always retained for 24 hours.
- Guests exist only in node memory and are not written to disk or backups; a guest that has been silent for a day is removed.
- Pairing and invitation request queues exist only in memory, for up to one day.

## What you can do

- **Password.** A weak password can be attacked offline. The application warns about it during creation and change through the field color and hint. A phrase of several words is better.
- **Emergency erase.** Use “Settings → Emergency erase” on the PC for the closed environment and on the phone for all application data. It requires no password but does require the confirmation word “ERASE” in the relevant interface.
- **Mail retention.** A shorter period leaves fewer traces on a node that could be seized.

## Known limitations (not hidden)

- The paired-device registry (names, token hashes, and keys) is stored openly on the node disk. Phones need it for login while the closed environment is locked, for example after a node restart overnight. Sealing it with the closed-environment key would break connectivity; a safer design using a node-root-derived key with migration and one-version backup is planned.
- Attempt limits (five passwords and 20 seeds) protect against guessing through the application, not against guessing copied files.
