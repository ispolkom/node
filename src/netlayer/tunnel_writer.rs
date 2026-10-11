//! One bounded queue and one writer per TCP tunnel. Dropping the handle cancels
//! a blocked write and notifies the paired reader, closing both socket halves.
use super::rx_pipeline::{self, Limits, Sender};
use std::{
    collections::HashMap,
    hash::Hash,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::{watch, Mutex},
    task::AbortHandle,
};

struct Block {
    data: Vec<u8>,
    close: bool,
    offset: Option<u64>,
}
impl super::stream_order::Chunk for rx_pipeline::Queued<Block> {
    fn offset(&self) -> Option<u64> {
        self.value.offset
    }
    fn len(&self) -> usize {
        self.value.data.len()
    }
    fn close(&self) -> bool {
        self.value.close
    }
}
// Gauges are released on normal exit, cancellation and map removal alike.
#[derive(Default)]
struct ReorderGauge {
    items: u64,
    bytes: u64,
}
impl ReorderGauge {
    fn set(&mut self, items: usize, bytes: usize) {
        let stats = super::transport::get_wagon_stats();
        stats
            .tunnel_reorder_depth
            .fetch_sub(self.items, Ordering::Relaxed);
        stats
            .tunnel_reorder_bytes
            .fetch_sub(self.bytes, Ordering::Relaxed);
        self.items = items as u64;
        self.bytes = bytes as u64;
        stats
            .tunnel_reorder_depth
            .fetch_add(self.items, Ordering::Relaxed);
        stats
            .tunnel_reorder_bytes
            .fetch_add(self.bytes, Ordering::Relaxed);
    }
}
impl Drop for ReorderGauge {
    fn drop(&mut self) {
        self.set(0, 0);
    }
}
pub(crate) struct TunnelWriter {
    tx: Sender<Block>,
    abort: AbortHandle,
    closed: watch::Sender<bool>,
}
impl Drop for TunnelWriter {
    fn drop(&mut self) {
        let _ = self.closed.send(true);
        self.abort.abort();
    }
}
impl TunnelWriter {
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.closed.subscribe()
    }
    #[cfg(test)]
    pub fn enqueue(&self, data: Vec<u8>, close: bool) -> Result<(), ()> {
        self.enqueue_offset(data, close, None)
    }
    pub fn enqueue_offset(
        &self,
        data: Vec<u8>,
        close: bool,
        offset: Option<u64>,
    ) -> Result<(), ()> {
        let len = data.len();
        self.tx
            .try_send(
                Block {
                    data,
                    close,
                    offset,
                },
                len,
            )
            .map_err(|e| {
                super::transport::get_wagon_stats()
                    .tunnel_queue_overflow
                    .fetch_add(1, Ordering::Relaxed);
                super::transport::get_wagon_stats()
                    .tunnel_stream_overflow
                    .fetch_add(1, Ordering::Relaxed);
                tracing::warn!("[rx] tunnel queue exhausted; closing this tunnel");
                e
            })
    }
}

#[cfg(test)]
pub(crate) fn spawn_writer<K, W>(
    writer: W,
    tunnels: Arc<Mutex<HashMap<K, TunnelWriter>>>,
    key: K,
) -> TunnelWriter
where
    K: Eq + Hash + Clone + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    spawn_stream_writer(writer, tunnels, key, false)
}

pub(crate) fn spawn_stream_writer<K, W>(
    writer: W,
    tunnels: Arc<Mutex<HashMap<K, TunnelWriter>>>,
    key: K,
    ordered: bool,
) -> TunnelWriter
where
    K: Eq + Hash + Clone + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let timeout = Duration::from_millis(rx_pipeline::env_limit(
        "YANDI_TUNNEL_WRITE_TIMEOUT_MS",
        30_000,
        600_000,
    ) as u64);
    spawn_ordered_with_limits(writer, tunnels, key, Limits::tunnel(), timeout, ordered)
}

#[cfg(test)]
fn spawn_writer_with_limits<K, W>(
    writer: W,
    tunnels: Arc<Mutex<HashMap<K, TunnelWriter>>>,
    key: K,
    limits: Limits,
    timeout: Duration,
) -> TunnelWriter
where
    K: Eq + Hash + Clone + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    spawn_ordered_with_limits(writer, tunnels, key, limits, timeout, false)
}

fn spawn_ordered_with_limits<K, W>(
    mut writer: W,
    tunnels: Arc<Mutex<HashMap<K, TunnelWriter>>>,
    key: K,
    limits: Limits,
    timeout: Duration,
    ordered: bool,
) -> TunnelWriter
where
    K: Eq + Hash + Clone + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let stats = super::transport::get_wagon_stats();
    let (tx, rx) =
        rx_pipeline::queue::<Block>(limits, &stats.tunnel_queue_depth, &stats.tunnel_queue_bytes);
    let (closed, _) = watch::channel(false);
    let notify = closed.clone();
    let tunnels = Arc::downgrade(&tunnels);
    let task = tokio::spawn(async move {
        let mut reorder = super::stream_order::Reorder::configured();
        let mut gauge = ReorderGauge::default();
        loop {
            let incoming = async { rx.lock().await.recv().await };
            let block = if ordered {
                if let Some(expires) = reorder.expires() {
                    tokio::select! {
                        biased;
                        _ = tokio::time::sleep_until(expires) => {
                            stats.tunnel_gap_closed.fetch_add(1, Ordering::Relaxed);
                            tracing::warn!("[rx] stream gap deadline; closing this tunnel");
                            break;
                        }
                        block = incoming => block,
                    }
                } else {
                    incoming.await
                }
            } else {
                incoming.await
            };
            let Some(block) = block else {
                break;
            };
            let mut ready = Vec::new();
            let close;
            if ordered {
                match reorder.insert(block, tokio::time::Instant::now()) {
                    Ok((moved, duplicate)) => {
                        if moved {
                            stats.tunnel_reordered.fetch_add(1, Ordering::Relaxed);
                        }
                        if duplicate {
                            stats
                                .tunnel_stream_duplicates
                                .fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(error) => {
                        match error {
                            super::stream_order::Error::Gap => {
                                stats.tunnel_gap_closed.fetch_add(1, Ordering::Relaxed);
                            }
                            super::stream_order::Error::Overflow => {
                                stats.tunnel_stream_overflow.fetch_add(1, Ordering::Relaxed);
                            }
                            _ => {
                                stats.tunnel_stream_invalid.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        tracing::warn!(
                            ?error,
                            "[rx] stream reassembly failed; closing this tunnel"
                        );
                        break;
                    }
                }
                gauge.set(reorder.size(), reorder.bytes);
                while let Some((block, skip, counted_duplicate)) =
                    reorder.pop(tokio::time::Instant::now())
                {
                    if skip > 0 && !counted_duplicate {
                        stats
                            .tunnel_stream_duplicates
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    ready.push((block, skip));
                }
                gauge.set(reorder.size(), reorder.bytes);
                close = reorder.finished();
            } else {
                // A pinned legacy tunnel must not switch codecs midstream.
                if block.value.offset.is_some() {
                    stats.tunnel_stream_invalid.fetch_add(1, Ordering::Relaxed);
                    break;
                }
                close = block.value.close;
                ready.push((block, 0));
            }
            let write_timeout = reorder
                .expires()
                .map(|t| {
                    t.saturating_duration_since(tokio::time::Instant::now())
                        .min(timeout)
                })
                .unwrap_or(timeout);
            let result = tokio::time::timeout(write_timeout, async {
                for (block, skip) in &ready {
                    writer.write_all(&block.value.data[*skip..]).await?;
                }
                if close {
                    writer.shutdown().await?;
                }
                Ok::<_, std::io::Error>(())
            })
            .await;
            if !matches!(result, Ok(Ok(()))) {
                if ordered && reorder.check(tokio::time::Instant::now()).is_err() {
                    stats.tunnel_gap_closed.fetch_add(1, Ordering::Relaxed);
                } else {
                    stats.tunnel_write_failed.fetch_add(1, Ordering::Relaxed);
                }
                tracing::warn!("[rx] tunnel write failed or timed out; closing this tunnel");
                break;
            }
            if close {
                break;
            }
        }
        let _ = notify.send(true);
        // The map owns our handle. Remove it only after all I/O is finished.
        if let Some(tunnels) = tunnels.upgrade() {
            tunnels.lock().await.remove(&key);
        }
    });
    TunnelWriter {
        tx,
        abort: task.abort_handle(),
        closed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, AsyncReadExt};

    #[tokio::test]
    async fn rx_pipeline_slow_tunnel_does_not_block_other_tunnel_and_overflow_closes_it() {
        let tunnels = Arc::new(Mutex::new(HashMap::new()));
        let (slow, mut slow_peer) = duplex(1);
        let (fast, mut fast_peer) = duplex(64);
        let limits = Limits { items: 2, bytes: 8 };
        let mut closed;
        {
            let mut map = tunnels.lock().await;
            let writer =
                spawn_writer_with_limits(slow, tunnels.clone(), 1, limits, Duration::from_secs(30));
            closed = writer.subscribe();
            map.insert(1, writer);
            map.insert(
                2,
                spawn_writer_with_limits(fast, tunnels.clone(), 2, limits, Duration::from_secs(30)),
            );
            map[&1].enqueue(vec![7; 8], false).unwrap();
            map[&2].enqueue(vec![1, 2], false).unwrap();
            map[&2].enqueue(vec![3, 4], true).unwrap();
        }
        let mut received = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), fast_peer.read_to_end(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, [1, 2, 3, 4]); // FIFO incl. close after queued data
        {
            let mut map = tunnels.lock().await;
            // Budget includes the pending TCP write, even after it left mpsc.
            assert!(map[&1].enqueue(vec![8], false).is_err());
            map.remove(&1);
        }
        tokio::time::timeout(Duration::from_secs(1), closed.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(*closed.borrow());
        tokio::time::timeout(
            Duration::from_secs(1),
            slow_peer.read_to_end(&mut Vec::new()),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(tunnels.lock().await.is_empty());
    }

    #[tokio::test]
    async fn rx_pipeline_writer_failure_removes_only_failed_tunnel() {
        let tunnels = Arc::new(Mutex::new(HashMap::new()));
        let (failed, peer) = duplex(1);
        drop(peer);
        let (other, _peer) = duplex(1);
        let mut closed;
        {
            let mut map = tunnels.lock().await;
            let writer = spawn_writer(failed, tunnels.clone(), 1);
            closed = writer.subscribe();
            map.insert(1, writer);
            map.insert(2, spawn_writer(other, tunnels.clone(), 2));
            map[&1].enqueue(vec![1], false).unwrap();
        }
        tokio::time::timeout(Duration::from_secs(1), closed.changed())
            .await
            .unwrap()
            .unwrap();
        let map = tunnels.lock().await;
        assert!(!map.contains_key(&1));
        assert!(map.contains_key(&2));
    }
    #[tokio::test]
    async fn rx_pipeline_write_timeout_releases_socket_and_queue() {
        let tunnels = Arc::new(Mutex::new(HashMap::new()));
        let (writer, mut peer) = duplex(1);
        let mut closed;
        {
            let mut map = tunnels.lock().await;
            let writer = spawn_writer_with_limits(
                writer,
                tunnels.clone(),
                1,
                Limits { items: 2, bytes: 8 },
                Duration::from_millis(10),
            );
            closed = writer.subscribe();
            map.insert(1, writer);
            map[&1].enqueue(vec![7; 8], false).unwrap();
        }
        tokio::time::timeout(Duration::from_secs(1), closed.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(*closed.borrow());
        assert!(tunnels.lock().await.is_empty());
        // Pending bytes were abandoned on timeout; only the byte written before the stall remains.
        let mut received = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), peer.read_to_end(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, [7]);
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;
    use crate::{proxy::ProxyTunnelData, socks5::Socks5TunnelData};
    use tokio::{
        io::AsyncReadExt,
        net::{TcpListener, TcpStream},
    };

    async fn reversed_tcp(http: bool) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut peer = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (socket, _) = listener.accept().await.unwrap();
        let tunnels = Arc::new(Mutex::new(HashMap::new()));
        let mut closed;
        {
            let mut map = tunnels.lock().await;
            let writer = spawn_stream_writer(socket, tunnels.clone(), 1, true);
            closed = writer.subscribe();
            map.insert(1, writer);
            // Concurrent completion workers may enqueue any permutation, including close first.
            for (offset, data, close) in [
                (6, &b""[..], true),
                (4, &b"ef"[..], false),
                (2, &b"cd"[..], false),
                (0, &b"ab"[..], false),
            ] {
                let (data, close, offset) = if http {
                    let d = ProxyTunnelData {
                        tunnel_id: 1,
                        data: data.to_vec(),
                        close,
                        byte_offset: Some(offset),
                    };
                    let parsed = ProxyTunnelData::from_bincode(&d.to_bincode().unwrap()).unwrap();
                    (parsed.data, parsed.close, parsed.byte_offset)
                } else {
                    let mut d = Socks5TunnelData::new(1, data.to_vec());
                    d.close = close;
                    d.byte_offset = Some(offset);
                    let parsed = Socks5TunnelData::parse(&d.encode()).unwrap();
                    (parsed.data, parsed.close, parsed.byte_offset)
                };
                map[&1].enqueue_offset(data, close, offset).unwrap();
            }
        }
        let mut received = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), peer.read_to_end(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, b"abcdef");
        if !*closed.borrow() {
            closed.changed().await.unwrap();
        }
        assert!(tunnels.lock().await.is_empty());
    }
    #[tokio::test]
    async fn socks5_reverse_blocks_to_tcp_in_order() {
        reversed_tcp(false).await;
    }
    #[tokio::test]
    async fn http_connect_reverse_blocks_to_tcp_in_order() {
        reversed_tcp(true).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn four_completion_workers_preserve_tcp_bytes_for_both_codecs() {
        for http in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let mut peer = TcpStream::connect(listener.local_addr().unwrap())
                .await
                .unwrap();
            let (socket, _) = listener.accept().await.unwrap();
            let tunnels = Arc::new(Mutex::new(HashMap::new()));
            {
                let mut map = tunnels.lock().await;
                map.insert(1, spawn_stream_writer(socket, tunnels.clone(), 1, true));
                map[&1].enqueue_offset(vec![], true, Some(64)).unwrap();
            }
            let barrier = Arc::new(tokio::sync::Barrier::new(4));
            let mut workers = Vec::new();
            for worker in 0..4u64 {
                let tunnels = tunnels.clone();
                let barrier = barrier.clone();
                workers.push(tokio::spawn(async move {
                    barrier.wait().await;
                    for group in (0..16u64).rev() {
                        let offset = group * 4 + worker;
                        let (data, offset) = if http {
                            let block = ProxyTunnelData {
                                tunnel_id: 1,
                                data: vec![offset as u8],
                                close: false,
                                byte_offset: Some(offset),
                            };
                            let parsed =
                                ProxyTunnelData::from_bincode(&block.to_bincode().unwrap())
                                    .unwrap();
                            (parsed.data, parsed.byte_offset)
                        } else {
                            let mut block = Socks5TunnelData::new(1, vec![offset as u8]);
                            block.byte_offset = Some(offset);
                            let parsed = Socks5TunnelData::parse(&block.encode()).unwrap();
                            (parsed.data, parsed.byte_offset)
                        };
                        tunnels.lock().await[&1]
                            .enqueue_offset(data, false, offset)
                            .unwrap();
                        tokio::task::yield_now().await;
                    }
                }));
            }
            for worker in workers {
                worker.await.unwrap();
            }
            let mut received = Vec::new();
            tokio::time::timeout(Duration::from_secs(2), peer.read_to_end(&mut received))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(received, (0..64u8).collect::<Vec<_>>());
        }
    }

    #[tokio::test]
    async fn gap_timer_closes_without_joining_suffix_and_releases_budget() {
        let tunnels = Arc::new(Mutex::new(HashMap::new()));
        let (socket, mut peer) = tokio::io::duplex(64);
        let stats = super::super::transport::get_wagon_stats();
        let before = stats.tunnel_gap_closed.load(Ordering::Relaxed);
        let bytes_before = stats.tunnel_queue_bytes.load(Ordering::Relaxed);
        let mut closed;
        {
            let mut map = tunnels.lock().await;
            let writer = spawn_stream_writer(socket, tunnels.clone(), 1, true);
            closed = writer.subscribe();
            map.insert(1, writer);
            map[&1]
                .enqueue_offset(b"suffix".to_vec(), false, Some(3))
                .unwrap();
            map[&1].enqueue_offset(vec![], true, Some(9)).unwrap();
        }
        tokio::time::timeout(Duration::from_secs(7), closed.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(tunnels.lock().await.is_empty());
        let mut received = Vec::new();
        peer.read_to_end(&mut received).await.unwrap();
        assert!(received.is_empty());
        assert_eq!(stats.tunnel_gap_closed.load(Ordering::Relaxed), before + 1);
        assert_eq!(
            stats.tunnel_queue_bytes.load(Ordering::Relaxed),
            bytes_before
        );
        assert_eq!(stats.tunnel_reorder_depth.load(Ordering::Relaxed), 0);
        assert_eq!(stats.tunnel_reorder_bytes.load(Ordering::Relaxed), 0);
    }
    #[tokio::test]
    async fn pinned_mode_rejects_mixed_forms() {
        for ordered in [true, false] {
            let tunnels = Arc::new(Mutex::new(HashMap::new()));
            let (socket, mut peer) = tokio::io::duplex(64);
            let mut closed;
            {
                let mut map = tunnels.lock().await;
                let writer = spawn_stream_writer(socket, tunnels.clone(), 1, ordered);
                closed = writer.subscribe();
                map.insert(1, writer);
                map[&1]
                    .enqueue_offset(vec![7], false, if ordered { None } else { Some(0) })
                    .unwrap();
            }
            tokio::time::timeout(Duration::from_secs(1), closed.changed())
                .await
                .unwrap()
                .unwrap();
            let mut received = Vec::new();
            peer.read_to_end(&mut received).await.unwrap();
            assert!(received.is_empty());
        }
    }
}
