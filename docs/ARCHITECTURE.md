# Architecture and Code Map

A single `yandi` executable (`src/main.rs`) starts the node: communication with other nodes, the messaging channel, the management web page on `127.0.0.1`, proxying, and Internet exit. The library (`src/lib.rs`) contains the same modules used by the tests.

## Layers

```text
        management web page (src/web)          CLI (netlayer/cli.rs)
                      │                                   │
   ┌──────────────────┴───────────────────────────────────┴──────────────────┐
   │ Node network: cards, reachability, exit, circuits, relays                │
   │ network_offers · reachability · exit_select · exit_policy · reciprocity  │
   │ hops / hops_net · relay_net · route_rules · mobile_tls · bootstrap       │
   ├──────────────────────────────────────────────────────────────────────────┤
   │ Core node transport (UDP): netlayer + protocol (YTP “trains and wagons”)│
   │ DHT (dht) · NAT/relay (netlayer) · proxies and tunnels (socks5/proxy)   │
   ├──────────────────────────────────────────────────────────────────────────┤
   │ Human channel: messaging, files, calls (p2p + communication + media)    │
   ├──────────────────────────────────────────────────────────────────────────┤
   │ Link encryption: crypto (X25519 + HKDF + AES-256-GCM, counters, windows) │
   └──────────────────────────────────────────────────────────────────────────┘
```

Two communication channels between nodes live side by side: the **core network** (`netlayer::P2PTransport`, discovery and data ports — node lists, DHT, proxies, and circuits) and the **human channel** (`p2p::P2PTransport` — messaging, files, and calls). Both use the shared link encryption from `crypto`, with one-time keys for every handshake.

## Module map

### Encryption and format

| Module | Purpose |
|---|---|
| `src/crypto/` | Session package: counter, replay window, key rotation, X25519 key exchange, and one-time-key handshake. The format is fixed by golden vectors (`docs/WIRE.md`). |
| `src/core/identity.rs` | Node identity: signing and exchange keys, stored encrypted with the owner's password. |
| `src/util/` | Utilities: data directory, private files (`private_file`), address masking, and system measurements. |

### Core transport

| Module | Purpose |
|---|---|
| `src/netlayer/` | Core node transport: hellos and signatures, peers, NAT, external-address detection, WebSocket/TLS entry, relaying (`relay.rs`), and the command line (`cli.rs`). |
| `src/protocol/` | YTP (“trains and wagons”): wagon splitting, ordering, retransmission requests, rate control, stations, and exit tunnels. |
| `src/dht/` | Distributed table (Kademlia): node records, jurisdiction index, and group records. |
| `src/dataplane/`, `src/connectors/`, `src/state_manager/`, `src/observability/` | Adaptive transport, connectors including obfuscation, state management, and metrics. |

### Proxy and exit

| Module | Purpose |
|---|---|
| `src/socks5/` | SOCKS5: the server on your computer (`server.rs`) and the exit handler on an exit node (`exit_node.rs`). |
| `src/proxy/` | HTTP proxy and gateway. |
| `src/exit_policy.rs` | **Who** may exit through the node (nobody / trusted / everyone) and **where** it may not connect: the node's own computer and home network are always blocked. |
| `src/exit_select.rs` | Automatic exit selection from the card directory: country and connection filters, quality ordering, rotation, failed-exit cooldown, and **measuring the exit country through the exit itself**. |
| `src/reciprocity.rs` | Reciprocity: a node that could help but refuses is limited after its grace volume; transferred bytes are counted. |
| `src/route_rules.rs` | Per-site routing rules: what goes through an exit and what goes directly. |

### Node network

| Module | Purpose |
|---|---|
| `src/network_offers.rs` | Signed **node cards** (country, capacity, and offered services), directory, exchange with neighbors, and country lists. |
| `src/reachability.rs` | Card reachability check through a **signed reverse connection**; unchecked cards are neither used nor distributed. |
| `src/hops.rs`, `src/hops_net.rs` | **Multi-node circuits** with layered encryption: a pure core (telescope construction, signed handshakes, equal-size cells, replay windows) and connection to a live node (exit, SOCKS, or personal gateway). |
| `src/relay_net.rs` | NAT clients: signed “reachable through relays” records, client mode, and relay discovery. |
| `src/mobile_tls.rs` | TLS entry for phones with a site-like cover. |
| `src/bootstrap/` | Signed entry-node list and owner tools (`yandi bootstrap …`); see [`bootstrap/README.md`](../bootstrap/README.md). |
| `src/mdns/` | Node discovery on the local network. |

### Human channel

| Module | Purpose |
|---|---|
| `src/p2p/` | Human channel: hello, handshake, and sessions. |
| `src/communication/` | Messaging (storage and read receipts), files, profiles, and groups. |
| `src/media/` | Calls: codecs and sessions. |
| `src/p2p_tunnel/`, `src/tunnel/`, `src/apps/` | Tunnels through the human channel, UDP tunnel, and application layer. |

### Web page

| Module | Purpose |
|---|---|
| `src/web/` | Management page (`ui/*.html`), password login, trusted nodes through a card (`peers.rs`), and API. On Linux it listens only through the owner-only local socket; on other platforms it uses the loopback web port and Host-header protection. |

### Supporting modules

| Module | Purpose |
|---|---|
| `src/supervisor.rs` | Supervision of long-lived tasks: name, restart policy, termination reason, and `GET /api/kernel/status`. |
| `src/testnet.rs` | **Test network**: `yandi testnet up N` starts N copies on one machine with separate directories, ports, and passwords; network tests run there. |
| `key_root/` | Separate node root-key utility. |

## How a connection passes through the network

1. The node listens for neighbors and exchanges **cards**; it checks every accepted card through a reverse connection (`reachability`). Verified cards enter the directory.
2. The user starts a proxy: “fast” (`socks5 auto`) or “anonymous” (`socks5 hops`). The application (browser) connects to `127.0.0.1` through SOCKS5.
3. For every connection, `exit_select` chooses an exit (country, quality, and queue). In “anonymous” mode, `hops_net` builds an entry → middle → exit circuit.
4. The exit (`socks5/exit_node.rs` or `hops_net`) connects to the destination only at an Internet address and only under the owner's rules (`exit_policy`).
5. A background check periodically asks the exit which country it is in (`exit_select::probe_country`) and excludes an exit that lied.

## Default ports

| Port | Purpose |
|---|---|
| 9000/UDP | Core node transport (discovery); the reachability check also listens on this number over **TCP**. |
| 10000/UDP | Core transport data. |
| 9001/UDP, 9998/UDP | Human channel: discovery and data. |
| — | Administration: the owner's Unix socket `~/.local/share/yandi/run/yandi-ui.sock` on Linux; there is no browser TCP port there. Other platforms use the loopback web UI port. |
| 8443/TCP | WebSocket-over-TLS entry (anchor). |
| Optional (`YANDI_MOBILE_TLS_PORT`, 443 recommended) | TLS entry for phones. |

In the test network, ports are allocated in blocks starting at `26000 + 100·k`.
