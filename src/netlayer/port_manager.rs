// src/netlayer/port_manager.rs
//! Port Rotation Manager
//! =====================
//!
//! Graceful UDP port rotation without breaking active sessions.

use rand::Rng;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use std::sync::Mutex;

pub const DEFAULT_DISCOVERY_PORT: u16 = 9000;
pub const DEFAULT_DATA_PORT: u16 = 10000;

/// Current port state
#[derive(Debug, Clone)]
pub struct PortState {
    /// Discovery port (Hello packets)
    pub discovery_port: u16,
    /// Data port (Encrypted session)
    pub data_port: u16,
    /// Last rotation timestamp
    pub last_rotated: std::time::Instant,
}

impl PortState {
    pub fn new(discovery_port: u16, data_port: u16) -> Self {
        Self {
            discovery_port,
            data_port,
            last_rotated: std::time::Instant::now(),
        }
    }
}

/// Port rotation manager
pub struct PortManager {
    current: Arc<Mutex<PortState>>,
    overlap_duration: Duration,
}

impl PortManager {
    /// Create new port manager
    pub fn new(initial_discovery: u16, initial_data: u16) -> Self {
        Self {
            current: Arc::new(Mutex::new(PortState::new(
                initial_discovery,
                initial_data,
            ))),
            overlap_duration: Duration::from_secs(20), // 20s overlap period
        }
    }

    /// Состояние портов под короткой блокировкой (без ожидания внутри). Раньше `try_lock().unwrap()` паниковал, если другая задача в этот момент держала блокировку
    /// (например, шла смена порта) — настоящая гонка между задачами.
    fn lock(&self) -> std::sync::MutexGuard<'_, PortState> {
        self.current.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Generate random ephemeral port
    pub fn random_ephemeral_port() -> u16 {
        rand::thread_rng().gen_range(49152..=65535)
    }

    /// Generate port pair ensuring they don't conflict
    pub fn generate_port_pair() -> (u16, u16) {
        let discovery = Self::random_ephemeral_port();
        let data = ((discovery as u32 + 1000) % 65536) as u16;
        (discovery.max(49152), data.max(49152))
    }

    /// Check if rotation is needed (5-10 minutes with jitter)
    pub fn should_rotate(&self) -> bool {
        let state = self.lock();
        let elapsed = state.last_rotated.elapsed();

        // Minimum interval: 5 minutes
        let interval_min = Duration::from_secs(20 * 60);

        println!("[port_manager] should_rotate: elapsed={:?}, interval={:?}, result={}", elapsed, interval_min, elapsed >= interval_min);

        elapsed >= interval_min
    }

    /// Get current port state
    pub fn current_state(&self) -> PortState {
        self.lock().clone()
    }

    /// Rotate ports with graceful transition
    /// 
    /// Returns (old_sockets, new_ports) for overlap period
    pub async fn rotate_ports(
        &self,
        old_discovery_socket: &Arc<UdpSocket>,
        old_data_socket: &Arc<UdpSocket>,
    ) -> Result<(Arc<UdpSocket>, Arc<UdpSocket>, PortState), String> {
        let old_state = self.current_state();
        let (new_discovery, new_data) = Self::generate_port_pair();
        
        println!("[port_manager] 🔄 Rotating ports:");
        println!("    Old: {} (discovery) + {} (data)", old_state.discovery_port, old_state.data_port);
        println!("    New: {} (discovery) + {} (data)", new_discovery, new_data);

        // Create new sockets
        let new_discovery_socket = Arc::new(
            UdpSocket::bind(format!("0.0.0.0:{}", new_discovery))
                .await
                .map_err(|e| format!("Failed to bind new discovery socket: {}", e))?
        );

        let new_data_socket = Arc::new(
            UdpSocket::bind(format!("0.0.0.0:{}", new_data))
                .await
                .map_err(|e| format!("Failed to bind new data socket: {}", e))?
        );

        // Update state
        {
        let mut state = self.lock();
            state.discovery_port = new_discovery;
            state.data_port = new_data;
            state.last_rotated = std::time::Instant::now();
        }

        println!("[port_manager] ✅ New sockets bound, entering overlap period ({:?})", self.overlap_duration);

        Ok((new_discovery_socket, new_data_socket, PortState::new(new_discovery, new_data)))
    }

    /// Get overlap duration
    pub fn overlap_duration(&self) -> Duration {
        self.overlap_duration
    }

    /// Update the advertised active ports after a successful rotation
    pub async fn set_current_ports(&self, discovery_port: u16, data_port: u16) {
        let mut state = self.lock();
        state.discovery_port = discovery_port;
        state.data_port = data_port;
        state.last_rotated = std::time::Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Гонка: чтение состояния портов одновременно со сменой не должно паниковать (раньше `try_lock().unwrap()` падал, если блокировку держала другая задача).
    #[test]
    fn reading_ports_while_another_thread_changes_them_never_panics() {
        let pm = Arc::new(PortManager::new(50000, 51000));
        let mut handles = vec![];
        for t in 0..8u16 {
            let pm = pm.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..20_000u16 {
                    if t % 2 == 0 {
                        let s = pm.current_state();
                        assert!(s.discovery_port >= 49152 || s.discovery_port == 50000);
                    } else {
                        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
                        rt.block_on(pm.set_current_ports(49152 + (i % 1000), 50152 + (i % 1000)));
                    }
                    let _ = pm.should_rotate();
                }
            }));
        }
        for h in handles {
            h.join().expect("a thread panicked");
        }
    }

    #[test]
    fn test_random_port_in_range() {
        for _ in 0..100 {
            let port = PortManager::random_ephemeral_port();
            assert!(port >= 49152 && port <= 65535);
        }
    }

    #[test]
    fn test_port_pair_different() {
        let (d1, d2) = PortManager::generate_port_pair();
        assert_ne!(d1, d2);
        assert!(d1 >= 49152 && d1 <= 65535);
        assert!(d2 >= 49152 && d2 <= 65535);
    }
}
