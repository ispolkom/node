// src/netlayer/tun_device.rs
//!
//! YANDI Virtual Network Interface
//! ================================
//!
//! Creates dual virtual TUN devices for YANDI P2P network.
//!
//! This allows applications to use YANDI transparently without
//! SOCKS5/HTTP proxy configuration - just route traffic through
//! the virtual interface!
//!
//! Architecture:
//!   yandi_client (TUN) -> для локальных приложений
//!   yandi_p2p (TUN)     -> для YTP P2P трафика
//!   eth0/ens3 (REAL)    -> для реального интернета
//!
//! Addresses:
//!   yandi_client: 192.168.100.1/24 + fd00::1/64
//!   yandi_p2p: fc00:1234:5678:1::1/64

use std::io;
use anyhow::{Result, anyhow};
use tokio::sync::mpsc;
use tun::{Device, TunPacket};
use std::net::Ipv6Addr;
use std::sync::Arc;
use crate::protocol::{Station, Wagon, TrainId};
use crate::util::{HashId, mask_hash_id};
use crate::netlayer::P2PTransport;
use crate::netlayer::tun_exit::{TunWagon, TunWagonResponse};

/// IPv6 protocol numbers
#[allow(dead_code)]
enum IpProtocol {
    ICMPv6 = 58,
    TCP = 6,
    UDP = 17,
}

/// Parsed IPv6 packet information
pub struct IPv6PacketInfo {
    pub source: Ipv6Addr,
    pub destination: Ipv6Addr,
    pub protocol: u8,
    pub payload: Vec<u8>,
}

/// Parse IPv6 packet header
///
/// IPv6 header format (RFC 8200):
/// - Version (4 bits) + Traffic Class (8 bits) + Flow Label (20 bits) = 4 bytes
/// - Payload Length (2 bytes)
/// - Next Header (1 byte) - this is the protocol
/// - Hop Limit (1 byte)
/// - Source Address (16 bytes)
/// - Destination Address (16 bytes)
/// Total: 40 bytes
fn parse_ipv6_packet(packet: &[u8]) -> Result<IPv6PacketInfo> {
    if packet.len() < 40 {
        return Err(anyhow!("Packet too short: {} bytes", packet.len()));
    }

    // Version (upper 4 bits of first byte)
    let version = (packet[0] & 0xF0) >> 4;
    if version != 6 {
        return Err(anyhow!("Not IPv6: version {}", version));
    }

    // Payload length (bytes 4-5)
    let payload_length = u16::from_be_bytes([packet[4], packet[5]]) as usize;

    // Next header / protocol (byte 6)
    let protocol = packet[6];

    // Source address (bytes 8-23)
    let source_bytes: [u8; 16] = packet[8..24].try_into()
        .map_err(|_| anyhow!("Invalid source address"))?;
    let source = Ipv6Addr::from(source_bytes);

    // Destination address (bytes 24-39)
    let dest_bytes: [u8; 16] = packet[24..40].try_into()
        .map_err(|_| anyhow!("Invalid destination address"))?;
    let destination = Ipv6Addr::from(dest_bytes);

    // Payload (everything after the 40-byte header)
    let payload_start = 40;
    let payload_end = std::cmp::min(payload_start + payload_length, packet.len());
    let payload = packet[payload_start..payload_end].to_vec();

    Ok(IPv6PacketInfo {
        source,
        destination,
        protocol,
        payload,
    })
}

/// YANDI virtual network interface
pub struct YandiTunDevice {
    /// TUN device name (e.g., "yandi0")
    pub device_name: String,
    /// Virtual IPv6 address
    pub ipv6_addr: String,
    /// Channel for sending packets to YTP
    tx: mpsc::Sender<Vec<u8>>,
    /// Channel for receiving packets from YTP
    rx: mpsc::Receiver<Vec<u8>>,
    /// Actual TUN device (created when started)
    device: Option<tun::AsyncDevice>,
    /// Station for sending YTP wagons (optional, set when available)
    station: Option<Arc<Station>>,
    /// P2P Transport for peer lookup (optional, set when available)
    transport: Option<Arc<P2PTransport>>,
    /// Channel for sending TunWagon to exit node (optional, set when available)
    tun_wagon_tx: Option<mpsc::Sender<(HashId, TunWagon)>>,
    /// Exit node peer ID (where to send TunWagons)
    exit_peer_id: Option<HashId>,
}

impl YandiTunDevice {
    /// Create new YANDI TUN device
    ///
    /// # Arguments
    /// * `name` - Device name (e.g., "yandi0")
    /// * `ipv6` - Virtual IPv6 address (e.g., "fc00:1234:5678:1::1/64")
    pub fn new(name: &str, ipv6: &str) -> Result<Self> {
        println!("🌐 Creating YANDI virtual interface: {} ({})", name, ipv6);

        let (tx, rx) = mpsc::channel(1000);

        Ok(Self {
            device_name: name.to_string(),
            ipv6_addr: ipv6.to_string(),
            tx,
            rx,
            device: None,
            station: None,
            transport: None,
            tun_wagon_tx: None,
            exit_peer_id: None,
        })
    }

    /// Set station reference for sending YTP wagons
    pub fn set_station(&mut self, station: Arc<Station>) {
        self.station = Some(station);
        println!("🔗 Station linked to TUN device: {}", self.device_name);
    }

    /// Set P2P transport reference for peer lookup
    pub fn set_transport(&mut self, transport: Arc<P2PTransport>) {
        self.transport = Some(transport);
        println!("🔗 P2P Transport linked to TUN device: {}", self.device_name);
    }

    /// Set TUN wagon channel for sending packets to exit node
    pub fn set_tun_wagon_channel(&mut self, tx: mpsc::Sender<(HashId, TunWagon)>) {
        self.tun_wagon_tx = Some(tx);
        println!("🔗 TUN Wagon channel linked to TUN device: {}", self.device_name);
    }

    /// Set exit node peer ID (where to send TunWagons for internet access)
    pub fn set_exit_node(&mut self, peer_id: HashId) {
        self.exit_peer_id = Some(peer_id);
        println!("🌍 Exit node set: {}", mask_hash_id(&peer_id));
    }

    /// Start the TUN device (packet processing loop)
    pub async fn start(&mut self) -> Result<()> {
        println!("🚀 Starting YANDI TUN device: {}", self.device_name);

        // 1. Configure TUN device
        let mut config = tun::Configuration::default();

        // Set device name
        config.name(&self.device_name);

        // NOTE: packet_information добавляет 4-byte header перед каждым пакетом
        // Это ломает IPv6 парсинг, поэтому отключаем
        // config.platform(|config| {
        //     config.packet_information(true);
        // });

        println!("📦 Creating TUN device: {} (requires CAP_NET_ADMIN)", self.device_name);

        // 2. Create the TUN device
        let device_result = tun::create_as_async(&config);

        let mut device = match device_result {
            Ok(dev) => {
                println!("✅ TUN device created: {}", self.device_name);
                dev
            }
            Err(e) => {
                return Err(anyhow!("Failed to create TUN device '{}': {}", self.device_name, e));
            }
        };

        // Store the device
        self.device = Some(device);

        // 3. Setup routing (ip addr add, ip link set up, etc.)
        self.setup_routing()?;

        // 4. Prepare channels for background task
        let device_name = self.device_name.clone();
        let tun_wagon_tx = self.tun_wagon_tx.clone();
        let exit_peer_id = self.exit_peer_id.clone();

        // Take device for background task
        let mut tun_device = self.device.take().unwrap();

        tokio::spawn(async move {
            println!("📥 [{}] Starting packet reader in background", device_name);

            let mut packet_count = 0u64;
            let mut buffer = vec![0u8; 1500];

            use tokio::io::AsyncReadExt;
            use tokio::time::{sleep, Duration};

            loop {
                // Read packet from TUN device
                let n = match tun_device.read(&mut buffer).await {
                    Ok(n) => n,
                    Err(e) => {
                        eprintln!("   ❌ [{}] Error reading from TUN: {}", device_name, e);
                        sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };

                if n == 0 {
                    sleep(Duration::from_millis(10)).await;
                    continue;
                }

                packet_count += 1;

                println!("📦 [{}] Packet #{}: {} bytes", device_name, packet_count, n);

                // Parse IPv6 packet
                let packet_data = buffer[..n].to_vec();

                match parse_ipv6_packet(&packet_data) {
                    Ok(pkt_info) => {
                        println!("   📍 {} -> {} (proto: {})",
                            pkt_info.source, pkt_info.destination, pkt_info.protocol);

                        // Check if exit node is configured
                        let exit_id = match &exit_peer_id {
                            Some(id) => id,
                            None => {
                                println!("   ⚠️  No exit node configured - packet dropped");
                                continue;
                            }
                        };

                        // Check if tun_wagon_tx channel is available
                        let tx = match &tun_wagon_tx {
                            Some(tx) => tx,
                            None => {
                                println!("   ⚠️  No TunWagon channel - packet dropped");
                                continue;
                            }
                        };

                        // Create TunWagon with full IPv6 packet
                        let connection_id = pkt_info.destination.to_string();
                        let wagon = TunWagon {
                            connection_id: connection_id.clone(),
                            packet: packet_data.clone(),
                            close: false,
                        };

                        // Send to exit node
                        if let Err(e) = tx.send((exit_id.clone(), wagon)).await {
                            eprintln!("   ❌ Failed to send TunWagon: {}", e);
                        } else {
                            println!("   ✅ TunWagon sent -> {} ({} bytes)",
                                hex::encode(&exit_id.0[..4]), n);
                        }
                    }
                    Err(e) => {
                        println!("   ⚠️  Failed to parse IPv6: {}", e);
                    }
                }
            }
        });

        println!("✅ TUN device {} running in background", self.device_name);

        Ok(())
    }

    /// Read packets from TUN device and forward to YTP
    async fn read_packets(&mut self) -> Result<()> {
        use tokio::time::{sleep, Duration};

        println!("📥 Reading packets from TUN device...");

        loop {
            // Simulate receiving packets
            sleep(Duration::from_secs(1)).await;

            // TODO: Real implementation:
            // let mut buf = [0u8; 1500];
            // let n = tun.read(&mut buf).await?;
            // self.handle_ipv6_packet(&buf[..n])?;
        }
    }

    /// Read REAL packets from TUN device
    async fn read_packets_real(&mut self) -> Result<()> {
        println!("📥 Reading REAL packets from TUN device...");

        if self.device.is_none() {
            return Err(anyhow!("TUN device not created"));
        }

        let mut packet_count = 0u64;
        let mut buffer = vec![0u8; 1500]; // MTU 1500

        use tokio::io::AsyncReadExt;
        use tokio::time::{sleep, Duration};

        loop {
            // Read packet from TUN device
            let n = {
                let device = self.device.as_mut().unwrap();
                match device.read(&mut buffer).await {
                    Ok(n) => n,
                    Err(e) => {
                        eprintln!("   ❌ Error reading from TUN: {}", e);
                        // Continue reading instead of breaking
                        sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                }
            };

            if n == 0 {
                // No data, continue
                sleep(Duration::from_millis(10)).await;
                continue;
            }

            packet_count += 1;

            println!("📦 [Packet #{}] Received {} bytes from TUN", packet_count, n);

            // Parse IPv6 packet
            match parse_ipv6_packet(&buffer[..n]) {
                Ok(packet_info) => {
                    println!("   📍 {} -> {} (proto: {})",
                        packet_info.source, packet_info.destination, packet_info.protocol);

                    // Show protocol type
                    match packet_info.protocol {
                        6 => println!("   🔵 TCP packet"),
                        17 => println!("   🟢 UDP packet"),
                        58 => println!("   🔴 ICMPv6 packet"),
                        _ => println!("   ⚪ Unknown protocol: {}", packet_info.protocol),
                    }

                    // Encapsulate in YTP and send to peer (async)
                    let self_clone = YandiTunDevice {
                        device_name: self.device_name.clone(),
                        ipv6_addr: self.ipv6_addr.clone(),
                        tx: self.tx.clone(),
                        rx: mpsc::channel(1).1, // Create new dummy receiver
                        device: None, // Don't clone the device
                        station: self.station.clone(),
                        transport: self.transport.clone(),
                        tun_wagon_tx: self.tun_wagon_tx.clone(),
                        exit_peer_id: self.exit_peer_id.clone(),
                    };

                    tokio::spawn(async move {
                        if let Err(e) = self_clone.handle_ipv6_packet_real(packet_info).await {
                            eprintln!("   ❌ Error handling packet: {}", e);
                        }
                    });
                }
                Err(e) => {
                    eprintln!("   ⚠️  Failed to parse IPv6: {} (packet may not be IPv6)", e);
                }
            }
        }
    }

    /// Handle incoming IPv6 packet from TUN (REAL implementation)
    async fn handle_ipv6_packet_real(&self, packet_info: IPv6PacketInfo) -> Result<()> {
        println!("🔧 [TUN] Processing IPv6 packet...");

        // Check if destination is in YANDI P2P network
        let dest = packet_info.destination;
        let segments = dest.segments();

        let is_yandi = segments[0] == 0xfc00 && segments[1] == 0x1234 && segments[2] == 0x5678;
        let is_client = (segments[0] & 0xff00) == 0xfd00;

        if is_yandi {
            println!("   ✅ Destination is YANDI P2P network");

            // Find peer by IPv6 address
            if let Some(ref transport) = self.transport {
                match Self::find_peer_by_ipv6(transport, dest) {
                    Some(peer_id) => {
                        println!("   🎯 Found peer: {}", hex::encode(&peer_id.0[..8]));

                        // Check if we have a station reference
                        if let Some(ref station) = self.station {
                            println!("   📦 Encapsulating packet in YTP wagon...");

                            println!("   🚂 Sending packet to peer: {}", hex::encode(&peer_id.0[..8]));
                            println!("   📦 Payload size: {} bytes", packet_info.payload.len());

                            // Send via station (station will create wagons)
                            match station.send_train(peer_id, packet_info.payload.clone()).await {
                                Ok(train_id) => {
                                    println!("   ✅ Packet sent successfully via P2P!");
                                    println!("   🚂 Train ID: {}", train_id);
                                }
                                Err(e) => {
                                    eprintln!("   ❌ Failed to send packet: {}", e);
                                }
                            }
                        } else {
                            println!("   ⚠️  No station reference - cannot send packet");
                            println!("   💡 Set station with set_station() method");
                        }
                    }
                    None => {
                        println!("   ⚠️  Peer not found for IPv6: {}", dest);
                        println!("   💡 Peer may not be connected yet");
                    }
                }
            } else {
                println!("   ⚠️  No transport reference - cannot lookup peer");
                println!("   💡 Set transport with set_transport() method");
            }

        } else if is_client {
            println!("   📱 Destination is client network (local)");
            println!("   ⏭️  Forwarding locally...");

            // TODO: Route to other client devices

        } else {
            println!("   🌍 Destination is external network");
            println!("   📤 Routing via TUN exit node...");

            // Check if we have exit node configured
            if let (Some(ref exit_peer_id), Some(ref tun_wagon_tx)) = (&self.exit_peer_id, &self.tun_wagon_tx) {
                println!("   🎯 Exit node: {}", mask_hash_id(exit_peer_id));

                // Reconstruct full IPv6 packet (header + payload)
                let mut ipv6_packet = Vec::with_capacity(40 + packet_info.payload.len());

                // IPv6 Header (40 bytes)
                // Version (4 bits) + Traffic Class (8 bits) + Flow Label (20 bits)
                ipv6_packet.push(0x60); // Version 6
                ipv6_packet.push(0x00); // Traffic Class + Flow Label (part 1)
                ipv6_packet.push(0x00); // Flow Label (part 2)
                ipv6_packet.push(0x00); // Flow Label (part 3)

                // Payload Length (2 bytes)
                let payload_len = packet_info.payload.len() as u16;
                ipv6_packet.extend_from_slice(&payload_len.to_be_bytes());

                // Next Header (1 byte) - protocol
                ipv6_packet.push(packet_info.protocol);

                // Hop Limit (1 byte)
                ipv6_packet.push(64); // Default hop limit

                // Source Address (16 bytes)
                ipv6_packet.extend_from_slice(&packet_info.source.octets());

                // Destination Address (16 bytes)
                ipv6_packet.extend_from_slice(&packet_info.destination.octets());

                // Payload
                ipv6_packet.extend_from_slice(&packet_info.payload);

                // Create TunWagon
                let connection_id = format!("{}:{}->{}",
                    packet_info.source,
                    packet_info.destination,
                    packet_info.protocol);

                let wagon = TunWagon {
                    connection_id: connection_id.clone(),
                    packet: ipv6_packet.clone(),
                    close: false,
                };

                println!("   📦 Creating TunWagon:");
                println!("      Connection ID: {}", connection_id);
                println!("      IPv6 packet size: {} bytes", ipv6_packet.len());
                println!("      Source: {}", packet_info.source);
                println!("      Destination: {}", packet_info.destination);

                // Send to exit node via channel
                if let Err(e) = tun_wagon_tx.send((exit_peer_id.clone(), wagon)).await {
                    eprintln!("   ❌ Failed to send TunWagon: {}", e);
                } else {
                    println!("   ✅ TunWagon sent to exit node!");
                }

            } else {
                if self.exit_peer_id.is_none() {
                    println!("   ⚠️  No exit node configured!");
                    println!("   💡 Use set_exit_node() to configure exit node");
                }
                if self.tun_wagon_tx.is_none() {
                    println!("   ⚠️  No TUN wagon channel configured!");
                    println!("   💡 Use set_tun_wagon_channel() to configure channel");
                }
            }
        }

        Ok(())
    }

    /// Find peer ID by virtual IPv6 address
    fn find_peer_by_ipv6(transport: &P2PTransport, ipv6: Ipv6Addr) -> Option<HashId> {
        use std::net::Ipv6Addr;

        // Get peers from transport (this needs to be async, but we're in sync context)
        // For now, we'll need a different approach
        // This is a limitation we'll solve later

        println!("   🔍 Looking up peer for IPv6: {}", ipv6);
        println!("   ⚠️  NOTE: Real lookup needs async access to peers table");

        // TODO: Implement actual peer lookup
        // This requires:
        // 1. Transport to expose get_peers() method
        // 2. Or make this method async
        // 3. Or cache peer lookups in TUN device

        None // Placeholder
    }

    /// Handle incoming IPv6 packet from TUN (OLD - deprecated)
    fn handle_ipv6_packet(&self, packet: &[u8]) -> Result<()> {
        // Parse IPv6 header (simplified)
        if packet.len() < 40 {
            return Ok(()); // Too short
        }

        // Check version (should be 6 for IPv6)
        let version = (packet[0] & 0xF0) >> 4;
        if version != 6 {
            return Ok(()); // Not IPv6
        }

        println!("📦 Received IPv6 packet: {} bytes", packet.len());

        // Parse the packet
        match parse_ipv6_packet(packet) {
            Ok(info) => {
                println!("   📍 {} -> {} (protocol: {})",
                    info.source, info.destination, info.protocol);

                // Route based on protocol
                match info.protocol {
                    6 => println!("   🔵 TCP packet"),
                    17 => println!("   🟢 UDP packet"),
                    58 => println!("   🔴 ICMPv6 packet"),
                    _ => println!("   ⚪ Unknown protocol: {}", info.protocol),
                }

                // TODO: Route to P2P network or handle locally
            }
            Err(e) => {
                eprintln!("   ❌ Failed to parse IPv6 packet: {}", e);
            }
        }

        Ok(())
    }

    /// Write packet to TUN device (from YTP)
    pub async fn write_packet(&self, packet: Vec<u8>) -> Result<()> {
        println!("📤 Writing packet to TUN: {} bytes", packet.len());

        // TODO: Write to TUN device

        Ok(())
    }

    /// Setup system routing for YANDI network
    pub fn setup_routing(&self) -> Result<()> {
        println!("🛣️  Setting up routing for YANDI network...");

        use std::process::Command;

        // Execute system commands to configure the TUN device
        let commands = vec![
            // Set IPv6 address
            format!("ip addr add {} dev {}", self.ipv6_addr, self.device_name),
            // Bring interface up
            format!("ip link set {} up", self.device_name),
            // Set MTU (Maximum Transmission Unit)
            format!("ip link set mtu 1400 dev {}", self.device_name),
        ];

        for cmd in &commands {
            println!("   ⚙️  Executing: {}", cmd);
            let parts: Vec<&str> = cmd.split_whitespace().collect();
            let result = Command::new(parts[0])
                .args(&parts[1..])
                .output();

            match result {
                Ok(output) => {
                    if output.status.success() {
                        println!("   ✅ Success");
                    } else {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        eprintln!("   ⚠️  Warning: {}", stderr);
                    }
                }
                Err(e) => {
                    return Err(anyhow!("Failed to execute '{}': {}", cmd, e));
                }
            }
        }

        println!("✅ Routing configured successfully");
        Ok(())
    }

    /// Cleanup routing and remove TUN device
    pub fn cleanup(&self) -> Result<()> {
        println!("🧹 Cleaning up YANDI TUN device...");

        use std::process::Command;

        // Bring interface down
        let cmd = format!("ip link set {} down", self.device_name);
        println!("   ⚙️  Executing: {}", cmd);
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if let Ok(output) = Command::new(parts[0]).args(&parts[1..]).output() {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!("   ⚠️  Warning bringing interface down: {}", stderr);
            }
        }

        println!("✅ Cleanup completed");
        Ok(())
    }
}

/// Manager for dual TUN devices (client + p2p)
pub struct YandiTunManager {
    /// Client-facing TUN device (for local apps)
    client_device: Option<YandiTunDevice>,
    /// P2P-facing TUN device (for YTP traffic)
    p2p_device: Option<YandiTunDevice>,
    /// Channel for receiving TunWagonResponse from exit node
    tun_wagon_resp_rx: Option<mpsc::Receiver<(HashId, TunWagonResponse)>>,
}

impl YandiTunManager {
    /// Create new TUN device manager
    pub fn new() -> Result<Self> {
        println!("🌐 Initializing YANDI TUN manager...");

        Ok(Self {
            client_device: None,
            p2p_device: None,
            tun_wagon_resp_rx: None,
        })
    }

    /// Create client TUN device
    pub fn create_client_device(&mut self) -> Result<()> {
        println!("📱 Creating yandi_client TUN device...");

        // Create client device with dual-stack addresses
        let client = YandiTunDevice::new(
            "yandi_client",
            "fd00::1/64"  // ULA address for client
        )?;

        self.client_device = Some(client);
        println!("✅ yandi_client TUN device initialized");
        Ok(())
    }

    /// Create P2P TUN device
    pub fn create_p2p_device(&mut self) -> Result<()> {
        println!("🔗 Creating yandi_p2p TUN device...");

        // Create P2P device with YANDI virtual address
        let p2p = YandiTunDevice::new(
            "yandi_p2p",
            "fc00:1234:5678:1::1/64"  // YANDI P2P address
        )?;

        self.p2p_device = Some(p2p);
        println!("✅ yandi_p2p TUN device initialized");
        Ok(())
    }

    /// Start both TUN devices
    pub async fn start_all(&mut self) -> Result<()> {
        println!("🚀 Starting all YANDI TUN devices...");

        // Start client device
        if let Some(ref mut device) = self.client_device {
            device.start().await?;
            println!("✅ yandi_client started");
        }

        // Start P2P device
        if let Some(ref mut device) = self.p2p_device {
            device.start().await?;
            println!("✅ yandi_p2p started");
        }

        println!("✅ All TUN devices running");
        Ok(())
    }

    /// Cleanup all devices
    pub fn cleanup_all(&self) -> Result<()> {
        println!("🧹 Cleaning up all YANDI TUN devices...");

        if let Some(ref device) = self.client_device {
            device.cleanup()?;
        }

        if let Some(ref device) = self.p2p_device {
            device.cleanup()?;
        }

        println!("✅ All TUN devices cleaned up");
        Ok(())
    }

    /// Get client device reference
    pub fn client(&self) -> Option<&YandiTunDevice> {
        self.client_device.as_ref()
    }

    /// Get P2P device reference
    pub fn p2p(&self) -> Option<&YandiTunDevice> {
        self.p2p_device.as_ref()
    }

    /// Get mutable client device reference
    pub fn client_mut(&mut self) -> Option<&mut YandiTunDevice> {
        self.client_device.as_mut()
    }

    /// Get mutable P2P device reference
    pub fn p2p_mut(&mut self) -> Option<&mut YandiTunDevice> {
        self.p2p_device.as_mut()
    }

    /// Route packet from client to P2P interface
    pub fn route_client_to_p2p(&self, packet_info: &IPv6PacketInfo) -> Result<()> {
        println!("🔀 Route: client -> P2P");
        println!("   📦 {} -> {}", packet_info.source, packet_info.destination);

        // TODO: Implement NAT64 translation if needed
        // TODO: Encapsulate in YTP wagon
        // TODO: Send to appropriate peer

        Ok(())
    }

    /// Route packet from P2P to client interface
    pub fn route_p2p_to_client(&self, packet_info: &IPv6PacketInfo) -> Result<()> {
        println!("🔀 Route: P2P -> client");
        println!("   📦 {} -> {}", packet_info.source, packet_info.destination);

        // TODO: Decapsulate from YTP wagon
        // TODO: NAT64 reverse translation if needed
        // TODO: Write to client TUN device

        Ok(())
    }

    /// Check if destination is in YANDI P2P network
    pub fn is_yandi_network(dest: &Ipv6Addr) -> bool {
        // Check if address is in fc00:1234:5678::/32 range
        let segments = dest.segments();
        segments[0] == 0xfc00 && segments[1] == 0x1234 && segments[2] == 0x5678
    }

    /// Set TUN wagon response channel (for receiving data from exit node)
    pub fn set_tun_wagon_response_channel(&mut self, rx: mpsc::Receiver<(HashId, TunWagonResponse)>) {
        println!("📡 [TUN ENTRY] TunWagonResponse channel connected");
        self.tun_wagon_resp_rx = Some(rx);
    }

    /// Start processing TunWagonResponse messages from exit node
    pub async fn start_tun_wagon_response_handler(&mut self) -> Result<()> {
        if self.tun_wagon_resp_rx.is_none() {
            return Err(anyhow!("TunWagonResponse channel not set! Call set_tun_wagon_response_channel() first"));
        }

        if self.client_device.is_none() {
            return Err(anyhow!("Client TUN device not created! Call create_client_device() first"));
        }

        let mut rx = self.tun_wagon_resp_rx.take().unwrap();
        let device_name = self.client_device.as_ref().unwrap().device_name.clone();

        // Take the device out of client_device (we'll manage it in the spawned task)
        let tun_device = self.client_device.as_mut()
            .and_then(|d| d.device.take());

        if tun_device.is_none() {
            return Err(anyhow!("TUN device is not started! Call start_all() first"));
        }

        let mut tun_device = tun_device.unwrap();

        println!("🚀 [TUN ENTRY] Starting TunWagonResponse handler...");
        println!("   💡 Writing received data to {} TUN device", device_name);

        tokio::spawn(async move {
            println!("📨 [TUN ENTRY] TunWagonResponse listener started");

            use tokio::io::AsyncWriteExt;

            while let Some((peer_id, response)) = rx.recv().await {
                println!();
                println!("╔════════════════════════════════════════════════════════════╗");
                println!("║  📨 TUN ENTRY: Получен ответ от exit node                  ║");
                println!("║     от ноды: {}                              ║",
                         mask_hash_id(&peer_id));
                println!("╚════════════════════════════════════════════════════════════╝");
                println!();

                println!("   📦 Connection ID: {}", response.connection_id);
                println!("   📦 Data size: {} bytes", response.data.len());
                println!("   📦 Close signal: {}", response.close);

                if response.close {
                    println!("   🔒 Закрытие соединения...");
                    continue;
                }

                // Парсим IPv6 пакет
                match parse_ipv6_packet(&response.data) {
                    Ok(packet_info) => {
                        println!("   ✅ IPv6: {} -> {}", packet_info.source, packet_info.destination);

                        // Записываем пакет в TUN устройство
                        println!("   💡 Записываем {} bytes в TUN устройство {}", response.data.len(), device_name);

                        match tun_device.write(&response.data).await {
                            Ok(n) => {
                                println!("   ✅ Записано {} bytes в TUN устройство", n);
                            }
                            Err(e) => {
                                eprintln!("   ❌ Ошибка записи в TUN устройство: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        println!("   ❌ Ошибка парсинга IPv6: {}", e);
                        println!("   📦 Raw data (first 64 bytes):");
                        println!("      {}", hex::encode(&response.data[..response.data.len().min(64)]));
                    }
                }

                println!("   ✅ Обработка TunWagonResponse завершена");
                println!();
            }

            println!("📨 [TUN ENTRY] TunWagonResponse listener stopped");
        });

        println!("✅ [TUN ENTRY] TunWagonResponse handler запущен в фоне");
        Ok(())
    }

    /// Check if destination is local client network
    pub fn is_client_network(dest: &Ipv6Addr) -> bool {
        // Check if address is in fd00::/8 range (ULA)
        let segments = dest.segments();
        (segments[0] & 0xff00) == 0xfd00
    }
}

impl Default for YandiTunManager {
    fn default() -> Self {
        Self::new().expect("Failed to create TUN manager")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tun_creation() {
        let tun = YandiTunDevice::new("yandi0", "fc00:1234:5678:1::1/64");
        assert!(tun.is_ok());
    }
}
