
## States

| State | Description |
|-------|-------------|
| Normal | Default balanced mode |
| DualEnabled | Redundancy active (DUAL-PATH) |
| Stealth | Maximum obfuscation (padding + jitter + rotation) |
| Turbo | Low-latency optimized |

## Transitions

- Normal → DualEnabled: loss > 1%
- Normal → Stealth: DPI suspected
- Normal → Turbo: RTT < 30ms && loss == 0
- DualEnabled → Normal: loss < 0.5%
- DualEnabled → Stealth: DPI suspected
- Stealth → Normal: after 10 minutes
- Turbo → Normal: loss > 0.1% or RTT > 60ms

## Integration

In `transport.rs`:

```rust
use state_manager::{
    StateManager, StateManagerConfig,
    TelemetryCollector, Telemetry, PolicyEngine, Action
};

// Initialize
let config = StateManagerConfig::default();
let mut state_manager = StateManager::new(config);
let mut telemetry_collector = TelemetryCollector::new(config.telemetry_window_secs);

// Update loop (every 1-2 seconds)
fn update_control_plane(&mut self) {
    let telemetry = self.collect_telemetry();
    telemetry_collector.add_sample(telemetry);
    if let Some(new_state) = state_manager.update(telemetry) {
        let actions = PolicyEngine::get_actions(new_state);
        self.apply_actions(actions);
    }
}