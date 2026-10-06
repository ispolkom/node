// src/util/mask.rs
//! Security Masking for Sensitive Data
//! =====================================
//!
//! Utilities for masking sensitive information in logs and console output

use crate::util::HashId;

/// Mask a HashId to show only first 8 and last 4 characters
///
/// Example: `8283219e8d7412fac2121db32fd17db2697e534659debe729a89af9bdb29d02b`
///        -> `8283219e***********d02b`
pub fn mask_hash_id(id: &HashId) -> String {
    let hex = id.to_hex();
    if hex.len() <= 12 {
        return "********".to_string();
    }
    format!("{}***********{}", &hex[..8], &hex[hex.len()-4..])
}

/// Mask a hexadecimal string (32 bytes = 64 hex chars)
///
/// Example: `8283219e8d7412fac2121db32fd17db2697e534659debe729a89af9bdb29d02b`
///        -> `8283219e***********d02b`
pub fn mask_hex(hex: &str) -> String {
    if hex.len() <= 12 {
        return "********".to_string();
    }
    format!("{}***********{}", &hex[..8], &hex[hex.len()-4..])
}

/// Mask an IPv6 address
///
/// Example: `fc00:1234:5678::8283:219e:8d74:12fa`
///        -> `fc00:1234:5678::****:****:****:****`
pub fn mask_ipv6(ipv6: &str) -> String {
    if let Some(pos) = ipv6.find("::") {
        // Replace everything after :: with **** groups
        let prefix = &ipv6[..pos+2]; // Include ::
        let groups_after = ipv6[pos+2..].split(':').count();
        let masked = (0..groups_after).map(|_| "****").collect::<Vec<_>>().join(":");
        format!("{}{}", prefix, masked)
    } else {
        // No :: found, replace all groups with ****
        let groups = ipv6.split(':').count();
        (0..groups).map(|_| "****").collect::<Vec<_>>().join(":")
    }
}

/// Mask a 32-byte public key (X25519 or Ed25519)
///
/// Shows only first 8 and last 4 hex characters
///
/// Example: `8283219e8d7412fac2121db32fd17db2697e534659debe729a89af9bdb29d02b`
///        -> `8283219e***********d02b`
pub fn mask_public_key(key: &[u8; 32]) -> String {
    let hex = hex::encode(key);
    mask_hex(&hex)
}

/// Mask any byte array as hexadecimal
///
/// Example (16 bytes): `8283219e8d7412fac2121db32fd1`
///                   -> `8283219e****2fd1`
pub fn mask_bytes(bytes: &[u8]) -> String {
    let hex = hex::encode(bytes);
    if hex.len() <= 12 {
        return format!("{}****", &hex[..hex.len()/2.min(hex.len())]);
    }
    format!("{}****{}", &hex[..8], &hex[hex.len()-4..])
}

/// Mask a string (like IP address or hostname)
///
/// Shows only first and last parts
///
/// Example: `192.168.1.100` -> `192.***.100`
pub fn mask_string(s: &str, show_first: usize, show_last: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= show_first + show_last {
        return "***".to_string();
    }

    let first: String = chars[..show_first].iter().collect();
    let last: String = chars[chars.len()-show_last..].iter().collect();
    format!("{}***{}", first, last)
}

/// Mask an IPv4 address
///
/// Shows only first octet
///
/// Example: `203.0.113.7` -> `203.***.***.***`
pub fn mask_ipv4(ipv4: &str) -> String {
    let parts: Vec<&str> = ipv4.split('.').collect();
    if parts.len() != 4 {
        return "***.***.***.***".to_string();
    }

    format!("{}.***.***.***", parts[0])
}

/// Format bytes with appropriate unit
///
/// Example: `1024` -> `1.00 KB`, `1048576` -> `1.00 MB`
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{} {}", bytes, UNITS[unit_index])
    } else {
        format!("{:.2} {}", size, UNITS[unit_index])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_hash_id() {
        let id = HashId([0x42; 32]);
        let masked = mask_hash_id(&id);
        assert!(masked.contains("********"));
        assert!(masked.starts_with("42424242"));
    }

    #[test]
    fn test_mask_ipv6() {
        let ipv6 = "fc00:1234:5678::8283:219e:8d74:12fa";
        let masked = mask_ipv6(ipv6);
        assert!(masked.contains("****"));
        assert!(masked.starts_with("fc00:1234:5678::"));
    }

    #[test]
    fn test_mask_public_key() {
        let key = [0x42; 32];
        let masked = mask_public_key(&key);
        assert!(masked.contains("********"));
        assert!(masked.starts_with("42424242"));
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1048576), "1.00 MB");
        assert_eq!(format_bytes(512), "512 B");
    }
}
