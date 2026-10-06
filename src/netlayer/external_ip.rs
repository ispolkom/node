// src/netlayer/external_ip.rs
//! External IP Detection
//! =====================
//!
//! Get external IP address from public services

use reqwest::Client;
use std::time::Duration;
use tokio::time::timeout;
use serde::Deserialize;

pub struct ExternalIpService {
    client: Client,
    services: Vec<&'static str>,
}

impl ExternalIpService {
    pub fn new() -> Self {
        // Свой внешний адрес узел узнаёт НАПРЯМУЮ: через системный прокси сервис назвал бы адрес прокси, и узел сообщал бы соседям
        // чужой адрес (так и было на машине владельца: копии тренировочной сети слали переписку на адрес прокси).
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("Failed to create HTTP client");

        let services = vec![
            "https://api.ipify.org",
            "https://ifconfig.me",
            "https://ipinfo.io/ip",
            "https://icanhazip.com",
            "https://httpbin.org/ip",
        ];

        Self { client, services }
    }

    pub async fn get_external_ip(&self) -> Result<String, String> {
        println!("🌍 Detecting external IP...");

        for service in &self.services {
            match timeout(Duration::from_secs(3), self.client.get(*service).send()).await {
                Ok(Ok(response)) => {
                    if response.status().is_success() {
                        match response.text().await {
                            Ok(text) => {
                                let ip = text.trim().to_string();
                                if !ip.is_empty() && self.is_valid_ip(&ip) {
                                    let masked_ip = crate::util::mask_ipv4(&ip);
                                    println!("   ✅ External IP: {} (from {})", masked_ip, service);
                                    return Ok(ip);
                                }
                            }
                            Err(e) => {
                                eprintln!("   ❌ Failed to read response from {}: {}", service, e);
                            }
                        }
                    } else {
                        eprintln!("   ❌ HTTP {} from {}", response.status(), service);
                    }
                }
                Ok(Err(e)) => {
                    eprintln!("   ❌ HTTP error from {}: {}", service, e);
                }
                Err(_) => {
                    eprintln!("   ❌ Timeout from {}", service);
                }
            }
        }
        Err("All external IP services failed".to_string())
    }

    fn is_valid_ip(&self, ip_str: &str) -> bool {
        ip_str.parse::<std::net::IpAddr>().is_ok()
    }

    /// Get external IPv6 address from local interfaces ONLY
    /// NO external services - no bans!
    pub async fn get_external_ipv6(&self) -> Result<String, String> {
        println!("🌍 Detecting external IPv6 from local interfaces...");

        // Only method: local interfaces (no bans, reliable!)
        match crate::util::discover_external_ipv6() {
            Ok(Some(ip)) => Ok(ip),
            Ok(None) => Err("No IPv6 address found on local interfaces".to_string()),
            Err(e) => Err(format!("Failed to detect IPv6: {}", e)),
        }
    }

    /// Get external IPv6 via DNS lookup
    async fn get_ipv6_via_dns(&self) -> Result<String, String> {
        use std::process::Command;

        // Try to resolve AAAA records for services that return our IP
        let dns_services = vec![
            "myip.opendns.com",
            "myip.google.com",
        ];

        for service in dns_services {
            if let Ok(output) = Command::new("dig")
                .args(&["AAAA", service, "+short", "@resolver1.opendns.com", "-6"])
                .output()
            {
                let text = String::from_utf8_lossy(&output.stdout);
                let ip = text.trim().to_string();

                if !ip.is_empty() && self.is_valid_ipv6(&ip) {
                    let masked_ip = crate::util::mask_ipv6(&ip);
                    println!("   ✅ External IPv6: {} (from DNS via {})", masked_ip, service);
                    return Ok(ip);
                }
            }
        }

        // Fallback: Try without specifying resolver
        if let Ok(output) = Command::new("dig")
            .args(&["AAAA", "myip.opendns.com", "+short"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            let ip = text.trim().to_string();

            if !ip.is_empty() && self.is_valid_ipv6(&ip) {
                let masked_ip = crate::util::mask_ipv6(&ip);
                println!("   ✅ External IPv6: {} (from DNS)", masked_ip);
                return Ok(ip);
            }
        }

        Err("DNS IPv6 detection failed".to_string())
    }

    fn is_valid_ipv6(&self, ip_str: &str) -> bool {
        if let Ok(ip_addr) = ip_str.parse::<std::net::IpAddr>() {
            matches!(ip_addr, std::net::IpAddr::V6(_))
        } else {
            false
        }
    }

    pub async fn get_detailed_ip_info(&self) -> Result<IpInfo, String> {
        let info_services = vec![
            "https://ipinfo.io/json",
            "https://api.ipify.org?format=json",
        ];

        for service in &info_services {
            match timeout(Duration::from_secs(5), self.client.get(*service).send()).await {
                Ok(Ok(response)) => {
                    if response.status().is_success() {
                        match response.text().await {
                            Ok(text) => {
                                if let Ok(info) = self.parse_ip_info(&text) {
                                    println!("   📍 Location: {}, {} ({})", info.city, info.country, info.org);
                                    return Ok(info);
                                }
                            }
                            Err(e) => {
                                eprintln!("   ❌ Failed to read response from {}: {}", service, e);
                            }
                        }
                    }
                }
                Ok(Err(e)) => {
                    eprintln!("   ❌ HTTP error from {}: {}", service, e);
                }
                Err(_) => {
                    eprintln!("   ❌ Timeout from {}", service);
                }
            }
        }

        // Fallback: try to get just the IP
        let ip = self.get_external_ip().await?;
        Ok(IpInfo {
            ip,
            city: "Unknown".to_string(),
            country: "Unknown".to_string(),
            org: "Unknown".to_string(),
        })
    }

    fn parse_ip_info(&self, json: &str) -> Result<IpInfo, serde_json::Error> {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json) {
            let ip = parsed["ip"].as_str().unwrap_or("unknown").to_string();
            let city = parsed["city"].as_str().unwrap_or("Unknown").to_string();
            let country = parsed["country"].as_str().unwrap_or("Unknown").to_string();
            let org = parsed["org"].as_str().unwrap_or("Unknown").to_string();

            Ok(IpInfo { ip, city, country, org })
        } else {
            // Try ipify format
            serde_json::from_str(json)
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct IpInfo {
    pub ip: String,
    pub city: String,
    pub country: String,
    pub org: String,
}

// For ipify format
#[derive(Debug, Deserialize)]
struct IpifyResponse {
    ip: String,
}
