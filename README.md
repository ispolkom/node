<div align="center">

<img src="docs/assets/logo.png" alt="YANDI" width="120">

# YANDI

**A decentralized peer-to-peer network for communication and Internet access through self-hosted nodes.**<br>
Encrypted messaging, file sharing, voice and video calls, HTTP/SOCKS5 access and multi-hop routing. No mandatory central messaging server or accounts. Encryption boundaries depend on the communication path; see [Privacy](docs/PRIVACY.md).

[![Website](https://img.shields.io/badge/website-yandi.su-1f6feb)](https://yandi.su/)
[![Downloads](https://img.shields.io/badge/downloads-Debian%20%C2%B7%20Ubuntu%20%C2%B7%20Android-2ea043)](https://yandi.su/network/)
[![License: MIT](https://img.shields.io/badge/license-MIT-lightgrey)](LICENSE)
[![Status](https://img.shields.io/badge/status-working%20prototype-orange)](#honest-status)
[![Languages](https://img.shields.io/badge/UI-English%20%C2%B7%20%D0%A0%D1%83%D1%81%D1%81%D0%BA%D0%B8%D0%B9-blue)](#languages)

[**Download**](https://yandi.su/network/) · [Install on a server](#install-a-node-console-only) · [Features](#features) · [How it works](#how-it-works) · [Honest status](#honest-status) · [Русский](README.ru.md)

</div>

---

## Download

> **Test build.** Everything below is a working prototype meant for testing. Do not rely on it where people's safety depends on it.

| Platform | Package | Version | Page with instructions |
|---|---|---|---|
| **Debian 12+ / Ubuntu 24.04+** (amd64) — node | [Debian download](https://yandi.su/network/downloads/debian/) · [Ubuntu download](https://yandi.su/network/downloads/ubuntu/) | [Current DEB manifest](https://yandi.su/update/deb/MANIFEST.json) | Installation instructions are on the download pages |
| Debian / Ubuntu — desktop shell (window) | Same download pages; install alongside the node | Same DEB manifest | The shell is optional |
| **Android** | [`yandi.apk`](https://yandi.su/update/android/yandi.apk) | [Current Android manifest](https://yandi.su/update/android/MANIFEST.json) | [Android](https://yandi.su/network/downloads/android/) |

All current downloads, checksums and release notes: **<https://yandi.su/network/>** (Russian: <https://yandi.su/ru/network/>). [GitHub Releases](https://github.com/ispolkom/node/releases) are an archive and may lag behind the site; verify the manifest of the version you download.

**Every release is signed.** Each folder on the site has a `MANIFEST.json` and `MANIFEST.json.sig` (Ed25519, with a rollback counter and a size and SHA-256 for every file). Pinned public keys, so you can check them yourself:

| What | Public value |
|---|---|
| Release manifests (`.deb`, APK) | `23a55aa8e0762e6ca9447dfa92ca401be2ba274c3571b7c3fc67bcb3c7abf43b` ([`release/yandi-release-pubkey.hex`](release/yandi-release-pubkey.hex)) |
| Android signing certificate (SHA-256) for the current site APK | `69:BE:6A:97:04:02:C2:C8:33:54:83:77:6F:AB:7D:6B:9A:29:85:A0:A2:45:37:53:46:6B:F7:28:EE:64:7E:E0` |
| List of entry nodes ([`bootstrap/bootstrap.json`](bootstrap/bootstrap.json)) | `52ba508fca1ca54bfe010dbb3d511e51ea4ee06bd59ce0816ae240745fd4025a` ([`bootstrap/signer.pub`](bootstrap/signer.pub)) |

The signing keys are derived from a developer passphrase that is never stored; the key for the entry-node list lives only inside the owner's private environment.

---

## Install a node (console only)

A server needs no window: the node is fully managed from the terminal.

```bash
# 1. install the node (Debian 12+/Ubuntu 24.04+, amd64)
sudo apt install ./yandi-node_*.deb

# 2. first setup, as a regular user (not root): language, password, recovery phrase, private environment
yandi setup

# 3. manage it
yandi admin                       # menu; asks for the administrator password (15-minute session)
yandi admin pairing owner --host YOUR_PUBLIC_ADDRESS     # pairing code + QR for your phone
```

**Write the recovery phrase (12 or 24 words) on paper** — it is shown once. Without it and the password the data cannot be restored.

The package enables and starts the systemd service `yandi-node@YOUR_USER` by itself (it waits for `yandi setup`). To open ports (`8444/tcp` for the phone app, `9000/udp` and `10000/udp` for the node network) and enlarge the network buffers, follow the step-by-step guides, in English and Russian, on the download pages above (section *Server without the shell*) or [`DEPLOYMENT.md`](DEPLOYMENT.md).

Want the window too? Also install `yandi-desktop_*.deb` — it adds the desktop shell (chats, admin panel, devices, support page). Install both packages of the same version together:

```bash
sudo apt install ./yandi-node_*.deb ./yandi-desktop_*.deb
```

Removing: `sudo apt remove yandi-node` keeps your profile; `sudo apt purge yandi-node` requests its deletion. Secure physical erasure cannot be guaranteed on SSDs or from backups.

### Connect a phone

1. Install `yandi.apk` ([download](https://yandi.su/update/android/yandi.apk)) and set a password.
2. On first start scan the pairing QR from your node (or paste the `YANDI-PAIR-1:…` text). Verify the node fingerprint independently; the phone then pins its TLS certificate for subsequent connections.
3. **No node of your own?** Enter as a **guest** through a node from the signed list of entry nodes (limited access, see [`docs/GUEST.md`](docs/GUEST.md)).
4. **Invite a person** from your own device: an 8-digit code with a 4-digit PIN (or a link), valid for 60 minutes; the administrator of the node confirms the new user.

---

## Features

| Feature | State |
|---|---|
| **Chat, files, groups and calls** through your own node; paired phones use E4 between devices, while other paths have different trust boundaries | works in tested scenarios; see [Privacy](docs/PRIVACY.md) |
| **Private environment**: owner-protected data is sealed by your password; service data needed while locked uses a device key; emergency erase | works with [documented limits](docs/PRIVACY.md) |
| **Roles**: administrator (separate password, 15-minute session), user, guest; invitations by chain of trust with a PIN, branch revocation | works |
| **Signed list of entry nodes** for phones and new nodes; a phone checks a node's TLS certificate against the key in the list | works |
| **Applications of new nodes**: a node with a public address sends only connection data (no personal data) to the nodes in the list; the owner sees them in the admin panel, with IPv4/IPv6 reachability probes | works |
| **Two languages everywhere**: English and Russian — console, installer, desktop shell, phone app | works |
| **Signed releases and installer** (`tools/install.sh`): signature and every file hash checked before anything is installed; rollback protection | works |
| **Backup of connections** (`.yandibak`) restored with the seed phrase | in progress |
| PC-to-PC chat content uses a recipient-bound YSE1 envelope inside encrypted node transport; file and call paths have their own protections | works in tested local-node scenarios; see [encryption map](docs/CRYPTO_MAP.md) |
| **Internet exit** through another node (SOCKS5 on your computer): automatic choice by country and quality, exit country checked by measurement, rotation of failing exits, route rules | works |
| **Multi-hop privacy mode**: entry → middle → exit circuit with layered encryption (equal-size cells, signed handshakes) | works in local multi-node tests; does not provide full anonymity |
| **"Fast" mode**: a single exit, close to a normal connection | works |
| **Personal gateway**: your own devices reach the internet through your own computer | works |
| **Clients behind NAT through relays**: signed availability records; only reachable nodes count (cards proved by a signed connect-back) | works |
| **TLS entry for a phone** with a decoy website for anyone who does not know the secret | works (node side) |
| Support page with the project wallets (from the signed release manifest) | works |

## How it works

- **Node** — the `yandi` program on a machine with a reachable address. It publishes a signed **card**; neighbours verify it and exchange cards like peers in a torrent swarm. For PC-to-PC chat, the communicating nodes are the YSE1 endpoints; E4 protects paired phones between devices. A phone's own node is trusted with plaintext when sending to an ordinary remote contact (see [`docs/PRIVACY.md`](docs/PRIVACY.md)).
- **Client** — a phone or a node that cannot be reached from outside (NAT). It keeps links to two relays and announces a signed record "reach me through them".
- **Exit** — a node through which traffic leaves to the internet from its own address. The owner decides who may use it: nobody / trusted nodes (default) / everyone.
- **Circuit** — a path through several nodes with layered encryption: the entry knows you but not the target; the exit knows the target but not you; the middle knows neither.
- **Closed environment** — owner-protected data is sealed by a password. Service data needed while the owner is away uses a device key; see [`docs/PRIVACY.md`](docs/PRIVACY.md).

Details: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md), [`docs/NETWORK.md`](docs/NETWORK.md), [`docs/ROLES.md`](docs/ROLES.md), [`docs/PRIVACY.md`](docs/PRIVACY.md), [`docs/CONSOLE.md`](docs/CONSOLE.md) (parts are in Russian).

## Languages

The console, the installer, `yandi --help`, the desktop shell and the phone app speak **English and Russian**. Choose with `--lang ru|en`, `YANDI_LANG`, the saved choice (`~/.config/yandi/lang`) or the system language.

## Honest status

**A working prototype, not a finished product.** The main features are verified on real processes, but there has been no independent security audit and the network is tiny.

- **No independent security audit.**
- **Encryption is path-specific.** Paired devices use E4; PC nodes use YSE1 for chat content in addition to encrypted transport. A phone's own node reads messages to ordinary remote contacts before sealing them for that contact's node. First-contact TOFU needs independent fingerprint verification. There is no ratchet or forward secrecy for stored content. See [`docs/PRIVACY.md`](docs/PRIVACY.md).
- **Known R16 issues remain open** in the closed environment and pending-message queue; see [`docs/SECURITY_STATUS.md`](docs/SECURITY_STATUS.md). A passing default test suite does not run the opt-in R16 checks.
- **Circuits do not protect against an observer who sees the whole network** (no cover traffic, no entry-node pinning).
- **Metadata is not hidden**: the node of the addressee learns who subscribes to its presence and when ([`docs/PRIVACY.md`](docs/PRIVACY.md)).
- UDP is throttled by some providers at peak hours; a TCP fallback path is planned (the TLS entry for phones already exists).
- The packages are built on Debian 12; a clean-system Ubuntu upgrade still needs verification. The phone app has automated tests and developer-reported physical call tests, but background lifecycle behavior has not been comprehensively verified for every release.
- The network must not be called anonymous until the points above are closed.

Current security findings and their status: [`docs/SECURITY_STATUS.md`](docs/SECURITY_STATUS.md). Test results are those of [CI on GitHub Actions](https://github.com/ispolkom/node/actions); older audit reports live in [`docs/history/`](docs/history/) and describe the code of their date only. Plan and limits: [`docs/ROADMAP.md`](docs/ROADMAP.md), [`docs/CRYPTO_MAP.md`](docs/CRYPTO_MAP.md).

## Build from source

Requires Rust (stable), and Flutter for the apps. On Linux, raise the UDP buffers (see [`DEPLOYMENT.md`](DEPLOYMENT.md)).

```bash
cargo build --release --workspace --bins
./target/release/yandi            # start a node; control is the shell or `yandi admin` over the local owner socket
```

A training network on one machine:

```bash
./target/release/yandi testnet up 3 && ./target/release/yandi testnet status && ./target/release/yandi testnet down
```

Checks: `scripts/check.sh` (build and quick tests), `scripts/check.sh полностью` (adds network tests on real processes; those need a machine with a public address). [`docs/INVARIANTS.md`](docs/INVARIANTS.md) lists the project's promises, each guarded by a test. Selected `chaos/` harness scripts are published; other lab material may remain private. How releases are built and signed: [`docs/RELEASE_SIGNING.md`](docs/RELEASE_SIGNING.md).

## Repository layout

```text
src/                node source code (map: docs/ARCHITECTURE.md)
mobile/             phone app (Flutter)          desktop/   desktop shell (Flutter)
bootstrap/          signed list of entry nodes and the tool that edits it
release/            public release key, release counter, signed manifests
deploy/site/        sources of the website yandi.su
tools/              installer, release and packaging scripts
docs/  tests/  scripts/  key_root/  DEPLOYMENT.md  SECURITY.md  CONTRIBUTING.md
```

## Support the project

The project wallets are shown in the app and the desktop shell (*Support the project*) and on the website; they come from the signed release manifest, so a fake wallet cannot be slipped in.

## License and security

MIT — see [`LICENSE`](LICENSE). Found a vulnerability? Do not open a public issue — read [`SECURITY.md`](SECURITY.md).
