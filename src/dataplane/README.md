# Dataplane Module

**Adaptive Transport Layer for Smart P2P Network**

**Status:** ✅ Fully implemented (Transport, QoS, Metrics, Multipath)
**Last Updated:** 2025-12-31

## Overview

The dataplane module provides intelligent, self-adjusting transport capabilities for the YANDI network. It enables the network to adapt to changing conditions, prioritize critical traffic, and use multiple paths simultaneously for resilience.

## Architecture

```
dataplane/
├── mod.rs          # Module exports
├── transport.rs    # Core data transport with health monitoring
├── qos.rs          # Quality of Service (5-level prioritization)
├── metrics.rs      # Real-time transport statistics
└── multipath.rs    # Multipath transport management
```

## Components

### 1. Data Transport (`transport.rs`)

Adaptive transport that can switch between different protocols and strategies based on network conditions.

**Transport Types:**
- `Udp` - Fast, unreliable datagrams
- `Tcp` - Reliable stream transport
- `Quic` - Modern UDP-based transport with TLS
- `Tunnel` - Encapsulated traffic through proxies
- `Obfuscated` - Traffic shaping for bypass

**Features:**
- Health monitoring (loss rate, latency checking)
- Automatic transport degradation detection
- Configurable reliability and ordering
- Per-transport statistics tracking

**Key Methods:**
```rust
let config = TransportConfig {
    transport_type: TransportType::Udp,
    timeout: Duration::from_secs(5),
    max_packet_size: 65536,
    reliable: false,
    ordered: false,
    encrypted: true,
};

let transport = DataTransport::new(remote_addr, config);
transport.update_latency(latency_ms);
transport.record_sent(bytes, packets);
let is_healthy = transport.is_healthy();  // < 5% loss, < 1s latency
```

### 2. Quality of Service (`qos.rs`)

Packet prioritization system with 5 priority levels to ensure critical traffic gets through first.

**Priority Levels:**
1. `Critical` (0) - Control messages, key exchanges
2. `High` (1) - Real-time data (VoIP, video)
3. `Normal` (2) - Regular application data
4. `Low` (3) - Bulk transfers
5. `Background` (4) - Non-essential traffic

**Features:**
- Priority queue for each level
- Configurable maximum queue size
- Packet drop counting when queues full
- Strict priority ordering (higher priority always first)

**Usage:**
```rust
let mut qos = QoSManager::new(1000);  // max 1000 packets per queue

qos.enqueue(QoSPacket {
    priority: PacketPriority::Critical,
    data: vec![0x01, 0x02, 0x03],
    timestamp: current_time(),
})?;

if let Some(packet) = qos.dequeue() {
    // Send highest priority packet
}
```

### 3. Transport Metrics (`metrics.rs`)

Real-time statistics for monitoring transport performance.

**Metrics Tracked:**
- Bytes/packets sent and received
- Packet loss rate
- Latency measurements
- Per-transport snapshots
- Uptime tracking

**Usage:**
```rust
let metrics = DataplaneMetrics {
    transport_stats: vec![],
    active_transports: 3,
    total_bytes: 1024000,
    total_packets: 512,
    uptime: Instant::now(),
};
```

### 4. Multipath Transport (`multipath.rs`)

Send data across multiple paths simultaneously for redundancy and performance.

**Features:**
- Path state tracking (Active, Degraded, Failed)
- Automatic best path selection
- Multiple transport management
- Active path counting

**Usage:**
```rust
let mut manager = MultipathManager::new();

// Add multiple transports
manager.add_transport(path_id1, transport1);
manager.add_transport(path_id2, transport2);

// Select best transport
if let Some(transport) = manager.select_transport() {
    // Use best path
}

// Mark degraded paths
manager.mark_degraded(path_id2);
```

## Design Principles

### 1. Self-Adaptation
- Transports automatically detect degradation
- Paths are monitored and marked failed/unhealthy
- System switches to best available path

### 2. Resilience
- Multipath transport provides redundancy
- Graceful degradation when paths fail
- Packet loss tracking and recovery

### 3. Prioritization
- Critical traffic always processed first
- Configurable queue sizes prevent starvation
- Drop statistics for capacity planning

### 4. Observability
- Real-time metrics for all operations
- Per-transport health checking
- Loss rate and latency monitoring

## Integration with Other Modules

**Netlayer:**
- Provides `DataTransport` for peer communication
- QoS prioritizes netlayer packets
- Metrics feed into observability system

**Connectors:**
- Different connectors use different transport types
- Multipath can combine UDP + TCP + tunnels
- Health monitoring triggers connector switches

**Observability:**
- Transport metrics aggregated into network metrics
- Logging for transport state changes
- Performance tracking for optimization

## Use Cases

### 1. Stealth Operation
```rust
let transport = DataTransport::new(addr, TransportConfig {
    transport_type: TransportType::Obfuscated,
    encrypted: true,
    ..Default::default()
});
```

### 2. Real-Time Communication
```rust
qos.enqueue(QoSPacket {
    priority: PacketPriority::High,
    data: voice_packet,
    timestamp: now(),
})?;
```

### 3. Redundant Connectivity
```rust
// Use all available paths
for (path_id, transport) in &transports {
    multipath.add_transport(*path_id, transport.clone());
}
```

### 4. Adaptive Performance
```rust
if !transport.is_healthy() {
    // Switch to backup transport
    selector.mark_degraded(current_path);
    let new_path = selector.select_best_path()?;
}
```

## Future Enhancements

1. **Forward Error Correction (FEC)** - Recover lost packets without retransmission
2. **Adaptive Bitrate** - Adjust data rate based on network conditions
3. **Congestion Control** - Prevent network overload
4. **Traffic Analysis Resistance** - Constant-rate padding, timing obfuscation
5. **Path Encryption** - Per-path encryption keys for compartmentalization

## Testing

Test transport health detection:
```rust
let mut transport = DataTransport::new(addr, config);
transport.record_loss(100);  // Simulate packet loss
assert!(!transport.is_healthy());  // Should detect degradation
```

Test QoS prioritization:
```rust
qos.enqueue(low_priority_packet)?;
qos.enqueue(critical_packet)?;
assert_eq!(qos.dequeue().unwrap().priority, PacketPriority::Critical);
```

## Performance Considerations

- **Queue Size**: Larger queues = more buffering but higher latency
- **Health Checks**: Too frequent = overhead; too slow = missed failures
- **Multipath**: More paths = redundancy but coordination overhead
- **Metrics**: Memory usage grows with number of transports

## Security Notes

- All transports should be encrypted by default
- Obfuscated transport for traffic analysis resistance
- Don't expose transport health in plain metadata
- Consider side-channel attacks from timing patterns
