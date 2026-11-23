# YANDI Deployment Guide

## Minimum host requirements

- Linux (kernel 5.x+, tested on Debian 12).
- Rust 1.75+ for building, unless you use a prebuilt binary.
- Open ports: 9000/UDP (discovery), 10000/UDP (data), 9001/UDP (P2P discovery), 9998/UDP (P2P data), 8443/TCP (WS-over-TLS). There is no web port: administration uses the shell's local socket.

## ⚠️ Required sysctl settings on every host

Without these settings, the kernel may drop UDP packets under load because the receive buffer is capped.

```bash
sudo sysctl -w net.core.rmem_max=67108864
sudo sysctl -w net.core.wmem_max=67108864

cat <<EOF | sudo tee /etc/sysctl.d/99-yandi.conf
net.core.rmem_max=67108864
net.core.wmem_max=67108864
net.core.netdev_max_backlog=5000
EOF
sudo sysctl -p /etc/sysctl.d/99-yandi.conf
```

At startup the node logs a warning when `rmem_max < 4 MB`; this indicates that the sysctl settings were not applied.

## Starting the node automatically with systemd

The `yandi-node` package installs the `yandi-node@.service` template (source: [`deploy/yandi-node@.service`](deploy/yandi-node@.service)) and **enables and starts it on the first installation** for the user who ran the package installation through `sudo` (or for the only regular user on the system). The node runs as that user and waits for first-time setup until `yandi setup` is run.

```bash
systemctl status yandi-node@$USER
journalctl -u yandi-node@$USER -f
sudo systemctl restart yandi-node@$USER      # the package restarts the service after an upgrade
sudo systemctl enable --now yandi-node@USERNAME # if the service was not enabled automatically
```

`YANDI_NO_SERVICE=1 sudo -E apt install …` skips service activation. If `yandi-node.service` was created manually, the package leaves it alone and does not enable a second service because the ports would be occupied twice. The older manual unit is [`deploy/yandi-node.service`](deploy/yandi-node.service).

Node ports for phones and calls (in `/etc/nftables.conf`): 8444/TCP (app ingress), 3478 TCP+UDP, and 50000–50199 UDP (call server, installed with `sudo bash scripts/setup-turn.sh <public-address>`).

## Building

```bash
cargo build --release
# binary: ./target/release/yandi
```

## Running an anchor node (a public server)

```bash
./target/release/yandi --anchor --jurisdiction NL
```

Optional:
- `--ws-bind 0.0.0.0:443` — custom WS server bind address (default: 8443).
- `--jurisdiction XX` or `--my-jurisdiction XX` — ISO-3166 alpha-2 self-claim for the exit policy.

## Running the mobile client (a lightweight peer)

```bash
./target/release/yandi --mobile \
  --anchor-url wss://anchor.example.com:443/ \
  --anchor-fp <TLS-sha256-fingerprint-hex>
```

Optional:
- `--anchor-store /path/to/paired_anchors.json` — custom storage path.
- `--import-pairing '<QR-string>'` — import a pairing payload from a QR code once.
- `--exit-jurisdiction DE` — prefer exits in the specified country.

## Bootstrap list

A new node finds the network through the **signed list of entry nodes** (`bootstrap/bootstrap.json` in the repository). It is downloaded at startup and every six hours, checked against the pinned key, and saved to disk. See [`bootstrap/README.md`](bootstrap/README.md) for details and instructions for the owner who updates the list.

The node owner may also place a local `nodes/bootstrap.json` file next to the program. It does not require a signature and is not committed to the repository; see `.gitignore`:
```json
{
  "auto_connect": true,
  "connect_on_startup": true,
  "nodes": [
    { "name": "MY-NODE-1", "address": "203.0.113.10:9000", "jurisdiction": "XX", "enabled": true, "role": "entry",
      "ed25519_fingerprint": "<64 hex: node key; if set, a node replacement at this address is rejected>" }
  ]
}
```

## Configuration and state files

All of them live under `~/.yandi/`:

- `config.yaml` (optional) — general configuration; see `[ws] bind` and related settings.
- `paired_clients.json` — the anchor stores paired-client permissions: the client key and expiry, **without secrets** (v2 format, mode `0600`; an old format containing secrets is rewritten on first read, without carrying over its permissions, and requires pairing again).
- `paired_anchors.json` — the client stores an ordered list of anchors (address, TLS fingerprint, public key) **without secrets** (v2 format; the old format is rewritten while preserving addresses and order). Set its path with `--anchor-store`; backup and restore include it.
- `tls/cert.pem`, `tls/key.pem` — self-signed TLS identity, generated automatically.
- `node_identity_*.json` — Ed25519/X25519 key pairs, generated on first startup.

## Web UI

After startup, administration goes through the local socket (`curl --unix-socket ~/.local/share/yandi/run/yandi-ui.sock http://localhost/...`). Pairing endpoints:
- `GET /pair/qr` — SVG QR code containing a pairing payload.
- `GET /pair/qr.json` — JSON for copying and pasting.
- `POST /pair/issue` — the anchor issues a session token for a public key.

## HTTP/SOCKS5 proxy

After starting the node, use these CLI commands:
```
> proxy <SHORT_ID>     # HTTP proxy → ::8080, traffic through peer SHORT_ID
> socks5 <SHORT_ID>    # SOCKS5 proxy → ::1080
> peers                # list known peers
```

## Verification

After startup, verify the following:
- `Bootstrap complete: N connected` — bootstrap nodes connected.
- `Session vN established with peer: <id>` — an ECDH session was established.
- `ws-server 🌐 listening on 0.0.0.0:8443` — the TLS fingerprint was displayed.
- There are no warnings about `rmem_max`.

## Stress test (after the first deployment)

```bash
# 8 parallel downloads through the proxy
for i in 1 2 3 4 5 6 7 8; do
  curl -s -o /dev/null -x http://127.0.0.1:8080 \
    -w "#$i: %{speed_download} B/s\n" \
    --max-time 60 https://speed.cloudflare.com/__down?bytes=20000000 &
done; wait

# UDP-drops:
awk 'NR>1 && $13!="0" { port=strtonum("0x"substr($2,index($2,":")+1)); printf "port=%d drops=%s\n", port, $13 }' /proc/net/udp | sort -u
```

Expected result: approximately 60–80 Mbps in total, depending on the ISP connection, with `drops=0`.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| Browser white screen under load | RCVBUF is too small and the kernel drops UDP packets | `sudo sysctl rmem_max=67108864` |
| `Bootstrap complete: 0 connected` | Bootstrap nodes are unavailable | Check the firewall: 9000–10000/UDP |
| `ws-server bind 8443 failed: Permission denied` | You want to bind to :443 without root | Use `--ws-bind 0.0.0.0:8443` or `setcap cap_net_bind_service=+ep` on the binary |
| Slow proxy without shaping | Count `/proc/net/udp` drops; if they increase, apply the sysctl settings | See above |

---

See also:
- `NETWORK_EVOLUTION_PLAN.md` — architecture roadmap.
- `NETWORK_EVOLUTION_STATUS.md` — current implementation status.
- `FUTURE_IDEAS.md` — backlog of architectural ideas (PEP, swarming, mask mode, and so on).
