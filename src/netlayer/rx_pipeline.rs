//! Bounded local receive queues. Admission never awaits a consumer.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::sync::{mpsc, Mutex};

pub(crate) fn env_limit(name: &str, default: usize, max: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|n| (1..=max).contains(n))
        .unwrap_or(default)
}

#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub items: usize,
    pub bytes: usize,
}
impl Limits {
    pub fn rx() -> Self {
        Self {
            items: env_limit("YANDI_RX_QUEUE_DATAGRAMS", 8192, 1_048_576),
            bytes: env_limit("YANDI_RX_QUEUE_BYTES", 16 * 1024 * 1024, 1024 * 1024 * 1024),
        }
    }
    pub fn tunnel() -> Self {
        Self {
            items: env_limit("YANDI_TUNNEL_QUEUE_ITEMS", 100, 65536),
            bytes: env_limit(
                "YANDI_TUNNEL_QUEUE_BYTES",
                4 * 1024 * 1024,
                256 * 1024 * 1024,
            ),
        }
    }
}

struct Budget {
    bytes: AtomicU64,
    items: AtomicU64,
    item_limit: u64,
    limit: u64,
    depth: &'static AtomicU64,
    total_bytes: &'static AtomicU64,
}
pub(crate) struct Queued<T> {
    pub value: T,
    pub enqueued_at: std::time::Instant,
    size: u64,
    budget: Arc<Budget>,
}
impl<T> Drop for Queued<T> {
    fn drop(&mut self) {
        self.budget.bytes.fetch_sub(self.size, Ordering::Relaxed);
        self.budget.items.fetch_sub(1, Ordering::Relaxed);
        self.budget
            .total_bytes
            .fetch_sub(self.size, Ordering::Relaxed);
        self.budget.depth.fetch_sub(1, Ordering::Relaxed);
    }
}
pub(crate) struct Sender<T> {
    tx: mpsc::Sender<Queued<T>>,
    budget: Arc<Budget>,
}
pub(crate) type Receiver<T> = Arc<Mutex<mpsc::Receiver<Queued<T>>>>;

pub(crate) fn queue<T>(
    limits: Limits,
    depth: &'static AtomicU64,
    bytes: &'static AtomicU64,
) -> (Sender<T>, Receiver<T>) {
    let (tx, rx) = mpsc::channel(limits.items);
    (
        Sender {
            tx,
            budget: Arc::new(Budget {
                bytes: AtomicU64::new(0),
                items: AtomicU64::new(0),
                item_limit: limits.items as u64,
                limit: limits.bytes as u64,
                depth,
                total_bytes: bytes,
            }),
        },
        Arc::new(Mutex::new(rx)),
    )
}
impl<T> Sender<T> {
    /// Drop-new policy: failure preserves all previously admitted items and their FIFO order.
    pub fn try_send(&self, value: T, size: usize) -> Result<(), ()> {
        let size = size as u64;
        self.budget
            .items
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                (used < self.budget.item_limit).then_some(used + 1)
            })
            .map_err(|_| ())?;
        if self
            .budget
            .bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(size)
                    .filter(|next| *next <= self.budget.limit)
            })
            .is_err()
        {
            self.budget.items.fetch_sub(1, Ordering::Relaxed);
            return Err(());
        }
        self.budget.depth.fetch_add(1, Ordering::Relaxed);
        self.budget.total_bytes.fetch_add(size, Ordering::Relaxed);
        self.tx
            .try_send(Queued {
                value,
                enqueued_at: std::time::Instant::now(),
                size,
                budget: self.budget.clone(),
            })
            .map_err(|_| ())
    }
}

pub(crate) struct Datagram {
    pub data: Vec<u8>,
    pub from: std::net::SocketAddr,
    pub sender: crate::util::HashId,
}

/// Exactly one task calls recv; the only consumer interaction is try_send.
/// Generic receive adapter also lets the regression exercise this loop without network I/O.
pub(crate) async fn read_datagrams<F, Fut>(mut recv: F, tx: Sender<Datagram>, rx_bytes: &AtomicU64)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::io::Result<(Vec<u8>, std::net::SocketAddr)>>,
{
    loop {
        let (data, from) = match recv().await {
            Ok(packet) => packet,
            Err(e) => {
                tracing::warn!("[rx] receive error: {e}");
                // не крутить цикл вхолостую, если сокет постоянно отдаёт ошибку
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                continue;
            }
        };
        rx_bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
        // Sender header is untrusted until the worker authenticates the unchanged envelope.
        let Some(header) = data.get(..32) else {
            continue;
        };
        let sender = crate::util::HashId(header.try_into().unwrap());
        let size = data.len();
        if tx.try_send(Datagram { data, from, sender }, size).is_err() {
            super::transport::get_wagon_stats()
                .rx_queue_dropped
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn counters() -> (&'static AtomicU64, &'static AtomicU64) {
        (
            Box::leak(Box::new(AtomicU64::new(0))),
            Box::leak(Box::new(AtomicU64::new(0))),
        )
    }
    #[tokio::test]
    async fn rx_pipeline_drop_new_limits_and_fifo() {
        let (depth, bytes) = counters();
        let (tx, rx) = queue(Limits { items: 2, bytes: 5 }, depth, bytes);
        assert!(tx.try_send(1, 2).is_ok());
        assert!(tx.try_send(99, 4).is_err()); // byte cap with a free item slot
        assert_eq!(depth.load(Ordering::Relaxed), 1);
        assert!(tx.try_send(2, 3).is_ok());
        assert!(tx.try_send(3, 1).is_err()); // bytes
        assert!(tx.try_send(3, 0).is_err()); // slots; byte reservation rolled back
        assert_eq!(bytes.load(Ordering::Relaxed), 5);
        assert_eq!(depth.load(Ordering::Relaxed), 2);
        let first = rx.lock().await.recv().await.unwrap();
        assert_eq!(first.value, 1);
        assert!(tx.try_send(3, 0).is_err()); // item cap includes a dequeued, retained write
        drop(first);
        assert!(tx.try_send(3, 2).is_ok());
        assert_eq!(rx.lock().await.recv().await.unwrap().value, 2);
        assert_eq!(rx.lock().await.recv().await.unwrap().value, 3);
        assert_eq!(bytes.load(Ordering::Relaxed), 0);
        assert_eq!(depth.load(Ordering::Relaxed), 0);
        assert!(tx.try_send(4, 6).is_err()); // one oversized datagram
        drop(rx);
        assert!(tx.try_send(5, 1).is_err()); // closed consumer releases reservation
        assert_eq!(bytes.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn rx_pipeline_workers_consume_each_item_once() {
        for workers in [1, 4] {
            let (depth, bytes) = counters();
            let (tx, rx) = queue(
                Limits {
                    items: 64,
                    bytes: 64,
                },
                depth,
                bytes,
            );
            for n in 0..64 {
                tx.try_send(n, 1).unwrap();
            }
            drop(tx);
            let output = Arc::new(Mutex::new(Vec::new()));
            let mut tasks = Vec::new();
            for _ in 0..workers {
                let rx = rx.clone();
                let output = output.clone();
                tasks.push(tokio::spawn(async move {
                    loop {
                        let item = rx.lock().await.recv().await;
                        let Some(item) = item else { break };
                        output.lock().await.push(item.value);
                        tokio::task::yield_now().await;
                    }
                }));
            }
            for task in tasks {
                task.await.unwrap();
            }
            let mut result = output.lock().await.clone();
            if workers == 1 {
                assert_eq!(result, (0..64).collect::<Vec<_>>());
            }
            result.sort();
            assert_eq!(result, (0..64).collect::<Vec<_>>());
            assert_eq!(depth.load(Ordering::Relaxed), 0);
        }
    }

    #[tokio::test]
    async fn rx_pipeline_reader_keeps_receiving_when_delivery_channel_is_blocked() {
        let (depth, bytes) = counters();
        let (tx, queue_rx) = queue(
            Limits {
                items: 2,
                bytes: 128,
            },
            depth,
            bytes,
        );
        let (delivery, mut blocked_consumer) = mpsc::channel(1);
        delivery.send(()).await.unwrap(); // consumer deliberately does not read
        let (entered, started) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(async move {
            let packet = queue_rx.lock().await.recv().await.unwrap();
            drop(packet);
            entered.send(()).unwrap();
            delivery.send(()).await.unwrap();
            // Keep RX queue alive while blocked on delivery.
            drop(queue_rx);
        });
        let (feed, input) = mpsc::channel(64);
        let input = Arc::new(Mutex::new(input));
        let consumed = Arc::new(AtomicU64::new(0));
        let count = consumed.clone();
        let reader = tokio::spawn(async move {
            read_datagrams(
                || {
                    let input = input.clone();
                    async move { Ok(input.lock().await.recv().await.unwrap()) }
                },
                tx,
                &count,
            )
            .await;
        });
        let addr = "127.0.0.1:1".parse().unwrap();
        feed.send((vec![1; 32], addr)).await.unwrap();
        started.await.unwrap(); // worker now awaits an already full channel
        for _ in 0..32 {
            feed.send((vec![1; 32], addr)).await.unwrap();
        }
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while consumed.load(Ordering::Relaxed) < 33 * 32 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(depth.load(Ordering::Relaxed), 2);
        assert_eq!(bytes.load(Ordering::Relaxed), 64);
        assert_eq!(blocked_consumer.recv().await, Some(()));
        worker.await.unwrap();
        reader.abort();
        let _ = reader.await;
        assert_eq!(depth.load(Ordering::Relaxed), 0);
    }
}
