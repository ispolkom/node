// src/netlayer/packet.rs
//! Network Packet Types
//! ====================
//!
//! Binary packet format for P2P communication

use crate::util::{HashId, NodeName};
use serde::{Serialize, Deserialize};

pub const NET_PACKET_HEADER_LEN: usize = 36;
pub const MIN_PACKET_SIZE: usize = 512;

/// 🌍 Iter 3: глобальный jurisdiction (ISO-3166 alpha-2). Выставляется один раз из CLI
/// (`--jurisdiction XX`) при старте main.rs и читается всеми Hello-конструкторами,
/// чтобы не таскать его через все сигнатуры. None — нода не объявляет юрисдикцию.
static NODE_JURISDICTION: once_cell::sync::OnceCell<String> = once_cell::sync::OnceCell::new();

pub fn set_node_jurisdiction(j: String) {
    let _ = NODE_JURISDICTION.set(j);
}

pub fn node_jurisdiction() -> Option<String> {
    normalize_jurisdiction(NODE_JURISDICTION.get().cloned())
}

/// Юрисдикция в приветствии — до 8 байт (по границе символа); пустая — это «не указана». Значение приводится к этому виду при создании пакета,
/// чтобы подписанное совпадало с передаваемым.
pub fn normalize_jurisdiction(j: Option<String>) -> Option<String> {
    let j = j?;
    let mut end = j.len().min(8);
    while !j.is_char_boundary(end) {
        end -= 1;
    }
    let t = &j[..end];
    if t.is_empty() { None } else { Some(t.to_string()) }
}

/// 🆕 Hardening Step 6: exit-jurisdiction preference. Если задан — circuit-builder
/// предпочитает exit'ов из этой страны при отборе кандидатов.
static EXIT_JURISDICTION: once_cell::sync::OnceCell<String> = once_cell::sync::OnceCell::new();

pub fn set_exit_jurisdiction(j: String) {
    let _ = EXIT_JURISDICTION.set(j);
}

pub fn exit_jurisdiction() -> Option<String> {
    EXIT_JURISDICTION.get().cloned()
}

/// Hardening Step 6: override пути к paired_anchors.json (полезно для тестов).
static ANCHOR_STORE_OVERRIDE: once_cell::sync::OnceCell<String> = once_cell::sync::OnceCell::new();

pub fn set_anchor_store_override(path: String) {
    let _ = ANCHOR_STORE_OVERRIDE.set(path);
}

pub fn anchor_store_override() -> Option<String> {
    ANCHOR_STORE_OVERRIDE.get().cloned()
}

#[cfg(test)]
mod hardening_step6_tests {
    use super::*;

    #[test]
    fn exit_jurisdiction_setter() {
        // OnceCell в этих тестах одноразовый — тест пишет только если cell пустая.
        // Проверяем что getter возвращает то что установили (без зависимости от
        // глобального порядка тестов: либо установим, либо уже установлено).
        if exit_jurisdiction().is_none() {
            set_exit_jurisdiction("DE".to_string());
            assert_eq!(exit_jurisdiction().as_deref(), Some("DE"));
        } else {
            assert!(exit_jurisdiction().is_some());
        }
    }

    #[test]
    fn anchor_store_override_setter() {
        if anchor_store_override().is_none() {
            set_anchor_store_override("/tmp/yandi-test/anchors.json".to_string());
            assert_eq!(anchor_store_override().as_deref(), Some("/tmp/yandi-test/anchors.json"));
        } else {
            assert!(anchor_store_override().is_some());
        }
    }
}

/// Packet types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketType {
    Handshake = 1,
    Chat = 2,
    Ping = 3,
    Pong = 4,
    Heartbeat = 5,
    Dht = 6,
    Hello = 14,
    HelloAck = 15,
    PeerList = 16,
    // Control commands (0x20-0x2F)
    StartExitHandler = 0x20,
    ExitHandlerStarted = 0x21,
    ExitHandlerError = 0x22,
    StopExitHandler = 0x23,
    // RawIP Tunnel control (0x24-0x27)
    StartRawIpTunnel = 0x24,
    RawIpTunnelStarted = 0x25,
    RawIpTunnelError = 0x26,
    StopRawIpTunnel = 0x27,
    // HTTP Proxy control (0x30-0x3F)
    StartProxyGateway = 0x30,
    ProxyGatewayStarted = 0x31,
    ProxyGatewayError = 0x32,
    StopProxyGateway = 0x33,
    // SOCKS5 Proxy control (0x34-0x36)
    StartSocks5Gateway = 0x34,
    Socks5GatewayStarted = 0x35,
    Socks5GatewayError = 0x36,
    StopSocks5Gateway = 0x37,
    // HTTP Proxy data (0x40-0x4F)
    ProxyRequest = 0x40,
    ProxyResponse = 0x41,
    ProxyResponseFragment = 0x42,
    // SOCKS5 Proxy data (0x50-0x5F)
    Socks5Request = 0x50,
    Socks5Response = 0x51,
    Socks5TunnelData = 0x52,
    // NAT Relay control (0x60-0x6F)
    RelayConnectRequest = 0x60,
    RelayConnectResponse = 0x61,
    RelayData = 0x62,
    RelayClose = 0x63,
    RelayHeartbeat = 0x64,
}

impl PacketType {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            1 => Some(PacketType::Handshake),
            2 => Some(PacketType::Chat),
            3 => Some(PacketType::Ping),
            4 => Some(PacketType::Pong),
            5 => Some(PacketType::Heartbeat),
            6 => Some(PacketType::Dht),
            14 => Some(PacketType::Hello),
            15 => Some(PacketType::HelloAck),
            16 => Some(PacketType::PeerList),
            0x20 => Some(PacketType::StartExitHandler),
            0x21 => Some(PacketType::ExitHandlerStarted),
            0x22 => Some(PacketType::ExitHandlerError),
            0x23 => Some(PacketType::StopExitHandler),
            0x30 => Some(PacketType::StartProxyGateway),
            0x31 => Some(PacketType::ProxyGatewayStarted),
            0x32 => Some(PacketType::ProxyGatewayError),
            0x33 => Some(PacketType::StopProxyGateway),
            0x34 => Some(PacketType::StartSocks5Gateway),
            0x35 => Some(PacketType::Socks5GatewayStarted),
            0x36 => Some(PacketType::Socks5GatewayError),
            0x37 => Some(PacketType::StopSocks5Gateway),
            0x40 => Some(PacketType::ProxyRequest),
            0x41 => Some(PacketType::ProxyResponse),
            0x42 => Some(PacketType::ProxyResponseFragment),
            // SOCKS5 Proxy
            0x50 => Some(PacketType::Socks5Request),
            0x51 => Some(PacketType::Socks5Response),
            0x52 => Some(PacketType::Socks5TunnelData),
            // NAT Relay
            0x60 => Some(PacketType::RelayConnectRequest),
            0x61 => Some(PacketType::RelayConnectResponse),
            0x62 => Some(PacketType::RelayData),
            0x63 => Some(PacketType::RelayClose),
            0x64 => Some(PacketType::RelayHeartbeat),
            _ => None,
        }
    }

    pub fn to_byte(self) -> u8 {
        self as u8
    }
}

/// Network packet
#[derive(Debug, Clone)]
pub struct NetPacket {
    pub packet_type: PacketType,
    pub sender: HashId,
    pub encrypted: bool,
    pub obfuscated: bool,
    pub payload: Vec<u8>,
}

impl NetPacket {
    pub fn new(packet_type: PacketType, sender: HashId, encrypted: bool, payload: Vec<u8>) -> Self {
        Self {
            packet_type,
            sender,
            encrypted,
            obfuscated: false,
            payload,
        }
    }

    pub fn is_encrypted(&self) -> bool {
        self.encrypted
    }

    pub fn is_obfuscated(&self) -> bool {
        self.obfuscated
    }

    /// Serialize to binary format
    pub fn to_bytes(&self) -> Vec<u8> {
        let payload_len = u16::try_from(self.payload.len())
            .expect("Payload too large");

        let mut out = Vec::with_capacity(NET_PACKET_HEADER_LEN + self.payload.len());

        // flags
        let mut flags = 0u8;
        if self.encrypted {
            flags |= 0b0000_0001;
        }
        if self.obfuscated {
            flags |= 0b0000_0010;
        }
        out.push(flags);

        // packet_type
        out.push(self.packet_type.to_byte());

        // sender_id
        out.extend_from_slice(self.sender.as_ref());

        // payload_len
        out.extend_from_slice(&payload_len.to_be_bytes());

        // payload
        out.extend_from_slice(&self.payload);

        // Add padding to reach minimum packet size (traffic analysis protection)
        let current_len = out.len();
        if current_len < MIN_PACKET_SIZE {
            let pad_len = MIN_PACKET_SIZE - current_len;
            out.extend(vec![0u8; pad_len]);
        }

        out
    }

    /// Deserialize from binary format
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < NET_PACKET_HEADER_LEN {
            return None;
        }

        let flags = data[0];
        let encrypted = (flags & 0b0000_0001) != 0;
        let obfuscated = (flags & 0b0000_0010) != 0;

        let packet_type = PacketType::from_byte(data[1])?;

        let mut sender = [0u8; 32];
        sender.copy_from_slice(&data[2..34]);

        let payload_len = u16::from_be_bytes([data[34], data[35]]) as usize;

        if data.len() < NET_PACKET_HEADER_LEN + payload_len {
            return None;
        }

        let payload = data[NET_PACKET_HEADER_LEN..NET_PACKET_HEADER_LEN + payload_len].to_vec();

        Some(Self {
            packet_type,
            sender: HashId(sender),
            encrypted,
            obfuscated,
            payload,
        })
    }
}

/// Hello packet types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HelloType {
    Request = 0,
    Ack = 1,
}

/// Signature wrapper for serde compatibility (arrays > 32 bytes)
#[derive(Debug, Clone)]
pub struct Signature(pub [u8; 64]);

impl serde::Serialize for Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Serialize as byte array
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for Signature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SignatureVisitor;

        impl<'de> serde::de::Visitor<'de> for SignatureVisitor {
            type Value = Signature;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a 64-byte signature")
            }

            fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if v.len() != 64 {
                    return Err(serde::de::Error::invalid_length(v.len(), &self));
                }
                let mut sig = [0u8; 64];
                sig.copy_from_slice(v);
                Ok(Signature(sig))
            }
        }

        deserializer.deserialize_bytes(SignatureVisitor)
    }
}

impl AsRef<[u8]> for Signature {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Hello packet for peer discovery
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloPacket {
    pub hello_type: HelloType,
    pub version: u8,
    pub node_id: HashId,
    pub node_name: NodeName,  // Self-certifying identity: SHA256(public_key)
    pub public_key: [u8; 32],  // Ed25519 signing public key
    pub x25519_public: [u8; 32],  // X25519 ECDH public key for session key derivation
    pub signature: Signature,   // Ed25519-подпись ВСЕХ остальных полей (см. challenge_data)
    pub cid: [u8; 8],
    pub capabilities: u16,
    pub timestamp: u64,
    pub nonce: u64,
    pub wan_address: Option<String>,
    pub lan_address: Option<String>,
    pub discovery_endpoint: Option<String>,
    pub ipv6_virtual: Option<[u8; 16]>,
    /// Real external IPv6 address (if available)
    pub ipv6_external: Option<String>,
    /// P2P Communication address (port 9998, MTU 65536) - for Chat, Files, Voice, Video
    pub p2p_data_addr: Option<String>,
    /// P2P X25519 public key for E2E encryption (separate from netlayer)
    pub p2p_x25519_public: Option<[u8; 32]>,
    /// 🌍 Iter 3: ISO-3166 alpha-2 country code self-claim ("US","DE","NL"). Без валидации.
    /// Используется для выбора foreign-exit anchor'ов при построении circuit'ов.
    pub jurisdiction: Option<String>,
}

impl HelloPacket {
    pub const MAGIC: [u8; 4] = *b"NET1";
    pub const CURRENT_VERSION: u8 = 1;

    pub fn new_request(
        node_id: HashId,
        public_key: [u8; 32],
        x25519_public: [u8; 32],
        cid: [u8; 8],
        capabilities: u16,
    ) -> Self {
        let node_name = NodeName::from_public_key(&public_key);
        Self {
            hello_type: HelloType::Request,
            version: Self::CURRENT_VERSION,
            node_id,
            node_name,
            public_key,
            x25519_public,
            signature: Signature([0u8; 64]),
            cid,
            capabilities,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            nonce: rand::random::<u64>(),
            wan_address: None,
            lan_address: None,
            discovery_endpoint: None,
            ipv6_virtual: None,
            ipv6_external: None,
            p2p_data_addr: None,
            p2p_x25519_public: None,
            jurisdiction: node_jurisdiction(),
        }
    }

    pub fn new_ack(
        node_id: HashId,
        public_key: [u8; 32],
        x25519_public: [u8; 32],
        cid: [u8; 8],
        capabilities: u16,
        request_nonce: u64,
    ) -> Self {
        Self::new_ack_with_ipv6(node_id, public_key, x25519_public, cid, capabilities, request_nonce, None)
    }

    pub fn new_ack_with_ipv6(
        node_id: HashId,
        public_key: [u8; 32],
        x25519_public: [u8; 32],
        cid: [u8; 8],
        capabilities: u16,
        request_nonce: u64,
        ipv6_virtual: Option<[u8; 16]>,
    ) -> Self {
        let node_name = NodeName::from_public_key(&public_key);
        Self {
            hello_type: HelloType::Ack,
            version: Self::CURRENT_VERSION,
            node_id,
            node_name,
            public_key,
            x25519_public,
            signature: Signature([0u8; 64]),
            cid,
            capabilities,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            nonce: request_nonce,
            wan_address: None,
            lan_address: None,
            discovery_endpoint: None,
            ipv6_virtual,
            ipv6_external: None,
            p2p_x25519_public: None,
            p2p_data_addr: None,
            jurisdiction: node_jurisdiction(),
        }
    }

    pub fn with_discovery_endpoint(mut self, endpoint: String) -> Self {
        self.discovery_endpoint = Some(endpoint);
        self
    }

    pub fn with_ipv6(mut self, ipv6_bytes: [u8; 16]) -> Self {
        self.ipv6_virtual = Some(ipv6_bytes);
        self
    }

    pub fn with_ipv6_external(mut self, ipv6_external: String) -> Self {
        self.ipv6_external = Some(ipv6_external);
        self
    }

    pub fn with_p2p_data_addr(mut self, p2p_addr: String) -> Self {
        self.p2p_data_addr = Some(p2p_addr);
        self
    }

    pub fn with_jurisdiction(mut self, jurisdiction: String) -> Self {
        self.jurisdiction = normalize_jurisdiction(Some(jurisdiction));
        self
    }

    pub fn with_p2p_x25519_public(mut self, p2p_key: [u8; 32]) -> Self {
        self.p2p_x25519_public = Some(p2p_key);
        self
    }

    /// Serialize Hello packet to bytes
    /// Format: [MAGIC:4][type:1][version:1][node_id:32][node_name:32][public_key:32][signature:64][cid:8][caps:2][timestamp:8][nonce:8][wan_len:1][wan_data][lan_len:1][lan_data][endpoint_len:1][endpoint_data][ipv6_flag:1][ipv6_data:16][ipv6_ext_len:1][ipv6_ext_data][p2p_addr_len:1][p2p_addr_data]
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        use std::io::Write;

        // Строки не усекаются молча (усечённое значение не совпало бы с подписанным); пустая строка и «нет значения» на проводе неразличимы, поэтому пустая запрещена.
        for (name, v, max) in [
            ("wan_address", &self.wan_address, 255usize),
            ("lan_address", &self.lan_address, 255),
            ("discovery_endpoint", &self.discovery_endpoint, 255),
            ("ipv6_external", &self.ipv6_external, 255),
            ("p2p_data_addr", &self.p2p_data_addr, 255),
            ("jurisdiction", &self.jurisdiction, 8),
        ] {
            if let Some(t) = v {
                if t.is_empty() || t.len() > max {
                    return Err(format!("Hello field {name}: length {} not allowed (1..={max})", t.len()));
                }
            }
        }
        let mut buffer = Vec::new();

        // Magic
        buffer.write_all(&Self::MAGIC).map_err(|e| e.to_string())?;

        // Hello type
        buffer.write_all(&[self.hello_type as u8]).map_err(|e| e.to_string())?;

        // Version
        buffer.write_all(&[self.version]).map_err(|e| e.to_string())?;

        // Node ID (32 bytes)
        buffer.write_all(&self.node_id.0).map_err(|e| e.to_string())?;

        // Node name (32 bytes) - self-certifying identity
        buffer.write_all(&self.node_name.0).map_err(|e| e.to_string())?;

        // Public key (32 bytes) - Ed25519
        buffer.write_all(&self.public_key).map_err(|e| e.to_string())?;

        // X25519 public key (32 bytes) - ECDH
        buffer.write_all(&self.x25519_public).map_err(|e| e.to_string())?;

        // Signature (64 bytes)
        buffer.write_all(&self.signature.0).map_err(|e| e.to_string())?;

        // CID (8 bytes)
        buffer.write_all(&self.cid).map_err(|e| e.to_string())?;

        // Capabilities (u16 big-endian)
        buffer.write_all(&self.capabilities.to_be_bytes()).map_err(|e| e.to_string())?;

        // Timestamp (u64 big-endian)
        buffer.write_all(&self.timestamp.to_be_bytes()).map_err(|e| e.to_string())?;

        // Nonce (u64 big-endian)
        buffer.write_all(&self.nonce.to_be_bytes()).map_err(|e| e.to_string())?;

        // WAN address (optional)
        if let Some(ref addr) = self.wan_address {
            let addr_bytes = addr.as_bytes();
            let len = addr_bytes.len().min(255) as u8;
            buffer.write_all(&[len]).map_err(|e| e.to_string())?;
            buffer.write_all(&addr_bytes[..len as usize]).map_err(|e| e.to_string())?;
        } else {
            buffer.write_all(&[0u8]).map_err(|e| e.to_string())?;
        }

        // LAN address (optional)
        if let Some(ref addr) = self.lan_address {
            let addr_bytes = addr.as_bytes();
            let len = addr_bytes.len().min(255) as u8;
            buffer.write_all(&[len]).map_err(|e| e.to_string())?;
            buffer.write_all(&addr_bytes[..len as usize]).map_err(|e| e.to_string())?;
        } else {
            buffer.write_all(&[0u8]).map_err(|e| e.to_string())?;
        }

        // Discovery endpoint (optional)
        if let Some(ref endpoint) = self.discovery_endpoint {
            let endpoint_bytes = endpoint.as_bytes();
            let len = endpoint_bytes.len().min(255) as u8;
            buffer.write_all(&[len]).map_err(|e| e.to_string())?;
            buffer.write_all(&endpoint_bytes[..len as usize]).map_err(|e| e.to_string())?;
        } else {
            buffer.write_all(&[0u8]).map_err(|e| e.to_string())?;
        }

        // IPv6 virtual address (optional)
        if let Some(ref ipv6) = self.ipv6_virtual {
            buffer.write_all(&[1u8]).map_err(|e| e.to_string())?;
            buffer.write_all(ipv6).map_err(|e| e.to_string())?;
        } else {
            buffer.write_all(&[0u8]).map_err(|e| e.to_string())?;
        }

        // IPv6 external address (optional string)
        if let Some(ref ipv6_ext) = self.ipv6_external {
            let ipv6_bytes = ipv6_ext.as_bytes();
            let len = ipv6_bytes.len().min(255) as u8;
            buffer.write_all(&[len]).map_err(|e| e.to_string())?;
            buffer.write_all(&ipv6_bytes[..len as usize]).map_err(|e| e.to_string())?;
        } else {
            buffer.write_all(&[0u8]).map_err(|e| e.to_string())?;
        }

        // P2P data address (optional string) - port 9999
        if let Some(ref p2p_addr) = self.p2p_data_addr {
            let p2p_bytes = p2p_addr.as_bytes();
            let len = p2p_bytes.len().min(255) as u8;
            buffer.write_all(&[len]).map_err(|e| e.to_string())?;
            buffer.write_all(&p2p_bytes[..len as usize]).map_err(|e| e.to_string())?;
        } else {
            buffer.write_all(&[0u8]).map_err(|e| e.to_string())?;
        }

        // P2P X25519 public key (optional, 32 bytes)
        if let Some(ref p2p_key) = self.p2p_x25519_public {
            buffer.write_all(&[1u8]).map_err(|e| e.to_string())?;
            buffer.write_all(p2p_key).map_err(|e| e.to_string())?;
        } else {
            buffer.write_all(&[0u8]).map_err(|e| e.to_string())?;
        }

        // 🌍 Iter 3: jurisdiction (optional ASCII string, max 8 байт). [len:1][data]
        if let Some(ref j) = self.jurisdiction {
            let jb = j.as_bytes();
            let len = jb.len().min(8) as u8;
            buffer.write_all(&[len]).map_err(|e| e.to_string())?;
            buffer.write_all(&jb[..len as usize]).map_err(|e| e.to_string())?;
        } else {
            buffer.write_all(&[0u8]).map_err(|e| e.to_string())?;
        }

        Ok(buffer)
    }

    /// Deserialize Hello packet from bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self, String> {
        if data.len() < 4 {
            return Err("Packet too short".to_string());
        }

        // Check magic
        let magic = &data[0..4];
        if magic != Self::MAGIC.as_ref() {
            return Err(format!("Invalid magic: {:?}", magic));
        }

        let mut pos = 4;

        // Hello type
        let hello_type = match data[pos] {
            0 => HelloType::Request,
            1 => HelloType::Ack,
            _ => return Err(format!("Invalid hello type: {}", data[pos])),
        };
        pos += 1;

        // Version
        let version = data[pos];
        pos += 1;

        // Node ID (32 bytes)
        if data.len() < pos + 32 {
            return Err("Packet truncated at node_id".to_string());
        }
        let mut node_id_bytes = [0u8; 32];
        node_id_bytes.copy_from_slice(&data[pos..pos + 32]);
        let node_id = HashId(node_id_bytes);
        pos += 32;

        // Node name (32 bytes) - self-certifying identity
        if data.len() < pos + 32 {
            return Err("Packet truncated at node_name".to_string());
        }
        let mut node_name_bytes = [0u8; 32];
        node_name_bytes.copy_from_slice(&data[pos..pos + 32]);
        let node_name = NodeName(node_name_bytes);
        pos += 32;

        // Public key (32 bytes) - Ed25519
        if data.len() < pos + 32 {
            return Err("Packet truncated at public_key".to_string());
        }
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(&data[pos..pos + 32]);
        pos += 32;

        // X25519 public key (32 bytes) - ECDH
        if data.len() < pos + 32 {
            return Err("Packet truncated at x25519_public".to_string());
        }
        let mut x25519_public = [0u8; 32];
        x25519_public.copy_from_slice(&data[pos..pos + 32]);
        pos += 32;

        // Signature (64 bytes)
        if data.len() < pos + 64 {
            return Err("Packet truncated at signature".to_string());
        }
        let mut signature = [0u8; 64];
        signature.copy_from_slice(&data[pos..pos + 64]);
        let signature = Signature(signature);
        pos += 64;

        // CID (8 bytes)
        if data.len() < pos + 8 {
            return Err("Packet truncated at cid".to_string());
        }
        let mut cid = [0u8; 8];
        cid.copy_from_slice(&data[pos..pos + 8]);
        pos += 8;

        // Capabilities (u16)
        if data.len() < pos + 2 {
            return Err("Packet truncated at capabilities".to_string());
        }
        let capabilities = u16::from_be_bytes([data[pos], data[pos + 1]]);
        pos += 2;

        // Timestamp (u64)
        if data.len() < pos + 8 {
            return Err("Packet truncated at timestamp".to_string());
        }
        let timestamp = u64::from_be_bytes([
            data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
            data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7],
        ]);
        pos += 8;

        // Nonce (u64)
        if data.len() < pos + 8 {
            return Err("Packet truncated at nonce".to_string());
        }
        let nonce = u64::from_be_bytes([
            data[pos], data[pos + 1], data[pos + 2], data[pos + 3],
            data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7],
        ]);
        pos += 8;

        // WAN address (optional)
        let wan_address = if data.len() > pos {
            let len = data[pos] as usize;
            pos += 1;
            if len > 0 && data.len() >= pos + len {
                let addr = String::from_utf8_lossy(&data[pos..pos + len]).to_string();
                pos += len;
                Some(addr)
            } else {
                None
            }
        } else {
            None
        };

        // LAN address (optional)
        let lan_address = if data.len() > pos {
            let len = data[pos] as usize;
            pos += 1;
            if len > 0 && data.len() >= pos + len {
                let addr = String::from_utf8_lossy(&data[pos..pos + len]).to_string();
                pos += len;
                Some(addr)
            } else {
                None
            }
        } else {
            None
        };

        // Discovery endpoint (optional)
        let discovery_endpoint = if data.len() > pos {
            let len = data[pos] as usize;
            pos += 1;
            if len > 0 && data.len() >= pos + len {
                let endpoint = String::from_utf8_lossy(&data[pos..pos + len]).to_string();
                pos += len;
                Some(endpoint)
            } else {
                None
            }
        } else {
            None
        };

        // IPv6 virtual address (optional)
        let ipv6_virtual = if data.len() > pos {
            let flag = data[pos];
            pos += 1;
            if flag == 1 && data.len() >= pos + 16 {
                let mut ipv6 = [0u8; 16];
                ipv6.copy_from_slice(&data[pos..pos + 16]);
                pos += 16;
                Some(ipv6)
            } else {
                None
            }
        } else {
            None
        };

        // IPv6 external address (optional string)
        let ipv6_external = if data.len() > pos {
            let len = data[pos] as usize;
            pos += 1;
            if len > 0 && data.len() >= pos + len {
                let ipv6 = String::from_utf8_lossy(&data[pos..pos + len]).to_string();
                pos += len; // без сдвига все следующие поля читались с неверного места
                Some(ipv6)
            } else {
                None
            }
        } else {
            None
        };

        // P2P data address (optional string) - port 9999
        let p2p_data_addr = if data.len() > pos {
            let len = data[pos] as usize;
            pos += 1;
            if len > 0 && data.len() >= pos + len {
                let addr = String::from_utf8_lossy(&data[pos..pos + len]).to_string();
                pos += len; // без сдвига ключ канала переписки и юрисдикция читались с неверного места и терялись
                Some(addr)
            } else {
                None
            }
        } else {
            None
        };

        // P2P X25519 public key (optional, 32 bytes)
        let p2p_x25519_public = if data.len() > pos {
            let flag = data[pos];
            pos += 1;
            if flag == 1 && data.len() >= pos + 32 {
                let mut key = [0u8; 32];
                key.copy_from_slice(&data[pos..pos + 32]);
                pos += 32;
                Some(key)
            } else {
                None
            }
        } else {
            None
        };

        // 🌍 Iter 3: jurisdiction (optional ASCII, max 8B). Backward-compatible: если данных нет — None.
        let jurisdiction = if data.len() > pos {
            let len = data[pos] as usize;
            pos += 1;
            if len > 0 && len <= 8 && data.len() >= pos + len {
                let s = String::from_utf8_lossy(&data[pos..pos + len]).to_string();
                Some(s)
            } else {
                None
            }
        } else {
            None
        };

        Ok(Self {
            hello_type,
            version,
            node_id,
            node_name,
            public_key,
            x25519_public,
            signature,
            cid,
            capabilities,
            timestamp,
            nonce,
            wan_address,
            lan_address,
            discovery_endpoint,
            ipv6_virtual,
            ipv6_external,
            p2p_data_addr,
            p2p_x25519_public,
            jurisdiction,
        })
    }

    /// Check if this is a Request packet
    pub fn is_request(&self) -> bool {
        self.hello_type == HelloType::Request
    }

    /// Check if this is an Ack packet
    pub fn is_ack(&self) -> bool {
        self.hello_type == HelloType::Ack
    }

    /// Verify that node_name matches public_key (self-certifying check)
    /// This prevents identity spoofing
    pub fn verify_node_name(&self) -> bool {
        self.node_name.verify_public_key(&self.public_key)
    }

    /// Что подписывает узел в приветствии: ВСЕ поля, кроме самой подписи, в однозначной записи, с меткой назначения подписи.
    ///
    /// Раньше подпись охватывала только номер узла, ключи, адрес IPv6, время и номер приветствия: тип приветствия, версия, возможности, внешний/локальный адрес,
    /// точка обнаружения, адрес канала переписки и юрисдикция оставались БЕЗ подписи, и тот, кто стоит на пути пакета, мог подправить адреса (перенаправить
    /// ответы, отразить трафик на чужой адрес) и заявленные возможности («я ретранслятор»), не ломая подпись.
    pub fn challenge_data(&self) -> Vec<u8> {
        fn opt_str(out: &mut Vec<u8>, v: &Option<String>) {
            match v {
                Some(s) => {
                    out.push(1);
                    out.extend_from_slice(&(s.len() as u32).to_be_bytes());
                    out.extend_from_slice(s.as_bytes());
                }
                None => out.push(0),
            }
        }
        fn opt_bytes<const N: usize>(out: &mut Vec<u8>, v: &Option<[u8; N]>) {
            match v {
                Some(b) => {
                    out.push(1);
                    out.extend_from_slice(b);
                }
                None => out.push(0),
            }
        }
        let mut d = Vec::with_capacity(256);
        d.extend_from_slice(b"yandi-hello-v2\0");
        d.push(self.hello_type as u8);
        d.push(self.version);
        d.extend_from_slice(&self.node_id.0);
        d.extend_from_slice(&self.node_name.0);
        d.extend_from_slice(&self.public_key);
        d.extend_from_slice(&self.x25519_public);
        d.extend_from_slice(&self.cid);
        d.extend_from_slice(&self.capabilities.to_be_bytes());
        d.extend_from_slice(&self.timestamp.to_be_bytes());
        d.extend_from_slice(&self.nonce.to_be_bytes());
        opt_str(&mut d, &self.wan_address);
        opt_str(&mut d, &self.lan_address);
        opt_str(&mut d, &self.discovery_endpoint);
        opt_bytes(&mut d, &self.ipv6_virtual);
        opt_str(&mut d, &self.ipv6_external);
        opt_str(&mut d, &self.p2p_data_addr);
        opt_bytes(&mut d, &self.p2p_x25519_public);
        opt_str(&mut d, &self.jurisdiction);
        d
    }


    /// Verify the signature on this HelloPacket
    /// Uses Ed25519 verification
    pub fn verify_signature(&self) -> bool {
        use ed25519_dalek::Verifier;

        // First check node_name matches public_key
        if !self.verify_node_name() {
            return false;
        }

        // Verify signature
        let verifying_key = match ed25519_dalek::VerifyingKey::from_bytes(&self.public_key) {
            Ok(key) => key,
            Err(_) => return false,
        };

        let challenge = self.challenge_data();
        let sig = ed25519_dalek::Signature::from_bytes(&self.signature.0);

        verifying_key.verify(&challenge, &sig).is_ok()
    }

    /// Verify IPv6 virtual address (Stage 2.2: Prevent IPv6 spoofing)
    /// Checks that ipv6_virtual is derived from node_id (not spoofed)
    /// Format: fc00:1234:5678::<first 8 bytes of node_id>
    ///
    /// Returns:
    /// - true if ipv6_virtual matches node_id
    /// - false if mismatch or ipv6_virtual is None
    pub fn verify_ipv6_virtual(&self) -> bool {
        let ipv6_bytes = match self.ipv6_virtual {
            Some(bytes) => bytes,
            None => return false,  // No IPv6 provided
        };

        // Expected IPv6: fc00:1234:5678::<first 8 bytes of node_id>
        // Last 8 bytes should match first 8 bytes of node_id
        let expected_suffix = &self.node_id.0[..8];
        let actual_suffix = &ipv6_bytes[8..16];

        expected_suffix == actual_suffix
    }

    /// Get expected IPv6 virtual address for this node
    /// Returns the IPv6 that should be used (derived from node_id)
    pub fn expected_ipv6_virtual(&self) -> [u8; 16] {
        let mut ipv6 = [0u8; 16];
        // Prefix: fc00:1234:5678::
        ipv6[0] = 0xfc;
        ipv6[1] = 0x00;
        ipv6[2] = 0x12;
        ipv6[3] = 0x34;
        ipv6[4] = 0x56;
        ipv6[5] = 0x78;
        ipv6[6] = 0x00;
        ipv6[7] = 0x00;
        // Suffix: first 8 bytes of node_id
        ipv6[8..16].copy_from_slice(&self.node_id.0[..8]);
        ipv6
    }
}

/// Hello capabilities flags. Узел объявляет о своих возможностях через биты в caps.
/// Чужой узел читает их и принимает решения (выбор relay'я, выбор introducer'а и т.п.).
pub mod hello_caps {
    pub const SUPERBOOT: u16 = 0x0001;
    pub const RELAY: u16 = 0x0002;
    pub const TUNNEL: u16 = 0x0004;
    pub const DHT: u16 = 0x0008;
    pub const GATEWAY: u16 = 0x0010;
    pub const NAT_TRAVERSAL: u16 = 0x0020;
    pub const MESH: u16 = 0x0040;
    pub const ENCRYPTED: u16 = 0x0080;
    /// 0x0100 — Mobile (lite-клиент): не сервер, не relay, не DHT-сервер.
    pub const MOBILE: u16 = 0x0100;
    /// 0x0200 — Anchor: 24/7 узел с public IP/port-mapping, accepts paired clients.
    pub const ANCHOR: u16 = 0x0200;
    /// 0x0400 — Introducer: умеет hole-punching посредничество.
    pub const INTRODUCER: u16 = 0x0400;
    /// 🆕 0x0800 — Hardening Step 7: TELESCOPING_HANDSHAKE — узел умеет ECDH-based
    /// circuit-extend (X25519 ephemeral pubkey в EXTEND, EXTEND_REPLY с hop'овым).
    /// Старые узлы без этого бита получают classical EXTEND (без pubkey'а) и продолжают
    /// работать с deterministic derive_hop_key. Caps-биты — backward-compat lever.
    pub const TELESCOPING_HANDSHAKE: u16 = 0x0800;
}

/// NAT Relay structures for future relay implementation

/// Relay connection request
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RelayConnectRequest {
    pub source_peer: HashId,
    pub target_peer: HashId,
    pub session_id: u64,
    pub timestamp: u64,
}

/// Relay connection response
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RelayConnectResponse {
    pub source_peer: HashId,
    pub target_peer: HashId,
    pub session_id: u64,
    pub accepted: bool,
    pub reason: Option<String>,
}

/// Relay data packet
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RelayDataPacket {
    pub session_id: u64,
    pub source_peer: HashId,
    pub target_peer: HashId,
    pub sequence: u32,
    pub data: Vec<u8>,
}

impl RelayDataPacket {
    pub fn new(session_id: u64, source_peer: HashId, target_peer: HashId, data: Vec<u8>) -> Self {
        Self {
            session_id,
            source_peer,
            target_peer,
            sequence: 0,
            data,
        }
    }

    pub fn with_sequence(mut self, seq: u32) -> Self {
        self.sequence = seq;
        self
    }
}

/// Relay close packet
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RelayClosePacket {
    pub session_id: u64,
    pub reason: String,
}

impl RelayClosePacket {
    pub fn new(session_id: u64, reason: String) -> Self {
        Self {
            session_id,
            reason,
        }
    }
}

#[cfg(test)]
mod hello_jurisdiction_tests {
    use super::*;

    fn fake_hello_request() -> HelloPacket {
        HelloPacket::new_request(
            HashId([0x42u8; 32]),
            [0x11u8; 32],
            [0x22u8; 32],
            [0x33u8; 8],
            0u16,
        )
    }

    #[test]
    fn jurisdiction_roundtrip() {
        let hello = fake_hello_request().with_jurisdiction("DE".to_string());
        let bytes = hello.to_bytes().expect("encode");
        let parsed = HelloPacket::from_bytes(&bytes).expect("decode");
        assert_eq!(parsed.jurisdiction.as_deref(), Some("DE"));
    }

    #[test]
    fn jurisdiction_optional_default_none() {
        let hello = fake_hello_request();
        let bytes = hello.to_bytes().unwrap();
        let parsed = HelloPacket::from_bytes(&bytes).unwrap();
        assert!(parsed.jurisdiction.is_none());
    }

    #[test]
    fn jurisdiction_truncated_to_8_bytes() {
        let hello = fake_hello_request().with_jurisdiction("UNITED-STATES".to_string());
        let bytes = hello.to_bytes().unwrap();
        let parsed = HelloPacket::from_bytes(&bytes).unwrap();
        let j = parsed.jurisdiction.expect("present");
        assert_eq!(j.len(), 8);
        assert_eq!(j, "UNITED-S");
    }
}

#[cfg(test)]
mod hello_signature_tests {
    use super::*;
    use crate::core::NodeIdentity;

    fn signed_hello(kind: HelloType) -> (HelloPacket, NodeIdentity) {
        let id = NodeIdentity::new();
        let mut h = match kind {
            HelloType::Request => HelloPacket::new_request(id.node_id(), id.signing_public_key, [0x22; 32], [0x33; 8], 0x0ABC),
            HelloType::Ack => HelloPacket::new_ack(id.node_id(), id.signing_public_key, [0x22; 32], [0x33; 8], 0x0ABC, 77),
        };
        h.wan_address = Some("203.0.113.9:10000".into());
        h.lan_address = Some("192.168.1.5:10000".into());
        h.discovery_endpoint = Some("0.0.0.0:10000".into());
        h.ipv6_virtual = Some([0xfc; 16]);
        h.ipv6_external = Some("2001:db8::1".into());
        h.p2p_data_addr = Some("0.0.0.0:9998".into());
        h.p2p_x25519_public = Some([0x55; 32]);
        h.jurisdiction = Some("DE".into());
        let sig = id.sign(&h.challenge_data()).unwrap();
        let mut b = [0u8; 64];
        b.copy_from_slice(&sig);
        h.signature = Signature(b);
        (h, id)
    }

    /// Приветствие, пройдя через запись в байты и разбор, должно давать те же подписываемые данные (иначе подпись ломается на приёме).
    #[test]
    fn the_signed_bytes_survive_the_wire() {
        for kind in [HelloType::Request, HelloType::Ack] {
            let (h, _) = signed_hello(kind);
            let back = HelloPacket::from_bytes(&h.to_bytes().unwrap()).unwrap();
            assert_eq!(h.challenge_data(), back.challenge_data(), "{kind:?}: после передачи по сети подписываемые данные другие");
            assert!(back.verify_signature(), "{kind:?}");
        }
    }

    #[test]
    fn long_or_empty_strings_are_refused_not_silently_changed() {
        let (mut h, _) = signed_hello(HelloType::Request);
        h.wan_address = Some("x".repeat(256));
        assert!(h.to_bytes().unwrap_err().contains("wan_address"));
        h.wan_address = Some(String::new());
        assert!(h.to_bytes().is_err());
        h.wan_address = Some("x".repeat(255));
        assert!(h.to_bytes().is_ok(), "255 — предел");
        h.jurisdiction = Some("123456789".into());
        assert!(h.to_bytes().unwrap_err().contains("jurisdiction"));
    }

    #[test]
    fn a_signed_hello_verifies() {
        for kind in [HelloType::Request, HelloType::Ack] {
            let (h, _) = signed_hello(kind);
            assert!(h.verify_signature());
        }
    }

    /// Главное: изменение ЛЮБОГО поля (в том числе адресов, возможностей, типа и юрисдикции) ломает подпись.
    #[test]
    fn changing_any_single_field_breaks_the_signature() {
        let (h, _) = signed_hello(HelloType::Request);
        let mutations: Vec<(&str, Box<dyn Fn(&mut HelloPacket)>)> = vec![
            ("hello_type", Box::new(|p| p.hello_type = HelloType::Ack)),
            ("version", Box::new(|p| p.version ^= 1)),
            ("node_id", Box::new(|p| p.node_id.0[0] ^= 1)),
            ("x25519_public", Box::new(|p| p.x25519_public[0] ^= 1)),
            ("cid", Box::new(|p| p.cid[0] ^= 1)),
            ("capabilities", Box::new(|p| p.capabilities ^= 0x0001)),
            ("capabilities relay bit", Box::new(|p| p.capabilities |= hello_caps::RELAY)),
            ("timestamp", Box::new(|p| p.timestamp += 1)),
            ("nonce", Box::new(|p| p.nonce ^= 1)),
            ("wan_address", Box::new(|p| p.wan_address = Some("198.51.100.1:10000".into()))),
            ("wan_address removed", Box::new(|p| p.wan_address = None)),
            ("lan_address", Box::new(|p| p.lan_address = Some("10.0.0.1:10000".into()))),
            ("lan_address removed", Box::new(|p| p.lan_address = None)),
            ("discovery_endpoint", Box::new(|p| p.discovery_endpoint = Some("0.0.0.0:10001".into()))),
            ("discovery_endpoint removed", Box::new(|p| p.discovery_endpoint = None)),
            ("ipv6_virtual", Box::new(|p| p.ipv6_virtual = Some([0xfd; 16]))),
            ("ipv6_virtual removed", Box::new(|p| p.ipv6_virtual = None)),
            ("ipv6_external", Box::new(|p| p.ipv6_external = Some("2001:db8::2".into()))),
            ("ipv6_external removed", Box::new(|p| p.ipv6_external = None)),
            ("p2p_data_addr", Box::new(|p| p.p2p_data_addr = Some("0.0.0.0:9999".into()))),
            ("p2p_data_addr removed", Box::new(|p| p.p2p_data_addr = None)),
            ("p2p_x25519_public", Box::new(|p| p.p2p_x25519_public = Some([0x56; 32]))),
            ("p2p_x25519_public removed", Box::new(|p| p.p2p_x25519_public = None)),
            ("jurisdiction", Box::new(|p| p.jurisdiction = Some("RU".into()))),
            ("jurisdiction removed", Box::new(|p| p.jurisdiction = None)),
        ];
        for (name, m) in mutations {
            let mut p = h.clone();
            m(&mut p);
            assert!(!p.verify_signature(), "изменение поля «{name}» не сломало подпись");
        }
    }

    /// Значение поля нельзя «передвинуть» в соседнее: границы полей однозначны (длины строк записаны).
    #[test]
    fn moving_text_between_neighbouring_fields_is_detected() {
        let (mut h, id) = signed_hello(HelloType::Request);
        let a = h.challenge_data();
        // меняем местами wan и lan: подписанные данные должны отличаться
        std::mem::swap(&mut h.wan_address, &mut h.lan_address);
        assert_ne!(a, h.challenge_data());
        // склейка двух значений в одно поле не совпадает с исходными данными
        let mut g = h.clone();
        g.wan_address = Some("a".into());
        g.lan_address = Some("bc".into());
        let mut g2 = h.clone();
        g2.wan_address = Some("ab".into());
        g2.lan_address = Some("c".into());
        assert_ne!(g.challenge_data(), g2.challenge_data());
        let _ = id;
    }

    /// «Золотой» вектор: что именно подписывается, фиксируется. Меняете формат — повышайте метку «yandi-hello-v2» и обновляйте вектор осознанно.
    #[test]
    fn the_signed_bytes_are_frozen() {
        let mut h = HelloPacket::new_request(HashId([0x42; 32]), [0x11; 32], [0x22; 32], [0x33; 8], 0x0ABC);
        h.timestamp = 1_700_000_000;
        h.nonce = 0x0102030405060708;
        h.jurisdiction = None;
        h.wan_address = Some("203.0.113.9:10000".into());
        let bytes = h.challenge_data();
        assert!(bytes.starts_with(b"yandi-hello-v2\0"));
        use sha2::Digest;
        assert_eq!(hex::encode(sha2::Sha256::digest(&bytes)), GOLDEN_HELLO_HASH, "формат подписываемых данных изменился");
    }
    /// SHA-256 подписываемых байтов; получен независимой сборкой тех же байтов по описанию формата (Python).
    const GOLDEN_HELLO_HASH: &str = "7eeb4dc1bb872fd8c1d8349ec8682ffb7d3df0456391add13879428956ec8b7e";
}

#[cfg(test)]
mod jurisdiction_normalization_tests {
    use super::*;

    #[test]
    fn jurisdiction_is_cut_on_a_character_boundary_and_empty_means_none() {
        assert_eq!(normalize_jurisdiction(Some("UNITED-STATES".into())).as_deref(), Some("UNITED-S"));
        assert_eq!(normalize_jurisdiction(Some("RU".into())).as_deref(), Some("RU"));
        assert_eq!(normalize_jurisdiction(Some(String::new())), None);
        assert_eq!(normalize_jurisdiction(None), None);
        // многобайтовые буквы не режутся посередине
        let cyr = normalize_jurisdiction(Some("Россия".into())).unwrap();
        assert!(cyr.len() <= 8 && cyr.is_char_boundary(cyr.len()) && "Россия".starts_with(&cyr), "{cyr}");
    }
}
