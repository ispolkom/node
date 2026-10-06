# YANDI Node

**A peer-to-peer network for free communication: end-to-end encrypted chat, files and calls between people, access to the internet through nodes in other jurisdictions, circuits through several nodes.** One binary, `yandi`, is a network node with a built-in web interface. No central server, no accounts.

> Русская версия: [README.md](README.md)

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
./target/release/yandi            # start a node; control page: http://127.0.0.1:9999
```

On first start the node opens a small page on `127.0.0.1`: create a login password and a master password in the browser; the node then creates its identity (keys, stored encrypted with the master password). Everything else is configured on the web page ("Settings").

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

## What is not there yet (honestly)

- **No independent security audit.**
- **No end-to-end encryption between people through intermediaries**: every link is encrypted, not "person to person" as a whole, so a relay could read a relayed conversation. This is invariant I2, marked "pending".
- **Circuits do not protect against an observer who sees the whole network**: no cover traffic and no entry-node pinning (the entry is chosen at random).
- **UDP is throttled by some providers at peak hours**; a TCP fallback path is planned, the TLS entry for phones already exists.
- **The node sends every packet twice** between nodes (original and copy) — a deliberate reliability choice that halves the speed ceiling.
- **The mobile app** (`mobile/yandi_mobile`, Flutter) lags behind the node: the proxy password and certificate pinning are not wired in yet.
- The network must not be called anonymous until the points above are closed.

Full list and plan: [`docs/ROADMAP.md`](docs/ROADMAP.md), [`docs/CRYPTO_MAP.md`](docs/CRYPTO_MAP.md).

## Repository layout

```text
src/                node source code (map: docs/ARCHITECTURE.md)
tests/              unit, guard and network tests (testnet_*_test.rs run on real processes)
docs/               architecture, network, invariants, wire format, crypto map, roadmap
bootstrap/          signed list of entry nodes and the signing tool
mobile/             mobile app (Flutter), behind the node
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
