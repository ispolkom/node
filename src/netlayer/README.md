# YANDI Netlayer Module

**Complete P2P networking layer with TUN, SOCKS5, and HTTP Proxy support**
**Status:** ✅ Fully implemented
**Last updated:** 2025-12-29

---

## 📊 Module Overview

The Netlayer module provides complete P2P networking functionality including:
- ✅ Peer discovery and management
- ✅ Encrypted P2P transport (UDP)
- ✅ TUN device support (IPv6)
- ✅ SOCKS5 proxy
- ✅ HTTP proxy (DPI bypass)
- ✅ CLI commands

---

## 📦 Module Structure

```
src/netlayer/
├── mod.rs                 # Module declarations
├── peer.rs                # Peer information tracking
├── packet.rs              # Packet types and serialization
├── transport.rs           # P2P UDP transport (ports 9000, 10000)
├── encryption.rs          # ECDH encryption for peer sessions
├── tunnel.rs              # YTP tunnel management
├── cli.rs                 # Interactive CLI commands
├── bootstrap.rs           # Bootstrap node management
├── external_ip.rs         # External IP detection
├── interface_detector.rs  # Network interface discovery
├── node_introspection.rs  # Node capabilities detection
├── tun_device.rs          # TUN device management (Entry Node)
└── tun_exit.rs            # TUN exit handler (Exit Node)

Total: 13 files, ~4000+ lines of code
```

---

## 🔌 Core Components

### 1. Peer Management (`peer.rs`)

**PeerInfo** - Information about P2P peers:

```rust
pub struct PeerInfo {
    pub id: HashId,
    pub addr: String,
    pub local_addr: Option<String>,
    pub public_addr: Option<String>,
    pub ipv6_virtual: Option<[u8; 16]>,
    pub last_seen: u128,
}
```

**Key methods:**
- `new(id, addr)` - Create basic peer info
- `with_ipv6(...)` - Add IPv6 virtual address
- `touch()` - Update last_seen timestamp
- `from_identity_with_ipv6(...)` - Create from NodeIdentity

---

### 2. P2P Transport (`transport.rs`)

**P2PTransport** - Main UDP transport for P2P communication:

**Features:**
- Dual UDP sockets:
  - Port 9000: Discovery and signaling (Hello, Helo, etc.)
  - Port 10000: Data channel (YTP trains, TunWagon, etc.)
- ECDH encryption for peer sessions
- TunWagon routing (entry → exit)
- TunWagonResponse routing (exit → entry)
- Message type routing (0x01-0xFF)

**Key methods:**
- `new(identity, external_ip)` - Create transport
- `find_peer_by_short_id(short_id)` - Find peer by 4/6/8 byte ID
- `tun_wagon_tx()` - Get TunWagon sender channel
- `set_tun_wagon_channel(tx)` - Set TunWagon channel
- `with_handlers(...)` - Configure message handlers

**Message types:**
```rust
0x01 = HELLO_REQ
0x02 = HELLO_ACK
0x10 = HELO
0x20 = ENCRYPTED_MESSAGE
0x30 = TRAIN (YTP)
0x40 = HTTP_REQUEST
0x41 = HTTP_RESPONSE
0x50 = SOCKS5_CONNECT
0x51 = SOCKS5_DATA
0x60 = TUN_WAGON
0x61 = TUN_WAGON_RESPONSE
```

---

### 3. TUN Device Support (`tun_device.rs`, `tun_exit.rs`)

**YandiTunDevice** - TUN device for IPv6 packet encapsulation:

**Features:**
- Create virtual TUN interfaces (yandi_client, yandi_p2p)
- IPv6 packet parsing and encapsulation
- TunWagon creation and transmission
- TunWagonResponse handling
- Non-blocking background packet processing

**Key methods:**
- `new(name, ipv6)` - Create TUN device
- `start()` - Start packet processing (non-blocking)
- `set_tun_wagon_channel(tx)` - Connect TunWagon sender
- `set_exit_node(peer_id)` - Configure exit node
- `set_transport(transport)` - Connect to P2P transport

**TunExitHandler** - Exit node for TUN traffic:

**Features:**
- Receive TunWagon from entry nodes
- Extract IPv6 packets
- Create TCP connections to internet
- Return TunWagonResponse

---

### 4. SOCKS5 Proxy

Integrated SOCKS5 proxy for browser and application use:

**CLI command:**
```bash
socks5 <SHORT_ID>
```

**Usage:**
- Firefox: Settings → Network → Manual proxy → SOCKS Host: 127.0.0.1:1081
- Chrome: `--proxy-server="socks5://127.0.0.1:1081"`

**Protocol:** Full SOCKS5 implementation with IPv6 support

---

### 5. HTTP Proxy (DPI Bypass)

HTTP proxy with DPI bypass capabilities:

**CLI command:**
```bash
proxy <SHORT_ID>
```

**Features:**
- HTTP/HTTPS support
- DPI bypass techniques
- Header modification
- Connection pooling

---

### 6. CLI Commands (`cli.rs`)

**Available commands:**

| Command | Description |
|---------|-------------|
| `hello <IP:PORT>` | Send Hello request to peer |
| `peers` | Show known peers with Short IDs |
| `socks5 <SHORT_ID>` | Start SOCKS5 proxy through peer |
| `proxy <SHORT_ID>` | Start HTTP proxy through peer |
| `tun init` | Initialize TUN devices |
| `tun link <SHORT_ID>` | Link TUN to exit node |
| `tun exit` | Start TUN Exit Node |
| `tun status` | Show TUN device status |
| `tun info` | Show TUN device info |
| `exit` | Start Exit Node Handler |
| `help` | Show help |
| `quit` | Stop node |

---

## 🏗️ Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     Entry Node                              │
│  ┌──────────────┐         ┌──────────────────┐             │
│  │ Application  │         │   YandiTunDevice │             │
│  │  (curl, etc) │         │   (yandi_client) │             │
│  └──────┬───────┘         └────────┬─────────┘             │
│         │                          │                        │
│         │ IPv6 packet              │ TunWagon              │
│         │                          │                        │
│         ▼                          ▼                        │
│  ┌─────────────────────────────────────────────────┐        │
│  │              P2PTransport                        │        │
│  │  Port 9000: Discovery (Hello, Helo)            │        │
│  │  Port 10000: Data (YTP, TunWagon, etc.)        │        │
│  └────────────────────┬────────────────────────────┘        │
│                       │                                     │
└───────────────────────┼─────────────────────────────────────┘
                        │ UDP encrypted
                        │
┌───────────────────────┼─────────────────────────────────────┐
│                       │                                     │
│  ┌────────────────────┼────────────────────────────────┐   │
│  │            P2PTransport (Exit Node)                 │   │
│  │  Port 10000: Receive TunWagon                     │   │
│  └────────────────────┼────────────────────────────────┘   │
│                       │                                     │
│                       ▼                                     │
│  ┌─────────────────────────────────────────────────┐        │
│  │            TunExitHandler                       │        │
│  │  - Parse TunWagon                              │        │
│  │  - Extract IPv6 packet                         │        │
│  │  - Connect to Internet                         │        │
│  │  - Return TunWagonResponse                     │        │
│  └─────────────────────────────────────────────────┘        │
│                       │                                     │
│                       ▼                                     │
│              ┌─────────────┐                               │
│              │  Internet   │                               │
│              └─────────────┘                               │
└─────────────────────────────────────────────────────────────┘
```

---

## 🔐 Security

### Encryption
- **ECDH key exchange** for peer session establishment
- **ChaCha20** encryption for data packets
- **Ed25519** digital signatures

### NAT Traversal
- STUN-like external IP detection
- UDP hole punching
- Reverse connection activation

---

## 📖 Usage Examples

### Start Entry Node with TUN

```bash
./target/release/yandi

> tun init
> peers                           # Get exit node Short ID
> tun link b0eb0b0ab09c9924      # Link to exit node

# Test IPv6 through TUN
curl -6 --interface yandi_client http://ipv6.google.com/ -I
```

### Start Exit Node

```bash
./target/release/yandi

> tun exit
```

### SOCKS5 Proxy for Browser

```bash
./target/release/yandi

> socks5 b0eb0b0ab09c9924

# In browser: 127.0.0.1:1081 (SOCKS5)
```

### HTTP Proxy

```bash
./target/release/yandi

> proxy b0eb0b0ab09c9924

# In browser: 127.0.0.1:8080 (HTTP)
```

---

## 🚀 Compilation

```bash
cargo check              # Check compilation (11s)
cargo build --release    # Release build (50s)
```

**Result:** `/home/user/yandi/target/release/yandi`

---

## 📝 Dependencies

- `tokio` - Async runtime
- `tun` - TUN device creation
- `serde` - Serialization
- `x25519-dalek` - ECDH
- `chacha20poly1305` - Encryption
- `ed25519-dalek` - Signatures

---

## 🎯 Status

| Component | Status | Notes |
|-----------|--------|-------|
| Peer discovery | ✅ Complete | Hello/Helo exchange |
| P2P transport | ✅ Complete | Dual UDP sockets |
| Encryption | ✅ Complete | ECDH + ChaCha20 |
| SOCKS5 proxy | ✅ Complete | Full implementation |
| HTTP proxy | ✅ Complete | With DPI bypass |
| TUN entry | ✅ Complete | IPv6 encapsulation |
| TUN exit | ✅ Complete | Internet gateway |
| CLI | ✅ Complete | All commands |

---

## 📚 Related Documentation

- `/tmp/TUN_ENTRY_NODE_COMPLETE.md` - TUN implementation details
- `/tmp/TUN_USAGE_RU.md` - TUN usage instructions (Russian)
- `/tmp/TUN_TEST_BROWSER.md` - Browser testing guide

---

**Last Updated:** 2025-12-29
**Status:** ✅ Production ready
**Total LOC:** ~4000+ lines across 13 files
