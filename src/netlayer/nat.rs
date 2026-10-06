// src/netlayer/nat.rs
//! NAT Traversal Module
//! ====================
//!
//! NAT detection and relay functionality for P2P communication

use crate::util::HashId;
use crate::netlayer::packet::HelloPacket;
use std::net::SocketAddr;

/// Поведение NAT-маппинга нашего узла относительно внешних peer'ов.
/// RFC 4787: EIM (full-cone, predictable) vs EDM (symmetric, per-destination port).
/// Hole punching возможен между EIM↔EIM и EIM↔EDM (с предсказанием),
/// между EDM↔EDM — почти всегда нет, нужен relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingBehavior {
    /// Не определено (нет данных от ≥2 peer'ов).
    Unknown,
    /// Endpoint-Independent Mapping (full-cone) — одно и то же externalip:port для разных peer'ов.
    EndpointIndependent,
    /// Endpoint-Dependent Mapping (symmetric) — разные externalip:port для разных peer'ов.
    EndpointDependent,
    /// У нас публичный IP, NAT отсутствует.
    NoNat,
}

impl MappingBehavior {
    pub fn as_str(&self) -> &'static str {
        match self {
            MappingBehavior::Unknown => "Unknown",
            MappingBehavior::EndpointIndependent => "EIM (full-cone)",
            MappingBehavior::EndpointDependent => "EDM (symmetric)",
            MappingBehavior::NoNat => "NoNat",
        }
    }

    /// Можно ли вообще пытаться UDP hole punching из такого маппинга.
    pub fn punchable(&self) -> bool {
        matches!(self, MappingBehavior::EndpointIndependent | MappingBehavior::NoNat)
    }
}

/// NAT status of a peer
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NatStatus {
    /// Public IP - can accept incoming connections
    Public,
    /// Behind NAT - cannot accept direct incoming connections
    BehindNat,
    /// Multi-homed - has both public and local interfaces
    MultiHomed,
    /// Unknown - couldn't determine NAT status
    Unknown,
}

impl NatStatus {
    /// Detect NAT status from Hello packet and connection info
    pub fn from_hello_packet(
        packet: &HelloPacket,
        from_addr: &SocketAddr,
        my_external_ip: Option<&str>,
    ) -> Self {
        // Check if peer reported both WAN and LAN addresses
        match (&packet.wan_address, &packet.lan_address) {
            (Some(wan), Some(_lan)) => {
                // Peer reported both addresses
                let is_public_ip = is_public_ip(from_addr.ip());

                if is_public_ip && wan.contains(&from_addr.ip().to_string()) {
                    NatStatus::Public
                } else if is_public_ip {
                    NatStatus::MultiHomed
                } else {
                    NatStatus::BehindNat
                }
            }
            (Some(wan), None) => {
                // Only WAN address reported
                let is_public_ip = is_public_ip(from_addr.ip());

                if is_public_ip && wan.contains(&from_addr.ip().to_string()) {
                    NatStatus::Public
                } else if is_public_ip {
                    NatStatus::MultiHomed
                } else {
                    NatStatus::BehindNat
                }
            }
            (None, Some(_lan)) => {
                // Only LAN address - definitely behind NAT
                NatStatus::BehindNat
            }
            (None, None) => {
                // No addresses reported - infer from source
                if is_public_ip(from_addr.ip()) {
                    NatStatus::Public
                } else {
                    NatStatus::BehindNat
                }
            }
        }
    }

    /// Check if this peer can accept direct connections
    pub fn can_accept_direct_connection(&self) -> bool {
        matches!(self, NatStatus::Public | NatStatus::MultiHomed)
    }

    /// Check if relay is needed for this peer
    pub fn needs_relay(&self) -> bool {
        matches!(self, NatStatus::BehindNat)
    }

    /// Get NAT status as string
    pub fn as_str(&self) -> &'static str {
        match self {
            NatStatus::Public => "Public",
            NatStatus::BehindNat => "BehindNAT",
            NatStatus::MultiHomed => "MultiHomed",
            NatStatus::Unknown => "Unknown",
        }
    }
}

/// Check if IP address is public (not private/local)
pub fn is_public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ipv4) => {
            let octets = ipv4.octets();
            // 10.0.0.0/8
            if octets[0] == 10 {
                return false;
            }
            // 172.16.0.0/12
            if octets[0] == 172 && octets[1] >= 16 && octets[1] <= 31 {
                return false;
            }
            // 192.168.0.0/16
            if octets[0] == 192 && octets[1] == 168 {
                return false;
            }
            // 169.254.0.0/16 (link-local)
            if octets[0] == 169 && octets[1] == 254 {
                return false;
            }
            // 127.0.0.0/8 (loopback)
            if octets[0] == 127 {
                return false;
            }
            // 0.0.0.0 (unspecified)
            if octets[0] == 0 {
                return false;
            }
            true
        }
        std::net::IpAddr::V6(ipv6) => {
            let segments = ipv6.segments();
            // ::1 (loopback)
            if segments[0] == 0 && segments[1] == 0 && segments[2] == 0 && segments[3] == 0
                && segments[4] == 0 && segments[5] == 0 && segments[6] == 0 && segments[7] == 1 {
                return false;
            }
            // fe80::/10 (link-local)
            if (segments[0] & 0xffc0) == 0xfe80 {
                return false;
            }
            // fc00::/7 (unique local)
            if (segments[0] & 0xfe00) == 0xfc00 {
                return false;
            }
            // :: (unspecified)
            if ipv6.is_unspecified() {
                return false;
            }
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_public_ip() {
        assert!(is_public_ip("8.8.8.8".parse().unwrap()));
        assert!(is_public_ip("1.1.1.1".parse().unwrap()));
        assert!(!is_public_ip("192.168.1.1".parse().unwrap()));
        assert!(!is_public_ip("10.0.0.1".parse().unwrap()));
        assert!(!is_public_ip("172.16.0.1".parse().unwrap()));
        assert!(!is_public_ip("127.0.0.1".parse().unwrap()));
    }

    #[test]
    fn test_nat_status_can_accept_direct() {
        assert!(NatStatus::Public.can_accept_direct_connection());
        assert!(NatStatus::MultiHomed.can_accept_direct_connection());
        assert!(!NatStatus::BehindNat.can_accept_direct_connection());
        assert!(!NatStatus::Unknown.can_accept_direct_connection());
    }

    #[test]
    fn test_nat_status_needs_relay() {
        assert!(!NatStatus::Public.needs_relay());
        assert!(!NatStatus::MultiHomed.needs_relay());
        assert!(NatStatus::BehindNat.needs_relay());
        assert!(!NatStatus::Unknown.needs_relay());
    }
}
