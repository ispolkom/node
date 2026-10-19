# YANDI Node

**A peer-to-peer network for free communication: chat, files and calls encrypted on every link (end-to-end encryption between phones in the app; between nodes, the asynchronous end-to-end layer is still in design, see "What is not there yet"), access to the internet through nodes in other jurisdictions, circuits through several nodes.** One binary, `yandi`, is a network node with a built-in web interface. No central server, no accounts.

> Русская версия: [README.ru.md](README.ru.md)

> **Status: a working prototype, not a finished product.** The main features are verified on real processes, but the project has had no independent security audit and the network is still tiny. Do not rely on it where people's safety depends on it. See "What is not there yet" below.

## Features

| Feature | State |
|---|---|
| Chat, files and call signalling between nodes, encrypted per link (X25519 + AES-256-GCM, fresh keys for every handshake, replay protection) | works, verified on 3 nodes |
| Exit to the internet through another node (SOCKS5 on your computer) | works |
| **Automatic exit choice** from the node directory by country and quality, rotation, resting of failing exits | works |
| **Exit country checked by measurement**: the node asks the exit itself where it is and drops one that lies | works |
| **"Anonymous" mode**: entry → middle → exit circuit with layered encryption (equal-size cells, signed handshakes) | works, verified on 4 nodes |
| **"Fast" mode**: a single exit, close to a normal connection | works |
| **Route rules**: listed sites through the exit, everything else directly | works |
| **Only reachable nodes count**: cards are proved by a signed connect-back | works |
| **Clients behind NAT through relays**: signed availability records | works |
| **Personal gateway**: your own devices reach the internet through your own computer, using a secret only you hold | works |
| **TLS entry for a phone (port 443)** with a decoy website for anyone who does not know the secret | works (node side) |
| Reciprocity: a node that could help but refuses loses access to others' help, with an honest warning | works (for exits) |
| Trusted nodes by business card | works |
| Signed bootstrap list so new nodes find the network | implemented and tested; **the list ships without entry nodes** — the owner adds their own (see [`bootstrap/`](bootstrap/)) |

## How the network is organised

- **Node** — the `yandi` program on a machine with a reachable (public) address. A node publishes a **card** (country, power, what it offers) signed with its key; neighbours verify it and exchange cards like peers in a torrent swarm.
- **Client** — a node or device that cannot be reached from outside (behind NAT, a phone). It keeps links to two **relays** and announces a signed record "reach me through them".
- **Exit** — a node through which traffic leaves to the internet from its own address. The owner decides who may use it: nobody / trusted nodes only (default) / everyone.
- **Circuit** — a path through several nodes with layered encryption: the entry knows you but not the target; the exit knows the target but not you; the middle knows neither.
- Details: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md), [`docs/NETWORK.md`](docs/NETWORK.md) (in Russian).

## Quick start

Requires Rust (stable). On Linux, raise the UDP buffers (see [`DEPLOYMENT.md`](DEPLOYMENT.md)).

```bash
cargo build --release
./target/release/yandi            # start a node; control is the YANDI shell over the local owner socket (no TCP port)
```

On first start the node waits for the first setup, from the YANDI shell or from the console: `yandi setup` asks for a password, creates the keys and the closed environment and shows a recovery phrase (12 or 24 words, shown only in a terminal or written to a new 0600 file). A server needs no shell: `yandi admin` is a console menu with sub-commands (pairing code and QR, devices, proxies, the closed environment, erase), and `tools/install.sh` installs from a signed release (the signature and every file hash are checked before anything is installed) with a choice between "node only" and "node and shell". See [`docs/CONSOLE.md`](docs/CONSOLE.md).

**A training network on one machine** (several node copies with their own folders and ports):

```bash
./target/release/yandi testnet up 3
./target/release/yandi testnet status
./target/release/yandi testnet down
```

**Your own exit through the network, from the command line:**

```text
socks5 auto [NL]     fast: the exit is chosen automatically, country optional
socks5 hops [NL]     anonymous: a circuit through three nodes
```

(or on the settings page, or `POST /api/socks5/start/{auto|hops|rules}[-NL]`). The proxy listens on `127.0.0.1` only; every node has its own password.

## Testing

```bash
scripts/check.sh              # build and quick tests
scripts/check.sh полностью    # plus network tests on real processes ("полностью" = "fully")
cargo test --offline          # tests only
```

Network tests (`tests/testnet_*_test.rs`, marked `#[ignore]`) start real node processes on one machine. The exit, circuit, personal-gateway and rules tests need the machine to have a **public address** (the echo server is placed on it), so they do not run on ordinary CI runners. Run them one at a time: `cargo test --offline --test testnet_hops_test -- --ignored --test-threads=1` (they share ports).

[`docs/INVARIANTS.md`](docs/INVARIANTS.md) is a short list of the project's promises; each is guarded by a test, and `tests/invariants.rs` keeps the document in sync with the tests.

## People, roles and privacy

Every node has three kinds of participants (details: [`docs/ROLES.md`](docs/ROLES.md), in Russian):

- **Administrator** — the node's owner. Manages the node (complaints, pairing codes and requests, gateways, node settings, deleting groups, warning or removing users) from the **Admin panel** in the desktop shell. The panel opens with a separate administrator password for 15 minutes; the server enforces it on every management route. The administrator does not read anyone's conversations and does not write "as admin".
- **User** — a person whose phone was accepted by the administrator (or the owner's own personal account on the PC). Has private chats, creates groups, uses the node as an internet exit.
- **Guest** — a phone connected without a QR code, with limits (no groups, no internet exit, no calls, short-lived mail and files; see [`docs/GUEST.md`](docs/GUEST.md)).

Private data lives in a **closed environment** unlocked by the user's password; one seed phrase restores both the node and that environment. An emergency erase button destroys the environment's key and files. What the node logs, keeps and for how long is described in [`docs/PRIVACY.md`](docs/PRIVACY.md) (in Russian): addresses are hidden in logs, mail on the node expires (7 days by default).

## What is not there yet (honestly)

- **No independent security audit.**
- **No asynchronous end-to-end layer between nodes.** Between two nodes the session is encrypted between the endpoints (by code reading, a UDP relay forwards datagrams and holds no keys of the inner session; this has not been tested across several nodes or on devices). This applies to the UDP relay path only: an anchor is the endpoint of its WebSocket channel, and the exit node of a proxy chain sees the bytes it writes to the internet target. A phone that writes to an external computer through its own node is read by that node (a gateway), but there is no sealed-envelope layer for offline delivery, no forward secrecy for stored content, no network-wide distribution of nodes' encryption keys and no human key verification yet. The corrected analysis and the plan are in [`docs/E2E_ARCHITECTURE.md`](docs/E2E_ARCHITECTURE.md). Invariant I2 stays "pending".
- **Circuits do not protect against an observer who sees the whole network**: no cover traffic and no entry-node pinning (the entry is chosen at random).
- **UDP is throttled by some providers at peak hours**; a TCP fallback path is planned, the TLS entry for phones already exists.
- **The node sends every packet twice** between nodes (original and copy) — a deliberate reliability choice that halves the speed ceiling.
- **The phone app** (`mobile/yandi_mobile`, Flutter) is young: it works against your own node (pairing by QR, TLS pinned to the node's certificate fingerprint), but it has had far less testing than the node, and groups, calls and invitations are still being refined.
- The network must not be called anonymous until the points above are closed.

Full list and plan: [`docs/ROADMAP.md`](docs/ROADMAP.md), [`docs/CRYPTO_MAP.md`](docs/CRYPTO_MAP.md).

## Repository layout

```text
src/                node source code (map: docs/ARCHITECTURE.md)
tests/              unit, guard and network tests (testnet_*_test.rs run on real processes)
docs/               architecture, network, invariants, wire format, crypto map, roadmap
bootstrap/          signed list of entry nodes and the signing tool
mobile/             phone app (Flutter)
key_root/           node root-key utility
scripts/check.sh    all checks in one command
.github/workflows/  builds on Linux/Windows/macOS, network tests, dependency checks
DEPLOYMENT.md       server deployment: ports, sysctl, systemd
SECURITY.md         how to report a vulnerability
CONTRIBUTING.md     how to contribute
```

## License

MIT (see [`LICENSE`](LICENSE)).

## Security

Found a vulnerability? Do not file a public issue — read [`SECURITY.md`](SECURITY.md).
