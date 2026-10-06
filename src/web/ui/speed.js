// === Speed Monitor ===
let speedInterval = null;

function formatSpeed(mbps, kbps) {
    if (mbps >= 1) {
        return `${mbps.toFixed(2)} MB/s`;
    }
    return `${kbps.toFixed(1)} KB/s`;
}

function formatBytes(bytes) {
    if (bytes >= 1024 * 1024) {
        return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
    }
    if (bytes >= 1024) {
        return `${(bytes / 1024).toFixed(1)} KB`;
    }
    return `${bytes} B`;
}

async function loadSpeed() {
    try {
        const response = await fetch('/api/speed');
        const data = await response.json();
        
        // Обновляем входящую скорость
        const rxSpeedEl = document.getElementById('rxSpeed');
        if (rxSpeedEl) {
            rxSpeedEl.textContent = formatSpeed(data.rx_speed_mbps, data.rx_speed_kbps);
        }
        
        // Обновляем исходящую скорость.
        // Это локальный TX/peer RX estimate, а не "сеть сколько теоретически может".
        const txSpeedEl = document.getElementById('txSpeed');
        if (txSpeedEl) {
            txSpeedEl.textContent = formatSpeed(data.tx_speed_mbps, data.tx_speed_kbps);
        }
        
        // Обновляем receive-side Path0 loss. Это не "эффективная потеря доставки",
        // а debug-метрика приёма текущей ноды.
        const lossIncomingEl = document.getElementById('lossIncoming');
        if (lossIncomingEl && data.path0_loss_incoming !== undefined) {
            lossIncomingEl.textContent = `Path0 потери: ${data.path0_loss_incoming.toFixed(2)}%`;
        }

        const cloneHitsEl = document.getElementById('cloneHits');
        if (cloneHitsEl) {
            cloneHitsEl.textContent = `Clone hits: ${(data.clone_hit_pct ?? 0).toFixed(2)}%`;
        }

        const pingValueEl = document.getElementById('pingValue');
        if (pingValueEl) {
            if ((data.avg_rtt_ms ?? 0) > 0) {
                pingValueEl.textContent = `${data.avg_rtt_ms} ms`;
            } else {
                pingValueEl.textContent = '-- ms';
            }
        }
        
        // Для исходящей стороны показываем то, что peer оценивает как приём нашего трафика.
        const lossOutgoingEl = document.getElementById('lossOutgoing');
        if (lossOutgoingEl) {
            if (data.peer_rx_estimate_mbps !== undefined && data.peer_rx_estimate_kbps !== undefined) {
                lossOutgoingEl.textContent = `Peer RX: ${formatSpeed(data.peer_rx_estimate_mbps, data.peer_rx_estimate_kbps)}`;
            } else {
                lossOutgoingEl.textContent = 'Peer RX: --';
            }
        }

        const peerPath0LossEl = document.getElementById('peerPath0Loss');
        if (peerPath0LossEl) {
            peerPath0LossEl.textContent = `Peer Path0: ${(data.peer_path0_loss ?? 0).toFixed(2)}%`;
        }

        const wagonsRateEl = document.getElementById('wagonsRate');
        if (wagonsRateEl) {
            wagonsRateEl.textContent = `Wagons/s: ${(data.wagons_per_sec ?? 0).toFixed(1)}`;
        }

        const peerCountEl = document.getElementById('peerCount');
        if (peerCountEl) {
            peerCountEl.textContent = `Peers: ${data.peer_count ?? 0}`;
        }

        const dropCrcEl = document.getElementById('dropCrc');
        if (dropCrcEl) {
            dropCrcEl.textContent = `Drop+CRC: ${(data.wagon_drop_crc_pct ?? 0).toFixed(2)}%`;
        }

        const depotUsageEl = document.getElementById('depotUsage');
        if (depotUsageEl) {
            depotUsageEl.textContent = `Depot: ${formatBytes(data.depot_bytes ?? 0)}`;
        }

        const activeTrainsEl = document.getElementById('activeTrains');
        if (activeTrainsEl) {
            activeTrainsEl.textContent = `Active trains: ${data.active_trains ?? 0}`;
        }

        const evictionsEl = document.getElementById('evictions');
        if (evictionsEl) {
            evictionsEl.textContent = `Evict: +${data.evictions_delta ?? 0} / ${data.evictions_total ?? 0}`;
        }

        const wagonHealthEl = document.getElementById('wagonHealth');
        if (wagonHealthEl) {
            wagonHealthEl.textContent =
                `Retrans: ${data.wagon_retrans_total ?? 0} | RX: ${data.wagon_recv_total ?? 0}`;
        }
        
        // Raw данные для отладки
        console.log(
            `📊 Speed: RX=${data.rx_speed.toFixed(0)} B/s, TX=${data.tx_speed.toFixed(0)} B/s, ` +
            `Path0In=${data.path0_loss_incoming?.toFixed(2)}%, PeerPath0=${data.peer_path0_loss?.toFixed(2)}%, ` +
            `CloneHits=${data.clone_hit_pct?.toFixed(2)}%, Depot=${data.depot_bytes ?? 0}B, RTT=${data.avg_rtt_ms ?? 0}ms`
        );
    } catch (error) {
        console.error('Failed to load speed:', error);
    }
}

function startSpeedMonitor() {
    if (speedInterval) clearInterval(speedInterval);
    loadSpeed();
    speedInterval = setInterval(loadSpeed, 2000);
}

function stopSpeedMonitor() {
    if (speedInterval) {
        clearInterval(speedInterval);
        speedInterval = null;
    }
}

// Автозапуск
if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', startSpeedMonitor);
} else {
    startSpeedMonitor();
}
