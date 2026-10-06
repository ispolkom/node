// src/util/os_detector.rs
//! OS Detection Utility
//! ===================
//!
//! Runtime OS detection for cross-platform compatibility

use std::fmt;
use anyhow::Result;

/// Supported operating systems
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OperatingSystem {
    Windows,
    MacOS,
    Linux,
    Android,
    IOS,
    Unknown,
}

impl fmt::Display for OperatingSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OperatingSystem::Windows => write!(f, "Windows"),
            OperatingSystem::MacOS => write!(f, "macOS"),
            OperatingSystem::Linux => write!(f, "Linux"),
            OperatingSystem::Android => write!(f, "Android"),
            OperatingSystem::IOS => write!(f, "iOS"),
            OperatingSystem::Unknown => write!(f, "Unknown"),
        }
    }
}

/// System information
#[derive(Debug, Clone)]
pub struct SystemInfo {
    pub os: OperatingSystem,
    pub version: Option<String>,
    pub architecture: String,
    pub hostname: String,
}

impl SystemInfo {
    pub fn new(os: OperatingSystem, version: Option<String>, architecture: String, hostname: String) -> Self {
        Self {
            os,
            version,
            architecture,
            hostname,
        }
    }

    /// Get OS family
    pub fn family(&self) -> OSFamily {
        match self.os {
            OperatingSystem::Linux | OperatingSystem::MacOS | OperatingSystem::Android | OperatingSystem::IOS => {
                OSFamily::Unix
            }
            OperatingSystem::Windows => OSFamily::Windows,
            OperatingSystem::Unknown => OSFamily::Unknown,
        }
    }

    /// Check if mobile
    pub fn is_mobile(&self) -> bool {
        matches!(self.os, OperatingSystem::Android | OperatingSystem::IOS)
    }

    /// Check if desktop
    pub fn is_desktop(&self) -> bool {
        matches!(self.os, OperatingSystem::Windows | OperatingSystem::MacOS | OperatingSystem::Linux)
    }
}

/// OS family
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OSFamily {
    Unix,
    Windows,
    Unknown,
}

impl fmt::Display for OSFamily {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OSFamily::Unix => write!(f, "Unix-like"),
            OSFamily::Windows => write!(f, "Windows"),
            OSFamily::Unknown => write!(f, "Unknown"),
        }
    }
}

/// OS Detector
pub struct OSDetector;

impl OSDetector {
    /// Detect current operating system
    pub fn detect() -> Result<SystemInfo> {
        let os = Self::detect_os();
        let version = Self::detect_version(&os)?;
        let architecture = Self::detect_architecture();
        let hostname = Self::detect_hostname()?;

        Ok(SystemInfo::new(os, version, architecture, hostname))
    }

    /// Quick OS type detection
    pub fn detect_os_type() -> OperatingSystem {
        Self::detect_os()
    }

    /// Detect OS
    fn detect_os() -> OperatingSystem {
        // Compile-time detection
        #[cfg(target_os = "windows")]
        {
            return OperatingSystem::Windows;
        }

        #[cfg(target_os = "macos")]
        {
            return OperatingSystem::MacOS;
        }

        #[cfg(target_os = "linux")]
        {
            if Self::is_android() {
                return OperatingSystem::Android;
            }
            return OperatingSystem::Linux;
        }

        #[cfg(target_os = "ios")]
        {
            return OperatingSystem::IOS;
        }

        #[cfg(not(any(
            target_os = "windows",
            target_os = "macos",
            target_os = "linux",
            target_os = "ios"
        )))]
        {
            // Runtime detection for unknown systems
            return Self::detect_os_runtime();
        }

        #[cfg(any(
            target_os = "windows",
            target_os = "macos",
            target_os = "linux",
            target_os = "ios"
        ))]
        {
            // This branch is needed for type checking but never executed
            // All known OS cases return above
            #[allow(unreachable_code)]
            {
                unreachable!("All known OS cases should be handled above")
            }
        }
    }

    /// Runtime OS detection
    fn detect_os_runtime() -> OperatingSystem {
        // Windows
        if std::path::Path::new("C:\\Windows").exists() {
            return OperatingSystem::Windows;
        }

        // macOS
        if std::path::Path::new("/System/Library").exists() &&
           std::path::Path::new("/Applications").exists() {
            return OperatingSystem::MacOS;
        }

        // Android
        if Self::is_android() {
            return OperatingSystem::Android;
        }

        // iOS
        if std::path::Path::new("/var/mobile").exists() {
            return OperatingSystem::IOS;
        }

        // Linux
        if std::path::Path::new("/proc").exists() &&
           std::path::Path::new("/sys").exists() {
            return OperatingSystem::Linux;
        }

        OperatingSystem::Unknown
    }

    /// Check if Android
    fn is_android() -> bool {
        std::path::Path::new("/system/build.prop").exists() ||
        std::path::Path::new("/system/framework").exists() ||
        std::env::var("ANDROID_ROOT").is_ok()
    }

    /// Detect OS version
    fn detect_version(os: &OperatingSystem) -> Result<Option<String>> {
        match os {
            OperatingSystem::Windows => Self::get_windows_version(),
            OperatingSystem::MacOS => Self::get_macos_version(),
            OperatingSystem::Linux => Self::get_linux_version(),
            OperatingSystem::Android => Self::get_android_version(),
            OperatingSystem::IOS => Self::get_ios_version(),
            OperatingSystem::Unknown => Ok(None),
        }
    }

    /// Get Windows version
    fn get_windows_version() -> Result<Option<String>> {
        #[cfg(target_os = "windows")]
        {
            use std::process::Command;
            if let Ok(output) = Command::new("cmd")
                .args(&["/c", "ver"])
                .output()
            {
                let version = String::from_utf8_lossy(&output.stdout);
                return Ok(Some(version.trim().to_string()));
            }
        }
        Ok(None)
    }

    /// Get macOS version
    fn get_macos_version() -> Result<Option<String>> {
        #[cfg(target_os = "macos")]
        {
            use std::process::Command;
            if let Ok(output) = Command::new("sw_vers")
                .args(&["-productVersion"])
                .output()
            {
                let version = String::from_utf8_lossy(&output.stdout);
                return Ok(Some(version.trim().to_string()));
            }
        }
        Ok(None)
    }

    /// Get Linux version
    fn get_linux_version() -> Result<Option<String>> {
        if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
            for line in content.lines() {
                if line.starts_with("VERSION_ID=") {
                    let version = line.split('=').nth(1)
                        .unwrap_or("")
                        .trim_matches('"');
                    return Ok(Some(version.to_string()));
                }
            }
        }
        Ok(None)
    }

    /// Get Android version
    fn get_android_version() -> Result<Option<String>> {
        if let Ok(content) = std::fs::read_to_string("/system/build.prop") {
            for line in content.lines() {
                if line.starts_with("ro.build.version.release=") {
                    let version = line.split('=').nth(1).unwrap_or("");
                    return Ok(Some(version.to_string()));
                }
            }
        }
        Ok(None)
    }

    /// Get iOS version
    fn get_ios_version() -> Result<Option<String>> {
        #[cfg(target_os = "ios")]
        {
            // iOS version detection would require Objective-C/Swift bridge
        }
        Ok(None)
    }

    /// Detect architecture
    fn detect_architecture() -> String {
        #[cfg(target_arch = "x86_64")]
        return "x86_64".to_string();

        #[cfg(target_arch = "x86")]
        return "x86".to_string();

        #[cfg(target_arch = "aarch64")]
        return "aarch64".to_string();

        #[cfg(target_arch = "arm")]
        return "arm".to_string();

        #[cfg(not(any(
            target_arch = "x86_64",
            target_arch = "x86",
            target_arch = "aarch64",
            target_arch = "arm"
        )))]
        return "unknown".to_string();

        #[cfg(any(
            target_arch = "x86_64",
            target_arch = "x86",
            target_arch = "aarch64",
            target_arch = "arm"
        ))]
        {
            // This branch is needed for type checking but never executed
            // All known architectures return above
            #[allow(unreachable_code)]
            {
                unreachable!("All known architectures should be handled above")
            }
        }
    }

    /// Detect hostname
    fn detect_hostname() -> Result<String> {
        #[cfg(unix)]
        {
            use std::process::Command;
            if let Ok(output) = Command::new("hostname").output() {
                let hostname = String::from_utf8_lossy(&output.stdout);
                return Ok(hostname.trim().to_string());
            }
        }

        #[cfg(windows)]
        {
            use std::process::Command;
            if let Ok(output) = Command::new("hostname").output() {
                let hostname = String::from_utf8_lossy(&output.stdout);
                return Ok(hostname.trim().to_string());
            }
        }

        Ok("localhost".to_string())
    }

    /// Print system info
    pub fn print_info(&self) {
        match Self::detect() {
            Ok(info) => {
                println!("💻 System Information:");
                println!("   OS: {} {}", info.os, info.version.as_ref().unwrap_or(&String::new()));
                println!("   Architecture: {}", info.architecture);
                println!("   Hostname: {}", info.hostname);
                println!("   Family: {}", info.family());
                println!();
            }
            Err(e) => {
                println!("⚠️  Failed to detect system info: {}", e);
            }
        }
    }
}
