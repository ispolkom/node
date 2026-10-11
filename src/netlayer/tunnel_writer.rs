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
    pub fn enqueue(&self, data: Vec<u8>, close: bool) -> Result<(), ()> {
        let len = data.len();
        self.tx.try_send(Block { data, close }, len).map_err(|e| {
            super::transport::get_wagon_stats()
                .tunnel_queue_overflow
                .fetch_add(1, Ordering::Relaxed);
            tracing::warn!("[rx] tunnel queue exhausted; closing this tunnel");
            e
        })
    }
}

pub(crate) fn spawn_writer<K, W>(
    writer: W,
    tunnels: Arc<Mutex<HashMap<K, TunnelWriter>>>,
    key: K,
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
    spawn_writer_with_limits(writer, tunnels, key, Limits::tunnel(), timeout)
}

fn spawn_writer_with_limits<K, W>(
    mut writer: W,
    tunnels: Arc<Mutex<HashMap<K, TunnelWriter>>>,
    key: K,
    limits: Limits,
    timeout: Duration,
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
        loop {
            let Some(block) = rx.lock().await.recv().await else {
                break;
            };
            let result = tokio::time::timeout(timeout, async {
                writer.write_all(&block.value.data).await?;
                if block.value.close {
                    writer.shutdown().await?;
                }
                Ok::<_, std::io::Error>(())
            })
            .await;
            if !matches!(result, Ok(Ok(()))) {
                stats.tunnel_write_failed.fetch_add(1, Ordering::Relaxed);
                tracing::warn!("[rx] tunnel write failed or timed out; closing this tunnel");
                break;
            }
            if block.value.close {
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
