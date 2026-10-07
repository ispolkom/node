// src/util/external_ipv6.rs
//!
//! External IPv6 Detection
//! =======================
//!
//! 1. From local interfaces (primary method - no bans!)
//! 2. Via STUN protocol (for P2P traffic between nodes)

use std::net::{UdpSocket, Ipv6Addr, SocketAddrV6, ToSocketAddrs};
use anyhow::{Result, anyhow};
use std::fs;

/// Discover external IPv6 address from local network interfaces
/// This is the PRIMARY method - no external servers, no bans!
pub fn discover_external_ipv6() -> Result<Option<String>> {
    println!("🌍 Discovering external IPv6 from network interfaces...");

    match get_ipv6_from_interfaces() {
        Ok(Some(ip)) => {
            let masked = crate::util::mask_ipv6(&ip);
            println!("   ✅ External IPv6: {} (from network interface)", masked);
            Ok(Some(ip))
        }
        Ok(None) => {
            eprintln!("   ⚠️  No global IPv6 addresses found on interfaces");
            Ok(None)
        }
        Err(e) => {
            eprintln!("   ❌ Failed to read IPv6 interfaces: {}", e);
            Ok(None)
        }
    }
}

/// Get global IPv6 addresses from network interfaces
fn get_ipv6_from_interfaces() -> Result<Option<String>> {
    #[cfg(target_os = "linux")]
    {
        // Read from /proc/net/if_inet6
        let content = fs::read_to_string("/proc/net/if_inet6")
            .map_err(|e| anyhow!("Failed to read /proc/net/if_inet6: {}", e))?;

        // Parse each line looking for global addresses (2000::/3)
        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 6 {
                let addr_hex = parts[0];
                let scope_id = parts[2]; // Scope ID (0 = global)

                // Skip non-global addresses
                if scope_id != "00" {
                    continue; // Skip link-local (fe80::/10) and others
                }

                // Parse hexadecimal IPv6 address
                if let Ok(addr) = parse_proc_ipv6(addr_hex) {
                    // Check if it's a global unicast address (2000::/3)
                    if is_global_ipv6(&addr) {
                        // Verify connectivity with ping6
                        if test_ipv6_connectivity(&addr).unwrap_or(false) {
                            return Ok(Some(addr.to_string()));
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    #[cfg(target_os = "macos")]
    {
        // Use `ifconfig` command
        use std::process::Command;

        let output = Command::new("ifconfig")
            .arg("-a")
            .output()
            .map_err(|e| anyhow!("Failed to run ifconfig: {}", e))?;

        let content = String::from_utf8_lossy(&output.stdout);

        // Parse ifconfig output for inet6 addresses
        for line in content.lines() {
            if line.contains("inet6 ") && line.contains("global") {
                // Extract IPv6 address (format: "inet6 2001:db8::1 prefixlen 64")
                if let Some(addr_str) = line.split_whitespace().nth(1) {
                    if let Ok(addr) = addr_str.parse::<Ipv6Addr>() {
                        if is_global_ipv6(&addr) {
                            if test_ipv6_connectivity(&addr).unwrap_or(false) {
                                return Ok(Some(addr.to_string()));
                            }
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    #[cfg(target_os = "windows")]
    {
        // Use `netsh` command
        use std::process::Command;

        let output = Command::new("netsh")
            .args(&["interface", "ipv6", "show", "address"])
            .output()
            .map_err(|e| anyhow!("Failed to run netsh: {}", e))?;

        let content = String::from_utf8_lossy(&output.stdout);

        // Parse netsh output
        for line in content.lines() {
            if line.contains("2001:") || line.contains("2a01:") || line.contains("2a02:") {
                // Extract IPv6 address from line
                if let Some(addr_str) = line.split_whitespace().next() {
                    if let Ok(addr) = addr_str.parse::<Ipv6Addr>() {
                        if is_global_ipv6(&addr) {
                            if test_ipv6_connectivity(&addr).unwrap_or(false) {
                                return Ok(Some(addr.to_string()));
                            }
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        // Unsupported platform
        Ok(None)
    }
}

/// Parse IPv6 address from /proc/net/if_inet6 format
fn parse_proc_ipv6(hex_addr: &str) -> Result<Ipv6Addr> {
    // /proc/net/if_inet6 stores addresses in reverse nibble order
    // For example: 00000000000000000000000000000001 -> ::1
    let mut bytes = [0u8; 16];

    for (i, chunk) in (0..32).step_by(4).enumerate() {
        let nibble = &hex_addr[chunk..chunk + 4];
        let byte = u8::from_str_radix(nibble, 16)
            .map_err(|e| anyhow!("Failed to parse IPv6 byte: {}", e))?;

        // Reverse byte order for each pair
        let octet_index = if i % 2 == 0 { i + 1 } else { i - 1 };
        bytes[octet_index] = byte;
    }

    Ok(Ipv6Addr::from(bytes))
}

/// Check if IPv6 address is global unicast (2000::/3)
fn is_global_ipv6(addr: &Ipv6Addr) -> bool {
    let segments = addr.segments();
    // Global unicast addresses start with 001 (binary) = 2000::/3
    (segments[0] & 0xe000) == 0x2000
}

/// Test IPv6 connectivity with ping6
fn test_ipv6_connectivity(addr: &Ipv6Addr) -> Result<bool> {
    use std::process::Command;

    #[cfg(target_os = "linux")]
    {
        let output = Command::new("ping6")
            .args(&["-c", "1", "-W", "2", &addr.to_string()])
            .output()
            .map_err(|e| anyhow!("Failed to run ping6: {}", e))?;

        Ok(output.status.success())
    }

    #[cfg(target_os = "macos")]
    {
        let output = Command::new("ping6")
            .args(&["-c", "1", "-t", "2", &addr.to_string()])
            .output()
            .map_err(|e| anyhow!("Failed to run ping6: {}", e))?;

        Ok(output.status.success())
    }

    #[cfg(target_os = "windows")]
    {
        let output = Command::new("ping")
            .args(&["-n", "1", "-w", "2000", &addr.to_string()])
            .output()
            .map_err(|e| anyhow!("Failed to run ping: {}", e))?;

        Ok(output.status.success())
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        // Assume connectivity on unknown platforms
        Ok(true)
    }
}

/// STUN servers for P2P traffic between nodes (NOT for discovery!)
const STUN_SERVERS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun.cloudflare.com:3478",
];

/// Perform STUN query to peer node for NAT traversal
/// This is used for P2P traffic between nodes, NOT for initial discovery!
pub fn stun_query_peer(peer_addr: &str) -> Result<Option<String>> {
    println!("🔄 STUN query to peer: {}...", peer_addr);

    match query_stun_server(peer_addr) {
        Ok(Some(ip)) => {
            let masked = crate::util::mask_ipv6(&ip);
            println!("   ✅ STUN response: {}", masked);
            Ok(Some(ip))
        }
        Ok(None) => {
            eprintln!("   ⚠️  No STUN response from peer");
            Ok(None)
        }
        Err(e) => {
            eprintln!("   ❌ STUN query failed: {}", e);
            Err(e)
        }
    }
}

/// Query a single STUN server
fn query_stun_server(server_addr: &str) -> Result<Option<String>> {
    // Parse server address (already includes port, e.g., "stun.l.google.com:19302")
    let addrs: Vec<_> = server_addr
        .to_socket_addrs()
        .map_err(|e| anyhow!("Failed to resolve {}: {}", server_addr, e))?
        .filter(|addr| matches!(addr, std::net::SocketAddr::V6(_)))
        .collect();

    if addrs.is_empty() {
        return Ok(None);
    }

    let server_ipv6 = match addrs.first() {
        Some(std::net::SocketAddr::V6(addr)) => *addr,
        _ => return Ok(None),
    };

    // Create UDP socket bound to IPv6
    let socket = UdpSocket::bind("[::]:0")
        .map_err(|e| anyhow!("Failed to bind IPv6 UDP socket: {}", e))?;

    socket.set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .map_err(|e| anyhow!("Failed to set socket timeout: {}", e))?;

    // Send STUN Binding Request
    // STUN magic cookie + message type (Binding Request = 0x0001)
    let stun_request = [
        0x00, 0x01,              // Message Type: Binding Request
        0x00, 0x00,              // Message Length: 0
        0x21, 0x12, 0xa4, 0x42,  // Magic Cookie: 0x2112A442
        // Transaction ID (96 bits)
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
        0x08, 0x09, 0x0a, 0x0b,
    ];

    socket.send_to(&stun_request, server_ipv6)
        .map_err(|e| anyhow!("Failed to send STUN request: {}", e))?;

    // Receive STUN response
    let mut buf = [0u8; 512];
    let (len, from) = socket.recv_from(&mut buf)
        .map_err(|e| anyhow!("Failed to receive STUN response: {}", e))?;

    // Verify response is from the STUN server
    if from != std::net::SocketAddr::V6(server_ipv6) {
        return Ok(None);
    }

    // Parse STUN response to extract XOR-MAPPED-ADDRESS
    if let Some(ip) = parse_stun_response(&buf[..len]) {
        return Ok(Some(ip));
    }

    Ok(None)
}

/// Parse STUN Binding Response to extract XOR-MAPPED-ADDRESS
fn parse_stun_response(response: &[u8]) -> Option<String> {
    if response.len() < 20 {
        return None;
    }

    // Verify magic cookie
    if response[4..8] != [0x21, 0x12, 0xa4, 0x42] {
        return None;
    }

    // Check message type (Binding Response = 0x0101)
    let msg_type = u16::from_be_bytes([response[0], response[1]]);
    if msg_type != 0x0101 {
        return None;
    }

    // Parse attributes
    let mut pos = 20; // Skip STUN header (20 bytes)
    while pos + 4 <= response.len() {
        let attr_type = u16::from_be_bytes([response[pos], response[pos + 1]]);
        let attr_len = u16::from_be_bytes([response[pos + 2], response[pos + 3]]) as usize;
        pos += 4;

        if pos + attr_len > response.len() {
            break;
        }

        // Check for XOR-MAPPED-ADDRESS (0x0020)
        if attr_type == 0x0020 && attr_len >= 8 {
            let family = response[pos + 1];
            // an IPv6 address needs 20 bytes of attribute (reserved, family, port, 16 address bytes)
            if family == 0x02 && attr_len >= 20 {
                // IPv6
                let addr_xor = &response[pos + 4..pos + 20];

                // RFC 5389: the 16 address bytes are XORed with the magic cookie followed by the 12-byte transaction id,
                // i.e. with bytes 4..20 of the message header.
                let mut addr_bytes = [0u8; 16];
                for i in 0..16 {
                    addr_bytes[i] = addr_xor[i] ^ response[4 + i];
                }

                let addr = Ipv6Addr::from(addr_bytes);
                return Some(addr.to_string());
            }
        }

        pos += (attr_len + 3) & !3; // Round to multiple of 4
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stun_response(attr_len: u16, address: [u8; 16], total_attr: usize) -> Vec<u8> {
        // header: type 0x0101, length, magic cookie, transaction id
        let txid: [u8; 12] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let mut r = vec![0x01, 0x01, 0, 0, 0x21, 0x12, 0xa4, 0x42];
        r.extend_from_slice(&txid);
        r.extend_from_slice(&[0x00, 0x20]);
        r.extend_from_slice(&attr_len.to_be_bytes());
        r.extend_from_slice(&[0, 0x02, 0, 0]);
        let mut key = vec![0x21, 0x12, 0xa4, 0x42];
        key.extend_from_slice(&txid);
        for (i, b) in address.iter().enumerate() {
            r.push(b ^ key[i]);
        }
        r.truncate(20 + 4 + total_attr);
        r
    }

    #[test]
    fn an_ipv6_mapped_address_is_decoded_with_the_transaction_id_and_a_short_attribute_is_refused() {
        let addr: std::net::Ipv6Addr = "2001:db8:1234:5678:9abc:def0:1111:2222".parse().unwrap();
        let ok = stun_response(20, addr.octets(), 20);
        assert_eq!(parse_stun_response(&ok), Some(addr.to_string()));
        // an attribute that says "IPv6" but carries only 8 bytes must not panic
        let short = stun_response(8, addr.octets(), 8);
        assert_eq!(parse_stun_response(&short), None);
    }

    #[test]
    fn test_stun_discovery() {
        if let Ok(Some(ip)) = discover_external_ipv6() {
            println!("Discovered external IPv6: {}", ip);
            assert!(ip.parse::<Ipv6Addr>().is_ok());
        }
    }
}
