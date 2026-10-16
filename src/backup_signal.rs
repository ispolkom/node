//! Сигнал «изменилось то, без чего восстановление опасно» (устройство добавлено или отозвано, участник забанен, сменился владелец
//! группы). Плановая задача резервных копий ([`crate::backup_api`], только Unix) забирает его и делает копию сразу, не дожидаясь суток.
//! Модуль есть на всех платформах, чтобы места вызова не зависели от системы.
use std::sync::atomic::{AtomicBool, Ordering};

static CRITICAL: AtomicBool = AtomicBool::new(false);

pub fn notify_critical() {
    CRITICAL.store(true, Ordering::Relaxed);
}

/// Забрать сигнал (и сбросить).
pub fn take() -> bool {
    CRITICAL.swap(false, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_signal_is_seen_once() {
        super::notify_critical();
        assert!(super::take());
        assert!(!super::take());
    }
}
