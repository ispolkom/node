// src/netlayer/node_introspection.rs
//! Node Self-Introspection
//! =======================
//!
//! Local capability detection without global topology knowledge

use std::net::IpAddr;
use std::fs;
use anyhow::{Result, anyhow};

/// Node capabilities (FACTS only, not roles!)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeCapabilities {
    pub interface_count: usize,
    pub has_public_ip: bool,
    pub can_forward: bool,
    pub is_multi_homed: bool,
    pub bind_ip: IpAddr,
    pub external_ip: Option<String>,

    /// IPv6 connectivity to internet
    pub has_ipv6: bool,

    /// External IPv6 address (if available)
    pub external_ipv6: Option<String>,

    /// Мощность ноды (определяется на основе системных ресурсов)
    pub power: crate::util::NodePower,

    /// capabilities биты для Hello пакета
    pub capabilities_bits: u16,
}

/// Node role (locally inferred from facts, и одновременно объявляется в Hello caps_bits).
/// Эта роль определяет поведение подсистем: какие сервисы запускать, какие — нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NodeRole {
    /// Мобилка / lite-клиент: только клиент к своему anchor'у. Не сервер.
    /// Не запускает DHT-сервер, не форвардит relay-трафик, не выступает introducer'ом.
    Mobile,
    /// Десктоп за NAT: leaf-нода. Может быть peer'ом, но не релеем.
    Leaf,
    /// Anchor — домашний ПК с public IP / port-mapping, 24/7. Полная функциональность:
    /// relay, introducer, accepts paired mobiles. Включает прежний Gateway.
    Anchor,
    /// Multi-homed border-нода: exit/entry в другие сети.
    Border,
}

impl NodeRole {
    pub fn name(&self) -> &'static str {
        match self {
            NodeRole::Mobile => "Mobile",
            NodeRole::Leaf => "Leaf",
            NodeRole::Anchor => "Anchor",
            NodeRole::Border => "Border",
        }
    }

    /// Может ли этот узел вообще принимать роль relay-сервера?
    pub fn can_relay(&self) -> bool {
        matches!(self, NodeRole::Anchor | NodeRole::Border)
    }

    /// Может ли узел быть hole-punching introducer'ом?
    pub fn can_introduce(&self) -> bool {
        matches!(self, NodeRole::Anchor | NodeRole::Border)
    }

    /// Должен ли узел запускать DHT-сервер (а не только клиент)?
    pub fn runs_dht_server(&self) -> bool {
        matches!(self, NodeRole::Anchor | NodeRole::Border | NodeRole::Leaf)
    }

    /// Является ли узел lite-клиентом (мобилка)?
    pub fn is_lite(&self) -> bool {
        matches!(self, NodeRole::Mobile)
    }
}

/// Node self-introspection result
pub struct NodeIntrospection {
    pub capabilities: NodeCapabilities,
    pub role: NodeRole,
}

impl NodeIntrospection {
    /// Perform node self-introspection
    pub fn detect(
        topology: &crate::netlayer::NetworkTopology,
        external_ip: Option<String>,
        sys_info: &crate::util::SystemInfo
    ) -> Result<Self> {
        Self::detect_with_ipv6(topology, external_ip, None, sys_info)
    }

    /// Perform node self-introspection with external IPv6
    pub fn detect_with_ipv6(
        topology: &crate::netlayer::NetworkTopology,
        external_ip: Option<String>,
        external_ipv6: Option<String>,
        sys_info: &crate::util::SystemInfo
    ) -> Result<Self> {
        Self::detect_with_override(topology, external_ip, external_ipv6, sys_info, None)
    }

    /// Perform node self-introspection с возможностью форсировать роль (CLI флаги --lite/--anchor).
    pub fn detect_with_override(
        topology: &crate::netlayer::NetworkTopology,
        external_ip: Option<String>,
        external_ipv6: Option<String>,
        sys_info: &crate::util::SystemInfo,
        forced_role: Option<NodeRole>,
    ) -> Result<Self> {
        println!("🔍 Performing node self-introspection...");

        // 1. Count interfaces
        let interface_count = topology.interfaces.len();

        // 2. Check for public IP
        let has_public_ip = topology.wan_interface.is_some();

        // 3. Check IP forwarding
        let can_forward = Self::check_ip_forwarding()?;

        // 4. Check multi-homed
        let is_multi_homed = topology.is_multi_homed;

        // 5. Get bind IP
        let bind_ip = topology.get_best_bind_ip();

        // 6. Check IPv6 connectivity
        println!("🌍 Checking IPv6 connectivity to internet...");
        let has_ipv6 = Self::check_ipv6_connectivity()?;
        println!("   🌐 IPv6: {}", if has_ipv6 { "✅ Available" } else { "❌ Not available" });

        // 6a. Use provided external IPv6 or None
        // Note: external_ipv6 will be filled by async code in main.rs
        let _external_ipv6 = external_ipv6;

        // 7. Сначала роль (нужна для caps_bits).
        // Делаем preliminary capabilities без bits, чтобы передать в infer_role_with_os.
        let prelim_caps = NodeCapabilities {
            interface_count,
            has_public_ip,
            can_forward,
            is_multi_homed,
            bind_ip,
            external_ip: external_ip.clone(),
            has_ipv6,
            external_ipv6: _external_ipv6.clone(),
            power: sys_info.power,
            capabilities_bits: 0,
        };
        let role = match forced_role {
            Some(r) => {
                println!("   ⚙️  Role overridden by CLI: {}", r.name());
                r
            }
            None => Self::infer_role_with_os(&prelim_caps, &sys_info.os),
        };

        // 8. Capability bits на основе фактов + финальной роли.
        let is_anchor = matches!(role, NodeRole::Anchor | NodeRole::Border);
        let is_mobile = matches!(role, NodeRole::Mobile);
        let capabilities_bits = sys_info.to_full_capabilities(
            has_public_ip,
            can_forward,
            is_multi_homed,
            is_anchor,
            is_mobile,
        );

        let capabilities = NodeCapabilities {
            interface_count,
            has_public_ip,
            can_forward,
            is_multi_homed,
            bind_ip,
            external_ip: external_ip.clone(),
            has_ipv6,
            external_ipv6: _external_ipv6,
            power: sys_info.power,
            capabilities_bits,
        };

        println!("   📊 Interfaces: {}", interface_count);
        println!("   🌐 Public IP: {}", if has_public_ip { "Yes" } else { "No" });
        println!("   🔄 Can forward: {}", if can_forward { "Yes" } else { "No" });
        println!("   ✨ Multi-homed: {}", if is_multi_homed { "Yes" } else { "No" });
        println!("   ⚡ Node power: {}", sys_info.power);
        println!("   🎯 Inferred role: {}", role.name());
        println!();

        Ok(Self {
            capabilities,
            role,
        })
    }

    /// Старая версия — оставлена для обратной совместимости. Не учитывает ОС.
    fn infer_role(caps: &NodeCapabilities) -> NodeRole {
        Self::infer_role_with_os(caps, "")
    }

    /// Вывод роли с учётом ОС: Android/iOS → всегда Mobile, независимо от того, что считают факты
    /// (мобилки могут иметь временный «public» IP в 4G, но это не делает их anchor'ом).
    fn infer_role_with_os(caps: &NodeCapabilities, os: &str) -> NodeRole {
        // Мобильная ОС → Mobile, всегда. Никакого исключения.
        let os_lower = os.to_lowercase();
        if os_lower == "android" || os_lower == "ios" {
            return NodeRole::Mobile;
        }
        match (caps.has_public_ip, caps.can_forward, caps.interface_count) {
            // No public IP = leaf node (behind NAT) на десктопе
            (false, _, _) => NodeRole::Leaf,

            // Public IP + forwarding + multi-homed = border node
            (true, true, n) if n >= 2 => NodeRole::Border,

            // Public IP (с forwarding или без) = Anchor
            (true, _, _) => NodeRole::Anchor,
        }
    }

    /// Check if IP forwarding is enabled (cross-platform)
    fn check_ip_forwarding() -> Result<bool> {
        #[cfg(target_os = "linux")]
        {
            let path = "/proc/sys/net/ipv4/ip_forward";
            match fs::read_to_string(path) {
                Ok(content) => {
                    let value = content.trim().parse::<u32>().unwrap_or(0);
                    return Ok(value == 1);
                }
                Err(_) => Ok(false),
            }
        }

        #[cfg(target_os = "macos")]
        {
            // macOS: check sysctl net.inet.ip.forwarding
            use std::process::Command;
            if let Ok(output) = Command::new("sysctl")
                .args(&["-n", "net.inet.ip.forwarding"])
                .output()
            {
                let value = String::from_utf8_lossy(&output.stdout);
                let parsed = value.trim().parse::<u32>().unwrap_or(0);
                return Ok(parsed == 1);
            }
            return Ok(false);
        }

        #[cfg(target_os = "windows")]
        {
            // Windows: check IP routing via netsh
            // This is complex and usually requires admin privileges
            // For now, assume no forwarding on Windows
            return Ok(false);
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            // Unknown platform: assume no forwarding
            Ok(false)
        }
    }

    /// Check IPv6 connectivity to internet (cross-platform)
    ///
    /// Strategy: Try to reach IPv6 services directly, regardless of local interface config
    /// This works for various IPv6 setups: native, tunnel, NAT64, etc.
    fn check_ipv6_connectivity() -> Result<bool> {
        use std::process::Command;

        #[cfg(target_os = "linux")]
        {
            // Strategy: Try multiple IPv6 services in sequence
            // If ANY responds, we have IPv6 connectivity!

            let test_targets = vec![
                ("ping6", vec!["-c", "1", "-W", "3", "ipv6.google.com"]),
                ("ping6", vec!["-c", "1", "-W", "3", "2001:4860:4860::8888"]),
                ("ping6", vec!["-c", "1", "-W", "3", "test-ipv6.com"]),
            ];

            for (cmd, args) in test_targets {
                if let Ok(output) = Command::new(cmd).args(&args).output() {
                    if output.status.success() {
                        return Ok(true);
                    }
                }
            }

            // If all pings failed, try HTTP over IPv6
            let http_targets = vec![
                "https://ipv6.google.com/",
                "https://api64.ipify.org/",
                "https://test-ipv6.com/",
            ];

            for url in http_targets {
                if let Ok(output) = Command::new("curl")
                    .args(&["-6", "-s", "-o", "/dev/null", "--connect-timeout", "3", "--max-time", "5", url])
                    .output()
                {
                    if output.status.success() {
                        return Ok(true);
                    }
                }
            }

            // No IPv6 connectivity detected
            return Ok(false);
        }

        #[cfg(target_os = "macos")]
        {
            // Same strategy for macOS
            let test_targets = vec![
                ("ping6", vec!["-c", "1", "-t", "3", "ipv6.google.com"]),
                ("ping6", vec!["-c", "1", "-t", "3", "2001:4860:4860::8888"]),
            ];

            for (cmd, args) in test_targets {
                if let Ok(output) = Command::new(cmd).args(&args).output() {
                    if output.status.success() {
                        return Ok(true);
                    }
                }
            }

            let http_targets = vec![
                "https://ipv6.google.com/",
                "https://api64.ipify.org/",
            ];

            for url in http_targets {
                if let Ok(output) = Command::new("curl")
                    .args(&["-6", "-s", "-o", "/dev/null", "--connect-timeout", "3", url])
                    .output()
                {
                    if output.status.success() {
                        return Ok(true);
                    }
                }
            }

            return Ok(false);
        }

        #[cfg(target_os = "windows")]
        {
            // Windows: use ping.exe
            let test_targets = vec![
                ("ping", vec!["-n", "1", "-w", "3000", "ipv6.google.com"]),
                ("ping", vec!["-n", "1", "-w", "3000", "2001:4860:4860::8888"]),
            ];

            for (cmd, args) in test_targets {
                if let Ok(output) = Command::new(cmd).args(&args).output() {
                    if output.status.success() {
                        return Ok(true);
                    }
                }
            }

            return Ok(false);
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            // Unknown platform: assume no IPv6
            Ok(false)
        }
    }

    /// Get capabilities for DHT publication (FACTS only)
    pub fn get_capabilities(&self) -> &NodeCapabilities {
        &self.capabilities
    }

    /// Get role (LOCAL ONLY, never published!)
    pub fn get_role(&self) -> NodeRole {
        self.role
    }
}

/// Node profile for DHT publication (contains only FACTS)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeProfile {
    pub node_id: crate::util::HashId,
    pub capabilities: NodeCapabilities,
    pub asn: Option<String>,
    pub as_org: Option<String>,
    pub country: Option<String>,
    pub city: Option<String>,
    pub timestamp: u64,
}

impl NodeProfile {
    /// Create node profile (only FACTS are published)
    pub fn new(
        node_id: crate::util::HashId,
        capabilities: NodeCapabilities,
        asn: Option<String>,
        as_org: Option<String>,
        country: Option<String>,
        city: Option<String>,
    ) -> Self {
        Self {
            node_id,
            capabilities,
            asn,
            as_org,
            country,
            city,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        }
    }

    /// Check if profile is expired (TTL = 1 hour)
    pub fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        now > self.timestamp + 3600  // 1 hour TTL
    }
}
