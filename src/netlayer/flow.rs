//! Регулятор скорости по готовности получателя (флаг-кредит по смещению потока).
//!
//! Получатель сообщает: «в TCP выдано `consumed` байт, готов принять ещё `window`». Отправитель не читает следующий блок из своего
//! TCP-источника, пока его позиция не укладывается в `consumed + window`. Это не подтверждение доставки и не повтор: потерянное
//! окно перекрывается следующим (значения монотонны), а вечное ожидание ограничено таймером.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use tokio::sync::Notify;

use crate::util::HashId;

/// Сколько отправитель может послать до первого окна.
pub(crate) const INITIAL_WINDOW: u64 = 512 * 1024;
/// Окно, которое объявляет получатель (его очередь вмещает больше).
pub(crate) const ADVERTISED_WINDOW: u32 = 1024 * 1024;

/// Сколько ждать самого первого окна, прежде чем счесть, что получатель их не присылает (пир без регулятора, обходной маршрут).
pub(crate) const FIRST_UPDATE_GRACE: Duration = Duration::from_secs(1);

pub(crate) struct Window {
    limit: AtomicU64,
    notify: Notify,
    /// хоть одно окно приходило
    seen: AtomicBool,
    /// окон нет: регулятор для этого туннеля отключён (поведение как раньше)
    disabled: AtomicBool,
}

impl Window {
    pub fn new(initial: u64) -> Arc<Self> {
        Arc::new(Self { limit: AtomicU64::new(initial), notify: Notify::new(), seen: AtomicBool::new(false), disabled: AtomicBool::new(false) })
    }

    /// Значения монотонны: старое или повторное окно предел не уменьшает.
    pub fn update(&self, consumed: u64, window: u32) {
        self.seen.store(true, Ordering::Release);
        let new = consumed.saturating_add(window as u64);
        let mut cur = self.limit.load(Ordering::Acquire);
        while new > cur {
            match self.limit.compare_exchange_weak(cur, new, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => {
                    self.notify.notify_waiters();
                    break;
                }
                Err(c) => cur = c,
            }
        }
    }

    pub fn limit(&self) -> u64 {
        self.limit.load(Ordering::Acquire)
    }

    /// Дождаться, пока позиция `end` уложится в предел. `Err(())` — окна нет дольше `stall` (поток закрывают с ошибкой).
    /// Если ни одного окна так и не пришло за `FIRST_UPDATE_GRACE`, регулятор для туннеля отключается (получатель их не шлёт).
    pub async fn wait_until(&self, end: u64, stall: Duration) -> Result<(), ()> {
        self.wait_until_with(end, stall, FIRST_UPDATE_GRACE).await
    }

    async fn wait_until_with(&self, end: u64, stall: Duration, grace: Duration) -> Result<(), ()> {
        let start = tokio::time::Instant::now();
        let deadline = start + stall;
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            // зарегистрироваться ДО проверки: обновление между проверкой и ожиданием не потеряется
            notified.as_mut().enable();
            if end <= self.limit() || self.disabled.load(Ordering::Acquire) {
                return Ok(());
            }
            let seen = self.seen.load(Ordering::Acquire);
            let until = if seen { deadline } else { (start + grace).min(deadline) };
            if tokio::time::timeout_at(until, notified).await.is_err() {
                if end <= self.limit() {
                    return Ok(());
                }
                if !self.seen.load(Ordering::Acquire) {
                    // окон не было вовсе: работаем без регулятора
                    self.disabled.store(true, Ordering::Release);
                    return Ok(());
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(());
                }
            }
        }
    }
}

/// Замыкание для писателя туннеля: отправляет окно получателя «выдано `consumed`» отправителю потока.
pub(crate) fn credit_sender(
    station: Arc<crate::protocol::Station>,
    kind: u8,
    peer: HashId,
    tunnel_id: u64,
) -> super::tunnel_writer::Credit {
    Arc::new(move |consumed| {
        let station = station.clone();
        let msg = super::tunnel_wire::encode_window(kind, tunnel_id, consumed, ADVERTISED_WINDOW);
        // окно маленькое и идемпотентное: потеря перекрывается следующим
        tokio::spawn(async move {
            let _ = station.send_train(peer, msg).await;
        });
    })
}

/// Таймер защиты от «вечного нуля» (мс): `YANDI_TUNNEL_STALL_MS`.
pub(crate) fn stall_timeout() -> Duration {
    Duration::from_millis(super::rx_pipeline::env_limit("YANDI_TUNNEL_STALL_MS", 30_000, 600_000) as u64)
}

type Key = (u8, HashId, u64);

fn registry() -> &'static Mutex<HashMap<Key, Weak<Window>>> {
    static R: OnceLock<Mutex<HashMap<Key, Weak<Window>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Отправляющая сторона туннеля регистрирует окно; при падении `Registered` запись убирается.
pub(crate) struct Registered {
    pub window: Arc<Window>,
    key: Key,
}

impl Drop for Registered {
    fn drop(&mut self) {
        registry().lock().unwrap_or_else(|e| e.into_inner()).remove(&self.key);
    }
}

pub(crate) fn register(kind: u8, peer: HashId, tunnel_id: u64) -> Registered {
    let window = Window::new(INITIAL_WINDOW);
    let key = (kind, peer, tunnel_id);
    registry().lock().unwrap_or_else(|e| e.into_inner()).insert(key, Arc::downgrade(&window));
    Registered { window, key }
}

/// Окно от получателя: применяется только к туннелю, который мы сами зарегистрировали с этим пиром.
pub(crate) fn apply(kind: u8, peer: HashId, tunnel_id: u64, consumed: u64, window: u32) {
    let w = registry().lock().unwrap_or_else(|e| e.into_inner()).get(&(kind, peer, tunnel_id)).and_then(Weak::upgrade);
    if let Some(w) = w {
        w.update(consumed, window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sender_waits_for_credit_and_is_released_by_update() {
        let w = Window::new(100);
        assert!(w.wait_until(100, Duration::from_millis(50)).await.is_ok());
        // окно уже приходило, а нового нет: ждать вечно нельзя
        w.update(100, 0);
        assert!(w.wait_until(101, Duration::from_millis(30)).await.is_err(), "без окна ждать нельзя вечно");
        let w2 = w.clone();
        let waiter = tokio::spawn(async move { w2.wait_until(5_000, Duration::from_secs(2)).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        w.update(4_000, 1_000);
        assert!(waiter.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn without_any_window_the_regulator_switches_off_after_the_grace() {
        let w = Window::new(10);
        let t = tokio::time::Instant::now();
        assert!(w.wait_until_with(1_000_000, Duration::from_secs(5), Duration::from_millis(60)).await.is_ok());
        assert!(t.elapsed() < Duration::from_secs(1), "отключается по grace, а не по stall");
        // дальше без ожидания
        let t = tokio::time::Instant::now();
        assert!(w.wait_until_with(9_000_000, Duration::from_secs(5), Duration::from_millis(60)).await.is_ok());
        assert!(t.elapsed() < Duration::from_millis(30));
    }

    #[test]
    fn window_is_monotonic_and_scoped_to_registered_tunnel() {
        let w = Window::new(10);
        w.update(1_000, 1_000);
        w.update(10, 10); // старое/повторное окно
        assert_eq!(w.limit(), 2_000);
        w.update(u64::MAX, u32::MAX); // насыщение без переполнения
        assert_eq!(w.limit(), u64::MAX);

        let peer = HashId([7; 32]);
        let other = HashId([8; 32]);
        let reg = register(1, peer, 42);
        apply(1, other, 42, 1_000_000, 1_000_000); // чужой пир
        apply(2, peer, 42, 1_000_000, 1_000_000); // другой вид
        apply(1, peer, 43, 1_000_000, 1_000_000); // другой туннель
        assert_eq!(reg.window.limit(), INITIAL_WINDOW);
        apply(1, peer, 42, 1_000, 2_000);
        assert_eq!(reg.window.limit(), INITIAL_WINDOW.max(3_000));
        drop(reg);
        apply(1, peer, 42, 5, 5); // запись убрана — тишина, без паники
    }
}
