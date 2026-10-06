//! Telemetry collection and aggregation.
//!
//! Collects metrics from transport layer and maintains
//! sliding window with baseline calculations.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Aggregated telemetry snapshot for decision making
#[derive(Debug, Clone, Copy)]
pub struct Telemetry {
    pub loss_rate: f64,
    pub jitter_ms: f64,
    pub rst_events: u32,
    pub icmp_unreachable: u32,
    pub silence_detected: bool,
    pub traffic_entropy: f64,
    pub path_divergence: f64,
    pub peer_loss_rate: f64,
    pub timestamp: Instant,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            loss_rate: 0.0,
            jitter_ms: 0.0,
            rst_events: 0,
            icmp_unreachable: 0,
            silence_detected: false,
            traffic_entropy: 0.0,
            path_divergence: 0.0,
            peer_loss_rate: 0.0,
            timestamp: Instant::now(),
        }
    }
}

/// Collector that maintains sliding window and baseline
pub struct TelemetryCollector {
    window_secs: u32,
    samples: VecDeque<Telemetry>,
    baseline: Baseline,
}

#[derive(Debug, Clone, Copy)]
pub struct Baseline {
    pub loss_mean: f64,
    pub loss_std: f64,
}

impl TelemetryCollector {
    pub fn new(window_secs: u32) -> Self {
        Self {
            window_secs,
            samples: VecDeque::new(),
            baseline: Baseline {
                loss_mean: 0.0,
                loss_std: 0.0,
            },
        }
    }

    /// Add a new telemetry sample
    pub fn add_sample(&mut self, telemetry: Telemetry) {
        self.samples.push_back(telemetry);
        self.prune_old_samples();
        self.update_baseline();
    }

    /// Remove samples older than window
    fn prune_old_samples(&mut self) {
        let now = Instant::now();
        let cutoff = Duration::from_secs(self.window_secs as u64);

        while let Some(sample) = self.samples.front() {
            if now.duration_since(sample.timestamp) > cutoff {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    /// Calculate baseline statistics from current window
    fn update_baseline(&mut self) {
        if self.samples.is_empty() {
            return;
        }

        let loss_values: Vec<f64> = self.samples.iter().map(|s| s.loss_rate).collect();

        self.baseline.loss_mean = mean(&loss_values);
        self.baseline.loss_std = std_dev(&loss_values, self.baseline.loss_mean);
    }

    /// Get current telemetry (latest sample)
    pub fn current(&self) -> Option<Telemetry> {
        self.samples.back().copied()
    }

    /// Get baseline statistics
    pub fn baseline(&self) -> Baseline {
        self.baseline
    }

    /// Check if current loss is anomalous (> mean + 3*std)
    pub fn is_loss_anomalous(&self) -> bool {
        if let Some(current) = self.current() {
            let threshold = self.baseline.loss_mean + 3.0 * self.baseline.loss_std;
            current.loss_rate > threshold && threshold > 0.01
        } else {
            false
        }
    }

}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn std_dev(values: &[f64], mean_val: f64) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let variance = values.iter()
        .map(|v| (v - mean_val).powi(2))
        .sum::<f64>() / (values.len() - 1) as f64;
    variance.sqrt()
}