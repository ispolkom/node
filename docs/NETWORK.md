# Network: Roles, Modes, and Node Discovery

## Roles

- **Node** — the `yandi` program on a machine with a **reachable public address**. Only such machines become network nodes: they publish cards and may act as exits or relays. Reachability is checked in practice through a reverse connection, not by declaration.
- **Client** — a node or device that cannot be reached from outside, such as a phone or a device behind an ISP or router NAT. A client does not publish a node card, keeps outbound connections to two relays, and publishes a signed record saying “find me through these relays”. When its address changes, the client reconnects to the relays and issues a new record.
- **Relay** — an ordinary node with a verified address (`relay` in its card; disabled by `relay.json` `{"enabled": false}` or `YANDI_RELAY=off`). The client connects to it itself, so the outbound connection passes through NAT; the relay can extend a circuit to the client over that connection. It sees only encrypted layers.
- **Exit** — a node through which traffic reaches the Internet from its address. The node owner decides who may exit (`/api/exit/settings`, the “Settings” page): *nobody* / *trusted only* (default) / *everyone*. Connections to the node's own computer and home network are always blocked.

## How nodes discover each other

1. **Bootstrap.** A new node takes the signed list of entry nodes ([`bootstrap/`](../bootstrap/README.md)), checks it with the pinned key, and connects to the entries. It also checks the entry-node key: the node answering at the listed address must be that node.
2. **Cards.** Every node publishes a **card**: country (and how it was learned), whether it has a public address, capacity, latency, addresses, and offered services (exit or relay). A card is signed by the node key and lives for two hours; it is reissued every 30 minutes and immediately after a change such as an external-address change.
3. **Neighbor exchange.** A node sends its own card and some known cards from other nodes to every neighbor, like peer lists in torrents. Forged, expired, foreign-key, and unchecked cards are discarded.
4. **Reachability check.** Every accepted card is checked through a reverse connection: the node connects by TCP to the declared address on the core transport port and requires a signature from the card key over a fresh random value. If the node does not answer, the card remains “unchecked”: it is absent from country lists, is not selected for exits or circuits, and is not forwarded. A foreign card cannot make nodes connect to home networks: probes target public addresses only.
5. **Clients.** A client availability record (`ViaRecord`) is distributed through the same exchange. Anyone who knows the client's number and key, such as one of the owner's trusted devices, can find its relays through that record.

## Internet access modes

| Mode | How to start | What happens |
|---|---|---|
| **Fast** | `socks5 auto [NL]`, `POST /api/socks5/start/auto[-NL]`, settings page | Direct path through one exit; the exit sees the address, but cannot see HTTPS contents. |
| **Fast, selected sites only** | `…/start/rules[-NL]` | Sites from the rules, either the built-in list or `route_rules.json`, go through an exit. Other traffic goes directly from your computer; only Internet addresses are allowed. |
| **Anonymous** | `socks5 hops [NL]`, `…/start/hops[-NL]` | Entry → middle → exit circuit with layered encryption; no single node knows both you and the destination. The exit opens **web ports 80 and 443 only**. |
| **Personal gateway** | `POST /api/socks5/personal` | Your device reaches the Internet through your own computer, at home or behind NAT, optionally through a relay, using the owner's secret (`GET /api/devices/pairing`). Any ports are allowed; the home network is blocked. |

The exit is selected automatically: country and connection filters, quality ordering based on known country, capacity, and latency, rotation across connections, failed-exit cooldown (60 seconds, then twice as long), and **factual country verification**. The node asks the exit through the exit itself which country it is in and excludes an exit that lies.

## TLS entry for phones

When enabled by the owner (`YANDI_MOBILE_TLS_PORT=443` or `mobile_tls.json` `{"port": 443}`), the node accepts TLS with a self-signed certificate. `GET /api/devices/pairing` displays its fingerprint, and the node forwards bytes to its local SOCKS5 proxy. A peer that did not start with a SOCKS5 greeting receives an ordinary site page, from `decoy/index.html` in the data directory or the embedded copy, and receives 404 for everything else. To an ISP this looks like HTTPS within one country.

## Reciprocity

A node that could provide Internet exit service according to its card (public address and sufficient capacity) but has disabled exits loses free “for everyone” exit assistance after a grace volume of 100 MB. The owner sees a clear warning on the settings page. Trusted nodes, phones, and low-capacity machines are exempt.

## Environment settings

| Variable | Purpose |
|---|---|
| `YANDI_BOOTSTRAP_URL` | Alternate entry-node list address; separate multiple addresses with commas. |
| `YANDI_CLIENT_ONLY=1` | Treat this node as a client: do not publish a card and operate through relays. |
| `YANDI_RELAY=off` | Do not act as a relay. |
| `YANDI_MOBILE_TLS_PORT` | TLS entry port for phones. |
| `YANDI_EXIT_PORTS=any` | Allow any destination port through a circuit exit; tests only. |
| `YANDI_GEO_PROBE=address:port` | Country service used to verify exits; tests only. |
| `YANDI_TESTNET=1` | Test mode: probes and checks may use addresses on this machine. |

## Known limitations

- Circuits have no cover traffic or entry-node pinning, and no flow control; a fast source can fill the send queue. Cells in one circuit may arrive out of order; the stream reassembles them by sequence number.
- A client does not return to node mode by itself. A relay has no per-client limit beyond the global circuit limit.
- A client's availability record is visible to everyone: it publicly announces that the client is online and identifies its relays.
- The exit country is learned from the card or external address; only measurement through the exit verifies it.
