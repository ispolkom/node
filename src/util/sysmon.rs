// src/util/sysmon.rs
//! System Monitoring Module
//! ========================
//!
//! Мониторинг системных ресурсов для определения мощности ноды
//! и адаптации нагрузки под возможности устройства.

use std::fmt;
use serde::{Serialize, Deserialize};

/// Мощность ноды
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodePower {
    /// Низкая мощность (мобильные устройства, слабые системы)
    Low,
    /// Средняя мощность (обычные desktop)
    Medium,
    /// Высокая мощность (серверы, мощные рабочие станции)
    High,
}

impl fmt::Display for NodePower {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodePower::Low => write!(f, "Low (мобильное/слабое)"),
            NodePower::Medium => write!(f, "Medium (обычный PC)"),
            NodePower::High => write!(f, "High (сервер/мощный PC)"),
        }
    }
}

impl NodePower {
    /// Получить множитель нагрузки (0.0 - 1.0)
    pub fn load_multiplier(&self) -> f64 {
        match self {
            NodePower::Low => 0.2,      // 20% нагрузки (мобильные)
            NodePower::Medium => 0.5,   // 50% нагрузки (desktop)
            NodePower::High => 0.7,     // 70% нагрузки (серверы, conservative для VPS)
        }
    }

    /// Максимальное количество одновременных соединений
    pub fn max_connections(&self) -> usize {
        match self {
            NodePower::Low => 10,
            NodePower::Medium => 50,
            NodePower::High => 150,     // Уменьшено с 200 для VPS
        }
    }

    /// Максимальная пропускная способность (Mbps)
    pub fn max_bandwidth_mbps(&self) -> f64 {
        match self {
            NodePower::Low => 10.0,
            NodePower::Medium => 100.0,
            NodePower::High => 500.0,   // Уменьшено с 1000 для VPS
        }
    }

    /// Размер буфера для пакетов
    pub fn packet_buffer_size(&self) -> usize {
        match self {
            NodePower::Low => 100,
            NodePower::Medium => 1000,
            NodePower::High => 5000,    // Уменьшено с 10000 для VPS
        }
    }

    /// Timeout для сетевых операций (секунды)
    pub fn network_timeout_secs(&self) -> u64 {
        match self {
            NodePower::Low => 30,
            NodePower::Medium => 15,
            NodePower::High => 10,      // Увеличено с 5 для VPS
        }
    }

    /// Преобразовать в u16 для capabilities в Hello пакете
    pub fn to_capability_bits(&self) -> u16 {
        match self {
            NodePower::Low => 0b00000000_00000001,    // Бит 0: Low power
            NodePower::Medium => 0b00000000_00000010, // Бит 1: Medium power
            NodePower::High => 0b00000000_00000100,   // Бит 2: High power
        }
    }

    /// Из capabilities битов
    pub fn from_capability_bits(bits: u16) -> Option<Self> {
        if bits & 0b00000000_00000100 != 0 {
            Some(NodePower::High)
        } else if bits & 0b00000000_00000010 != 0 {
            Some(NodePower::Medium)
        } else if bits & 0b00000000_00000001 != 0 {
            Some(NodePower::Low)
        } else {
            None
        }
    }
}

/// Информация о CPU
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuInfo {
    /// Количество физических ядер
    pub physical_cores: usize,
    /// Количество логических ядер (с hyperthreading)
    pub logical_cores: usize,
    /// Название процессора
    pub brand: String,
    /// Частота (MHz)
    pub frequency_mhz: Option<u64>,
}

/// Информация о памяти
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryInfo {
    /// Всего памяти (MB)
    pub total_mb: u64,
    /// Доступно памяти (MB)
    pub available_mb: u64,
    /// Использовано памяти (MB)
    pub used_mb: u64,
    /// Процент использования
    pub used_percent: f32,
}

/// Информация о системе
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    /// Операционная система
    pub os: String,
    /// Архитектура
    pub arch: String,
    /// Информация о CPU
    pub cpu: CpuInfo,
    /// Информация о памяти
    pub memory: MemoryInfo,
    /// Мощность ноды
    pub power: NodePower,
    /// Оценка сети (ping до DNS в ms)
    pub network_latency_ms: Option<f64>,
}

impl SystemInfo {
    /// Собрать информацию о системе
    pub fn gather() -> Self {
        println!("🔍 Мониторинг системных ресурсов...");

        // Определяем ОС и архитектуру
        let os = std::env::consts::OS.to_string();
        let arch = std::env::consts::ARCH.to_string();

        // Собираем информацию о CPU
        let cpu = Self::gather_cpu_info(&os);

        // Собираем информацию о памяти
        let memory = Self::gather_memory_info();

        // Определяем мощность ноды
        let power = Self::assess_node_power(&os, &cpu, &memory);

        println!("   💻 CPU: {} ({} ядер)", cpu.brand, cpu.physical_cores);
        println!("   🧠 RAM: {} MB / {} MB ({}%)",
                 memory.used_mb, memory.total_mb, memory.used_percent);
        println!("   ⚡ Мощность: {}", power);

        SystemInfo {
            os,
            arch,
            cpu,
            memory,
            power,
            network_latency_ms: None, // Заполняется отдельно
        }
    }

    /// Собрать информацию о CPU
    #[cfg(target_os = "linux")]
    fn gather_cpu_info(os: &str) -> CpuInfo {
        use std::process::Command;

        // Получаем количество CPU
        let logical_cores = num_cpus::get();
        let physical_cores = if cfg!(target_arch = "x86_64") || cfg!(target_arch = "x86") {
            // Пытаемся определить физические ядра через /proc/cpuinfo
            std::fs::read_to_string("/proc/cpuinfo")
                .ok()
                .and_then(|s| {
                    s.lines()
                        .filter(|line| line.starts_with("cpu cores"))
                        .last()
                        .and_then(|line| {
                            line.split(':')
                                .nth(1)
                                .and_then(|v| v.trim().parse().ok())
                        })
                })
                .unwrap_or(logical_cores)
        } else {
            logical_cores
        };

        // Получаем название процессора
        let brand = std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|line| line.starts_with("model name"))
                    .and_then(|line| line.split(':').nth(1))
                    .map(|s| s.trim().to_string())
            })
            .unwrap_or_else(|| format!("Unknown CPU ({})", os));

        // Пытаемся получить частоту
        let frequency_mhz = std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|line| line.starts_with("cpu MHz"))
                    .and_then(|line| line.split(':').nth(1))
                    .and_then(|v| v.trim().parse().ok())
            });

        CpuInfo {
            physical_cores,
            logical_cores,
            brand,
            frequency_mhz,
        }
    }

    /// Собрать информацию о CPU (non-Linux)
    #[cfg(not(target_os = "linux"))]
    fn gather_cpu_info(os: &str) -> CpuInfo {
        let logical_cores = num_cpus::get();
        let physical_cores = logical_cores; // Упрощение для non-Linux

        let brand = format!("Unknown CPU ({})", os);

        CpuInfo {
            physical_cores,
            logical_cores,
            brand,
            frequency_mhz: None,
        }
    }

    /// Собрать информацию о памяти
    #[cfg(target_os = "linux")]
    fn gather_memory_info() -> MemoryInfo {
        use std::process::Command;

        let output = Command::new("free")
            .arg("-m")
            .output()
            .ok();

        let (total_mb, used_mb, available_mb) = if let Some(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let lines: Vec<&str> = stdout.lines().collect();

            if lines.len() >= 2 {
                let mem_line: Vec<u64> = lines[1]
                    .split_whitespace()
                    .skip(1)
                    .filter_map(|s| s.parse().ok())
                    .collect();

                let total = mem_line.get(0).copied().unwrap_or(1024);
                let used = mem_line.get(1).copied().unwrap_or(0);
                let available = mem_line.get(2).copied().unwrap_or(total);

                (total, used, available)
            } else {
                (1024, 0, 1024)
            }
        } else {
            (1024, 512, 512)
        };

        let used_percent = if total_mb > 0 {
            (used_mb as f32 / total_mb as f32) * 100.0
        } else {
            0.0
        };

        MemoryInfo {
            total_mb,
            available_mb,
            used_mb,
            used_percent,
        }
    }

    /// Собрать информацию о памяти (non-Linux)
    #[cfg(not(target_os = "linux"))]
    fn gather_memory_info() -> MemoryInfo {
        // Упрощённая версия для non-Linux
        let total_mb = 4096; // 4 GB по умолчанию
        let used_mb = 2048;
        let available_mb = total_mb - used_mb;
        let used_percent = 50.0;

        MemoryInfo {
            total_mb,
            available_mb,
            used_mb,
            used_percent,
        }
    }

    /// Оценить мощность ноды на основе характеристик
    fn assess_node_power(os: &str, cpu: &CpuInfo, memory: &MemoryInfo) -> NodePower {
        // Мобильные ОС - всегда низкая мощность
        if os == "android" || os == "ios" {
            return NodePower::Low;
        }

        match os {
            // Linux серверы (VPS, dedicated, desktop)
            "linux" => {
                // High для Linux:
                // - 2+ ядер (VPS с 2 ядрами - стандарт)
                // - 2+ GB RAM
                let is_high_power = cpu.physical_cores >= 2 && memory.total_mb >= 2048;

                if is_high_power {
                    return NodePower::High;
                }

                // Medium для слабых Linux:
                // - 1+ ядер
                // - 1+ GB RAM
                let is_medium_power = cpu.physical_cores >= 1 && memory.total_mb >= 1024;

                if is_medium_power {
                    return NodePower::Medium;
                }

                NodePower::Low
            }

            // Windows (desktop, server)
            "windows" => {
                // High: 4+ ядер И 8+ GB RAM
                let is_high_power = cpu.physical_cores >= 4 && memory.total_mb >= 8192;

                if is_high_power {
                    return NodePower::High;
                }

                // Medium: 2+ ядер И 4+ GB RAM
                let is_medium_power = cpu.physical_cores >= 2 && memory.total_mb >= 4096;

                if is_medium_power {
                    return NodePower::Medium;
                }

                NodePower::Low
            }

            // macOS (desktop, laptop)
            "macos" => {
                // High: 4+ ядер И 8+ GB RAM
                let is_high_power = cpu.physical_cores >= 4 && memory.total_mb >= 8192;

                if is_high_power {
                    return NodePower::High;
                }

                // Medium: 2+ ядер И 4+ GB RAM
                let is_medium_power = cpu.physical_cores >= 2 && memory.total_mb >= 4096;

                if is_medium_power {
                    return NodePower::Medium;
                }

                NodePower::Low
            }

            // Неизвестные ОС - консервативная оценка
            _ => {
                // High только если очень мощное железо
                if cpu.physical_cores >= 8 && memory.total_mb >= 16384 {
                    return NodePower::High;
                }

                // Medium для обычного ПК
                if cpu.physical_cores >= 4 && memory.total_mb >= 8192 {
                    return NodePower::Medium;
                }

                NodePower::Low
            }
        }
    }

    /// Измерить задержку сети (ping до Google DNS)
    pub fn measure_network_latency(&mut self) {
        use std::process::Command;

        println!("🌐 Измерение задержки сети...");

        #[cfg(target_os = "windows")]
        let output = Command::new("ping")
            .args(&["-n", "1", "8.8.8.8"])
            .output();

        #[cfg(not(target_os = "windows"))]
        let output = Command::new("ping")
            .args(&["-c", "1", "8.8.8.8"])
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);

                // Парсим время из вывода ping
                // Linux: "time=1.23 ms"
                // Windows: "time=1ms"
                let latency = stdout
                    .lines()
                    .find_map(|line| {
                        if line.contains("time=") || line.contains("time<") {
                            line.split("time=")
                                .nth(1)
                                .or_else(|| line.split("time<").nth(1))
                                .and_then(|s| {
                                    s.split_whitespace()
                                        .next()
                                        .and_then(|v| {
                                            v.trim_end_matches("ms")
                                                .trim_end_matches('s')
                                                .parse()
                                                .ok()
                                        })
                                })
                        } else {
                            None
                        }
                    });

                if let Some(latency) = latency {
                    self.network_latency_ms = Some(latency);
                    println!("   ⏱️  Задержка: {} ms", latency);
                }
            }
        }
    }

    /// Получить capabilities бит для Hello пакета (legacy — без NodeRole).
    /// Ставит только базовые флаги. Для полной картины используй to_full_capabilities.
    pub fn to_capabilities(&self) -> u16 {
        let mut caps = 0u16;
        caps |= self.power.to_capability_bits();
        if cfg!(target_os = "linux") || cfg!(target_os = "macos") || cfg!(target_os = "windows") {
            caps |= 0b00000000_00001000;
        }
        caps |= 0b00000000_00010000;
        caps |= 0b00000000_00100000;
        caps
    }

    /// Полные capability bits на основе фактов и роли.
    /// Эти биты потом читают peer'ы при выборе relay/introducer/DHT-сервера.
    /// `has_public_ip`, `can_forward`, `is_multi_homed` — факты из NodeCapabilities.
    /// `is_anchor`, `is_mobile` — финальная роль (после CLI override).
    pub fn to_full_capabilities(
        &self,
        has_public_ip: bool,
        can_forward: bool,
        is_multi_homed: bool,
        is_anchor: bool,
        is_mobile: bool,
    ) -> u16 {
        use crate::netlayer::packet::hello_caps::*;
        let mut caps: u16 = 0;
        // Базовые: мощность, IPv6, шифрование, DHT-клиент.
        caps |= self.power.to_capability_bits();
        if cfg!(target_os = "linux") || cfg!(target_os = "macos") || cfg!(target_os = "windows") {
            caps |= 0b00000000_00001000;
        }
        caps |= ENCRYPTED;
        caps |= DHT;
        caps |= NAT_TRAVERSAL; // все умеют участвовать в обходе

        // Mobile исключает все серверные роли независимо от других фактов.
        if is_mobile {
            caps |= MOBILE;
            return caps;
        }

        // Anchor / Border: serve relay, introducer, gateway, mesh.
        if is_anchor && has_public_ip {
            caps |= ANCHOR;
            caps |= RELAY;
            caps |= INTRODUCER;
            caps |= GATEWAY;
            caps |= MESH;
            if can_forward {
                caps |= TUNNEL;
            }
        } else if has_public_ip {
            // Public IP без явной anchor-роли: всё равно может быть relay'ем.
            caps |= RELAY;
            caps |= INTRODUCER;
        }
        // Multi-homed получает доп. бит mesh, если он не lite.
        if is_multi_homed && !is_mobile {
            caps |= MESH;
        }
        caps
    }

    /// Красивое отображение информации
    pub fn display(&self) {
        println!("📊 Системная информация:");

        // Определяем тип ОС для красивого вывода
        let os_type = match self.os.as_str() {
            "linux" => "Linux (серверная)",
            "windows" => "Windows",
            "macos" => "macOS",
            "android" => "Android (мобильная)",
            "ios" => "iOS (мобильная)",
            _ => &self.os,
        };

        println!("   ОС:            {}", os_type);
        println!("   Архитектура:   {}", self.arch);
        println!("   CPU:           {}", self.cpu.brand);
        println!("   Ядра:          {} физ. / {} лог.",
                 self.cpu.physical_cores, self.cpu.logical_cores);
        if let Some(freq) = self.cpu.frequency_mhz {
            println!("   Частота:       {} MHz", freq);
        }
        println!("   Память:        {} MB / {} MB ({:.1}%)",
                 self.memory.used_mb, self.memory.total_mb, self.memory.used_percent);
        println!("   Мощность:      {}", self.power);
        if let Some(latency) = self.network_latency_ms {
            println!("   Задержка сети: {:.2} ms", latency);
        }
        println!("   Load cap:      {}%", (self.power.load_multiplier() * 100.0) as u32);
        println!("   Max conn:      {}", self.power.max_connections());
        println!();
    }
}
