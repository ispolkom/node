// src/netlayer/interface_detector.rs
//! Network Interface Detection
//! ===========================
//!
//! Detect network interfaces and topology

use std::net::{IpAddr, Ipv4Addr};
use std::process::Command;
use anyhow::{Result, anyhow};

#[derive(Debug, Clone)]
pub struct NetworkInterface {
    pub name: String,
    pub ip: IpAddr,
    pub netmask: String,
    pub is_wan: bool,
    pub is_lan: bool,
    pub gateway: Option<IpAddr>,
}

#[derive(Debug, Clone)]
pub struct NetworkTopology {
    pub interfaces: Vec<NetworkInterface>,
    pub wan_interface: Option<NetworkInterface>,
    pub lan_interfaces: Vec<NetworkInterface>,
    pub is_multi_homed: bool,
}

impl NetworkTopology {
    pub fn detect() -> Result<Self> {
        println!("🌐 Detecting network topology...");

        #[cfg(target_os = "linux")]
        {
            return Self::detect_linux();
        }

        #[cfg(target_os = "macos")]
        {
            return Self::detect_macos();
        }

        #[cfg(target_os = "windows")]
        {
            return Self::detect_windows();
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            // Fallback for unknown platforms
            println!("   ⚠️  Unknown platform, using minimal detection");
            return Ok(Self {
                interfaces: vec![],
                wan_interface: None,
                lan_interfaces: vec![],
                is_multi_homed: false,
            });
        }
    }

    #[cfg(target_os = "linux")]
    fn detect_linux() -> Result<Self> {
        let mut interfaces = Vec::new();

        // Get all network interfaces
        let output = Command::new("ip")
            .args(&["addr", "show"])
            .output()
            .map_err(|e| anyhow!("Failed to run 'ip addr show': {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse output
        let mut current_interface: Option<(String, Vec<String>)> = None;

        for line in stdout.lines() {
            if line.starts_with(' ') || line.starts_with('\t') {
                // Interface details continuation
                if let Some((_, ref mut details)) = current_interface {
                    details.push(line.trim().to_string());
                }
            } else if !line.is_empty() {
                // New interface
                if let Some((ref name, ref details)) = current_interface {
                    if let Some(interface) = Self::parse_interface(name, details) {
                        interfaces.push(interface);
                    }
                }

                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() >= 2 {
                    let name = parts[1].trim().to_string();
                    current_interface = Some((name, Vec::new()));
                }
            }
        }

        // Process last interface
        if let Some((ref name, ref details)) = current_interface {
            if let Some(interface) = Self::parse_interface(name, details) {
                interfaces.push(interface);
            }
        }

        // Get routing info for gateway detection
        let gateways = Self::get_default_gateways_linux()?;

        // Add gateway info to interfaces
        for interface in &mut interfaces {
            if let Some(gateway_ip) = gateways.get(&interface.name) {
                interface.gateway = Some(*gateway_ip);
            }
        }

        // Classify interfaces
        let wan_interface = interfaces.iter()
            .find(|iface| iface.is_wan)
            .cloned();

        let lan_interfaces: Vec<NetworkInterface> = interfaces.iter()
            .filter(|iface| iface.is_lan)
            .cloned()
            .collect();

        let is_multi_homed = wan_interface.is_some() && !lan_interfaces.is_empty();

        println!("   📡 Found {} interface(s)", interfaces.len());
        if let Some(ref wan) = wan_interface {
            let wan_ip_str = wan.ip.to_string();
            println!("   🌐 WAN: {} ({})", wan.name, crate::util::mask_ipv4(&wan_ip_str));
        }
        for lan in &lan_interfaces {
            let lan_ip_str = lan.ip.to_string();
            println!("   🔗 LAN: {} ({})", lan.name, crate::util::mask_ipv4(&lan_ip_str));
        }
        if is_multi_homed {
            println!("   ✨ Multi-homed: Yes");
        }
        println!();

        Ok(NetworkTopology {
            interfaces,
            wan_interface,
            lan_interfaces,
            is_multi_homed,
        })
    }

    #[cfg(target_os = "macos")]
    fn detect_macos() -> Result<Self> {
        let mut interfaces = Vec::new();

        // Get all network interfaces using ifconfig
        let output = Command::new("ifconfig")
            .args(&["-a"])
            .output()
            .map_err(|e| anyhow!("Failed to run 'ifconfig -a': {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse ifconfig output (macOS format)
        for (name, details) in Self::split_ifconfig_macos(&stdout) {
            if let Some(interface) = Self::parse_interface_macos(&name, &details) {
                interfaces.push(interface);
            }
        }

        // Get routing info
        let gateways = Self::get_default_gateways_macos()?;

        // Add gateway info
        for interface in &mut interfaces {
            if let Some(gateway_ip) = gateways.get(&interface.name) {
                interface.gateway = Some(*gateway_ip);
            }
        }

        // Classify
        let wan_interface = interfaces.iter()
            .find(|iface| iface.is_wan)
            .cloned();

        let lan_interfaces: Vec<NetworkInterface> = interfaces.iter()
            .filter(|iface| iface.is_lan)
            .cloned()
            .collect();

        let is_multi_homed = wan_interface.is_some() && !lan_interfaces.is_empty();

        println!("   📡 Found {} interface(s)", interfaces.len());
        if let Some(ref wan) = wan_interface {
            let wan_ip_str = wan.ip.to_string();
            println!("   🌐 WAN: {} ({})", wan.name, crate::util::mask_ipv4(&wan_ip_str));
        }
        for lan in &lan_interfaces {
            let lan_ip_str = lan.ip.to_string();
            println!("   🔗 LAN: {} ({})", lan.name, crate::util::mask_ipv4(&lan_ip_str));
        }
        if is_multi_homed {
            println!("   ✨ Multi-homed: Yes");
        }
        println!();

        Ok(NetworkTopology {
            interfaces,
            wan_interface,
            lan_interfaces,
            is_multi_homed,
        })
    }

    #[cfg(target_os = "windows")]
    fn detect_windows() -> Result<Self> {
        use std::process::Command;

        let mut interfaces = Vec::new();

        // Get network interfaces using ipconfig
        let output = Command::new("ipconfig")
            .args(&["/all"])
            .output()
            .map_err(|e| anyhow!("Failed to run 'ipconfig /all': {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse ipconfig output (simplified)
        let mut current_adapter: Option<String> = None;

        for line in stdout.lines() {
            if line.contains("Ethernet adapter") || line.contains("Wireless LAN adapter") {
                if let Some(ref name) = current_adapter {
                    // TODO: Parse Windows adapter details
                    // For now, skip detailed parsing
                }
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() >= 2 {
                    current_adapter = Some(parts[1].trim().to_string());
                }
            }
        }

        // Windows detection is complex, return minimal for now
        println!("   ⚠️  Windows interface detection is limited");
        println!();

        Ok(NetworkTopology {
            interfaces,
            wan_interface: None,
            lan_interfaces: vec![],
            is_multi_homed: false,
        })
    }

    fn parse_interface(name: &str, details: &[String]) -> Option<NetworkInterface> {
        // Skip loopback and virtual interfaces
        if name == "lo" || name.starts_with("docker") || name.starts_with("veth") || name.starts_with("virbr") {
            return None;
        }

        let mut ip_addr = None;
        let mut netmask = None;

        for detail in details {
            if detail.contains("inet ") && !detail.contains("inet6") {
                // Parse IPv4 address
                let inet_part = detail.split_whitespace()
                    .find(|part| part.contains('.'))
                    .unwrap_or("");

                if let Some(slash_pos) = inet_part.find('/') {
                    let addr_str = &inet_part[..slash_pos];
                    if let Ok(addr) = addr_str.parse::<IpAddr>() {
                        ip_addr = Some(addr);
                        let cidr = &inet_part[slash_pos + 1..];
                        netmask = Some(cidr.to_string());
                    }
                }
            }
        }

        let ip = ip_addr?;
        let is_lan = Self::is_lan_ip(&ip);
        let is_wan = !is_lan;

        Some(NetworkInterface {
            name: name.to_string(),
            ip,
            netmask: netmask.unwrap_or_default(),
            is_wan,
            is_lan,
            gateway: None,
        })
    }

    fn is_lan_ip(ip: &IpAddr) -> bool {
        match ip {
            IpAddr::V4(ipv4) => {
                let octets = ipv4.octets();
                // 10.0.0.0/8
                if octets[0] == 10 {
                    return true;
                }
                // 172.16.0.0/12
                if octets[0] == 172 && octets[1] >= 16 && octets[1] <= 31 {
                    return true;
                }
                // 192.168.0.0/16
                if octets[0] == 192 && octets[1] == 168 {
                    return true;
                }
                // 169.254.0.0/16 (link-local)
                if octets[0] == 169 && octets[1] == 254 {
                    return true;
                }
                false
            }
            IpAddr::V6(_) => false, // Simplified for IPv6
        }
    }

    fn get_default_gateways() -> Result<std::collections::HashMap<String, IpAddr>> {
        let mut gateways = std::collections::HashMap::new();

        let output = Command::new("ip")
            .args(&["route"])
            .output()
            .map_err(|e| anyhow!("Failed to run 'ip route': {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        for line in stdout.lines() {
            if line.starts_with("default") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3 {
                    if let Some(dev_index) = parts.iter().position(|&p| p == "dev") {
                        if let Some(&interface_name) = parts.get(dev_index + 1) {
                            if let Some(gateway_str) = parts.get(2) {
                                if let Ok(gateway_ip) = gateway_str.parse::<IpAddr>() {
                                    gateways.insert(interface_name.to_string(), gateway_ip);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(gateways)
    }

    #[cfg(target_os = "linux")]
    fn get_default_gateways_linux() -> Result<std::collections::HashMap<String, IpAddr>> {
        let mut gateways = std::collections::HashMap::new();

        let output = Command::new("ip")
            .args(&["route"])
            .output()
            .map_err(|e| anyhow!("Failed to run 'ip route': {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        for line in stdout.lines() {
            if line.starts_with("default") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3 {
                    if let Some(dev_index) = parts.iter().position(|&p| p == "dev") {
                        if let Some(&interface_name) = parts.get(dev_index + 1) {
                            if let Some(gateway_str) = parts.get(2) {
                                if let Ok(gateway_ip) = gateway_str.parse::<IpAddr>() {
                                    gateways.insert(interface_name.to_string(), gateway_ip);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(gateways)
    }

    #[cfg(target_os = "macos")]
    fn get_default_gateways_macos() -> Result<std::collections::HashMap<String, IpAddr>> {
        let mut gateways = std::collections::HashMap::new();

        let output = Command::new("netstat")
            .args(&["-rn"])
            .output()
            .map_err(|e| anyhow!("Failed to run 'netstat -rn': {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        for line in stdout.lines() {
            if line.starts_with("default") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(gateway_ip) = parts[1].parse::<IpAddr>() {
                        if parts.len() >= 6 {
                            gateways.insert(parts[5].to_string(), gateway_ip);
                        }
                    }
                }
            }
        }

        Ok(gateways)
    }

    /// Разбить вывод `ifconfig -a` по интерфейсам: заголовок — строка без отступа с двоеточием (`en0: flags=8863<UP,...> mtu 1500`),
    /// имя — всё до первого двоеточия; остальные строки (с отступом) — подробности предыдущего интерфейса.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn split_ifconfig_macos(text: &str) -> Vec<(String, Vec<String>)> {
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        for line in text.lines() {
            let header = !line.is_empty() && !line.starts_with(char::is_whitespace);
            match line.find(':') {
                Some(i) if header => out.push((line[..i].to_string(), Vec::new())),
                _ => {
                    if let Some((_, details)) = out.last_mut() {
                        details.push(line.to_string());
                    }
                }
            }
        }
        out
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn parse_interface_macos(name: &str, details: &[String]) -> Option<NetworkInterface> {
        // Skip loopback and virtual interfaces
        if name == "lo0" || name.starts_with("vmnet") || name.starts_with("utun") {
            return None;
        }

        let mut ip_addr = None;
        let mut netmask = None;

        for detail in details {
            if detail.contains("inet ") && !detail.contains("inet6") {
                let parts: Vec<&str> = detail.split_whitespace().collect();
                for (i, part) in parts.iter().enumerate() {
                    if *part == "inet" {
                        // адрес — слово сразу после `inet` (а не `broadcast`, `-->` и т. п. в той же строке)
                        if let Some(addr) = parts.get(i + 1).and_then(|a| a.parse::<IpAddr>().ok()) {
                            ip_addr = Some(addr);
                        }
                    } else if *part == "netmask" {
                        // `netmask 0xffffff00`: маска — следующее слово, шестнадцатеричная
                        if let Some(mask) = parts.get(i + 1).and_then(|m| u32::from_str_radix(m.trim_start_matches("0x"), 16).ok()) {
                            netmask = Some(mask.count_ones().to_string());
                        }
                    }
                }
            }
        }

        let ip = ip_addr?;
        let is_lan = Self::is_lan_ip(&ip);
        let is_wan = !is_lan;

        Some(NetworkInterface {
            name: name.to_string(),
            ip,
            netmask: netmask.unwrap_or_default(),
            is_wan,
            is_lan,
            gateway: None,
        })
    }

    pub fn get_primary_wan_ip(&self) -> Option<IpAddr> {
        self.wan_interface.as_ref().map(|iface| iface.ip)
    }

    pub fn get_primary_lan_ip(&self) -> Option<IpAddr> {
        self.lan_interfaces.first().map(|iface| iface.ip)
    }

    pub fn get_best_bind_ip(&self) -> IpAddr {
        if let Some(wan) = &self.wan_interface {
            wan.ip
        } else if let Some(lan) = self.lan_interfaces.first() {
            lan.ip
        } else {
            // Fallback to localhost
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
        }
    }
}

#[cfg(test)]
mod macos_parse_tests {
    use super::*;

    const IFCONFIG: &str = "lo0: flags=8049<UP,LOOPBACK,RUNNING,MULTICAST> mtu 16384\n\tinet 127.0.0.1 netmask 0xff000000\nen0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500\n\tether 11:22:33:44:55:66\n\tinet6 fe80::1%en0 prefixlen 64 scopeid 0x6\n\tinet 192.168.1.23 netmask 0xffffff00 broadcast 192.168.1.255\n\tstatus: active\nutun0: flags=8051<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 1380\n\tinet 10.8.0.2 --> 10.8.0.1 netmask 0xffffff00\n";

    #[test]
    fn real_ifconfig_headers_are_recognised_and_en0_gets_its_address_and_mask() {
        let blocks = NetworkTopology::split_ifconfig_macos(IFCONFIG);
        let names: Vec<&str> = blocks.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["lo0", "en0", "utun0"], "заголовок вида `en0: flags=…` — не только строка, оканчивающаяся двоеточием");
        // строка `status: active` с отступом — подробность, а не новый интерфейс
        let en0 = blocks.iter().find(|(n, _)| n == "en0").unwrap();
        let iface = NetworkTopology::parse_interface_macos("en0", &en0.1).expect("en0 разобран");
        assert_eq!(iface.ip, "192.168.1.23".parse::<IpAddr>().unwrap(), "адрес — после inet, а не broadcast");
        assert_eq!(iface.netmask, "24");
        assert!(iface.is_lan && !iface.is_wan);
        assert!(NetworkTopology::parse_interface_macos("lo0", &blocks[0].1).is_none());
        assert!(NetworkTopology::parse_interface_macos("utun0", &blocks[2].1).is_none());
    }
}
