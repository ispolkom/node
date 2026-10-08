// src/protocol/rate_controller.rs
//!
//! # Rate Controller (Контроллер скорости)
//!
//! Адаптивный контроль скорости передачи на основе потерь пакетов.
//! Упрощённый TCP-style алгоритм: увеличиваем скорость при низких потерях,
//! снижаем при высоких потерях.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Контроллер скорости передачи
pub struct RateController {
    /// Текущая скорость (Mbps)
    current_rate_mbps: AtomicU64,

    /// Пределы скорости
    min_rate_mbps: u64,
    max_rate_mbps: u64,

    /// Пороги потерь (%)
    loss_threshold_green: u64,  // < 10% - можно увеличивать
    loss_threshold_red: u64,    // > 15% - нужно снижать

    /// Последнее изменение скорости
    last_adjustment_time: Arc<Mutex<Instant>>,

    /// Окно измерения потерь
    pub measurement_window: Arc<Mutex<LossMeasurement>>,

    /// Интервал пробования (сек)
    probe_interval_secs: u64,

    /// Шаги изменения скорости
    step_up_mbps: u64,
    step_down_mbps: u64,
}

/// Окно измерения потерь
pub struct LossMeasurement {
    pub sent_last_period: u64,
    pub path0_lost_last_period: u64,
    pub window_start: Instant,
}

/// Действие по корректировке скорости
#[derive(Debug, PartialEq, Clone)]
pub enum RateAction {
    /// Увеличить скорость на N Mbps
    Increase(u64),
    /// Удерживать текущую скорость
    Maintain,
    /// Снизить скорость на N Mbps
    Decrease(u64),
}

impl RateController {
    /// Создать новый контроллер скорости (backup-эталон: высокий старт, редкий probe).
    pub fn new() -> Self {
        Self {
            current_rate_mbps: AtomicU64::new(1000),  // start at 1 Gbps (backup)
            min_rate_mbps: 100,
            max_rate_mbps: 10000,                      // up to 10 Gbps (backup)
            loss_threshold_green: 10,
            loss_threshold_red: 15,
            last_adjustment_time: Arc::new(Mutex::new(Instant::now())),
            measurement_window: Arc::new(Mutex::new(LossMeasurement {
                sent_last_period: 0,
                path0_lost_last_period: 0,
                window_start: Instant::now(),
            })),
            probe_interval_secs: 600,                  // 10 мин — пассивный мониторинг (backup)
            step_up_mbps: 5,
            step_down_mbps: 10,
        }
    }

    /// Создать контроллер с кастомными параметрами
    pub fn with_config(
        min_rate_mbps: u64,
        max_rate_mbps: u64,
        loss_threshold_green: u64,
        loss_threshold_red: u64,
    ) -> Self {
        Self {
            current_rate_mbps: AtomicU64::new(min_rate_mbps),
            min_rate_mbps,
            max_rate_mbps,
            loss_threshold_green,
            loss_threshold_red,
            last_adjustment_time: Arc::new(Mutex::new(Instant::now())),
            measurement_window: Arc::new(Mutex::new(LossMeasurement {
                sent_last_period: 0,
                path0_lost_last_period: 0,
                window_start: Instant::now(),
            })),
            probe_interval_secs: 600,
            step_up_mbps: 5,
            step_down_mbps: 10,
        }
    }

    /// Главная логика: принять решение на основе потерь
    /// Главная логика: принять решение на основе потерь Path0 (истина!)
    pub async fn adjust_rate(&self, sent_path0: u64, path0_lost: u64) -> RateAction {
        let current = self.current_rate_mbps.load(Ordering::Relaxed);

        // Вычисляем % потерь Path0 (только истина!)
        let loss_rate = if sent_path0 > 0 {
            let sent_delta = sent_path0.saturating_sub(self.measurement_window.lock().await.sent_last_period);
            let lost_delta = path0_lost.saturating_sub(self.measurement_window.lock().await.path0_lost_last_period);

            // % потерь Path0
            if sent_delta > 0 {
                lost_delta * 100 / sent_delta
            } else {
                0
            }
        } else {
            0
        };

        // Сохраняем текущие значения для следующего периода
        {
            let mut window = self.measurement_window.lock().await;
            window.sent_last_period = sent_path0;
            window.path0_lost_last_period = path0_lost;
        }

        // Определяем зону потерь и действие
        let action = if loss_rate > self.loss_threshold_red {
            // 🔴 КРАСНАЯ ЗОНА: > 15% потерь - снижаем агрессивно
            let decrease = self.step_down_mbps.min(current - self.min_rate_mbps);
            if decrease > 0 {
                RateAction::Decrease(decrease)
            } else {
                RateAction::Maintain
            }

        } else if loss_rate >= self.loss_threshold_green {
            // 🟡 ЖЁЛТАЯ ЗОНА: 10-15% - идеальная зона, держим
            RateAction::Maintain

        } else {
            // 🟢 ЗЕЛЁНАЯ ЗОНА: < 10% - можно увеличивать
            // Проверяем: прошло ли достаточно времени с последнего изменения?
            let last_change = *self.last_adjustment_time.lock().await;
            let time_since_change = last_change.elapsed();

            if time_since_change > Duration::from_secs(self.probe_interval_secs) {
                // Можно пробовать увеличить
                let increase = self.step_up_mbps.min(self.max_rate_mbps - current);
                if increase > 0 {
                    RateAction::Increase(increase)
                } else {
                    RateAction::Maintain  // Достигли потолка
                }
            } else {
                // Еще не прошло время пробования, держим
                RateAction::Maintain
            }
        };

        // Применяем изменение
        match &action {
            RateAction::Increase(delta) => {
                self.current_rate_mbps.fetch_add(*delta, Ordering::Relaxed);
                *self.last_adjustment_time.lock().await = Instant::now();
            },
            RateAction::Decrease(delta) => {
                self.current_rate_mbps.fetch_sub(*delta, Ordering::Relaxed);
                *self.last_adjustment_time.lock().await = Instant::now();
            },
            RateAction::Maintain => {},
        }

        action
    }

    /// Вычислить задержку между wagon'ами для текущей скорости
    pub fn wagon_delay_for_rate(&self, wagon_size_bytes: usize) -> Duration {
        let rate_mbps = self.current_rate_mbps.load(Ordering::Relaxed);

        if rate_mbps == 0 {
            return Duration::from_millis(1);  // Fallback
        }

        // Вычисляем задержку:
        // rate_mbps * 1_000_000 = bits/sec
        // wagon_size_bytes * 8 = bits
        // delay_us = (wagon_bits * 1_000_000) / (rate_mbps * 1_000_000)
        //           = (wagon_size_bytes * 8) / rate_mbps
        let delay_us = (wagon_size_bytes as u64 * 8) / rate_mbps;

        // Минимальная задержка 92 мкс, максимальная 10 мс
        Duration::from_micros(delay_us.clamp(92, 10_000))
    }

    /// Получить текущую скорость
    pub fn current_rate_mbps(&self) -> u64 {
        self.current_rate_mbps.load(Ordering::Relaxed)
    }

    /// Получить конкурентность для текущей скорости
    pub fn concurrent_wagons_for_rate(&self) -> usize {
        let rate = self.current_rate_mbps.load(Ordering::Relaxed);
        // Примерно 5 Mbps на один concurrent wagon
        // Минимум 2, максимум 64
        ((rate / 5).max(2) as usize).min(64)
    }

    /// Получить статус потерь (для логирования)
    pub fn format_loss_status(loss_rate: u64) -> &'static str {
        if loss_rate < 10 {
            "🟢 GREEN (increase)"
        } else if loss_rate <= 15 {
            "🟡 YELLOW (maintain)"
        } else {
            "🔴 RED (decrease)"
        }
    }

    /// Сколько клонов слать на каждый оригинальный вагон.
    ///
    /// **Backup-эталон**: всегда 1 клон (path1). Транспорт построен на pure FEC через
    /// repetition coding — избыточность не должна зависеть от наблюдаемого loss'а,
    /// иначе исчезает анти-burst-loss свойство и теряется устойчивость на чистых
    /// каналах. Параметры sent_cum/lost_cum принимаются для backward-compat сигнатуры,
    /// но не используются.
    pub async fn clone_count(&self, _sent_cum: u64, _lost_cum: u64) -> u8 {
        1
    }
}

impl Default for RateController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_controller_creation() {
        // Backup-эталон: старт 1 Gbps.
        let rc = RateController::new();
        assert_eq!(rc.current_rate_mbps(), 1000);
    }

    #[test]
    fn test_wagon_delay_calculation() {
        let rc = RateController::new();
        // 1000 Mbps, 1200 bytes = 9600 bits → 9.6us, clamp min 92us.
        let delay = rc.wagon_delay_for_rate(1200);
        assert!(delay.as_micros() >= 92);
        assert!(delay.as_micros() <= 10_000);
    }

    #[test]
    fn test_concurrent_wagons() {
        let rc = RateController::new();
        let concurrent = rc.concurrent_wagons_for_rate();
        assert!(concurrent >= 2);
        assert!(concurrent <= 64);
    }

    #[test]
    fn test_rate_limits() {
        let rc = RateController::with_config(50, 1000, 10, 15);
        assert_eq!(rc.current_rate_mbps(), 50);
        assert_eq!(rc.min_rate_mbps, 50);
        assert_eq!(rc.max_rate_mbps, 1000);
    }
}


/// Real pacing for the sender of a station, open loop (no feedback): every wagon copy that goes to the wire reserves its place in a stream of
/// at most `rate` bits per second, shared by everything the station sends, and waits only if its place is still in the future. It replaces a
/// pause of at least a millisecond per wagon (the timer's resolution), which capped the speed by accident. A short burst (a couple of
/// milliseconds' worth) is allowed so that small trains do not wait at all.
pub struct Pacer {
    /// bits per second
    rate_bps: AtomicU64,
    next: std::sync::Mutex<Option<Instant>>,
}

/// how far behind real time the stream may be (unused credit that can be spent at once)
const PACER_BURST: Duration = Duration::from_millis(2);

impl Pacer {
    pub fn new(rate_mbps: u64) -> Self {
        Self { rate_bps: AtomicU64::new(rate_mbps.max(1) * 1_000_000), next: std::sync::Mutex::new(None) }
    }

    pub fn rate_mbps(&self) -> u64 {
        self.rate_bps.load(Ordering::Relaxed) / 1_000_000
    }

    /// The moment when `bytes` may go to the wire (not before `now`'s credit). Reserves that stretch of the stream.
    pub fn reserve_at(&self, now: Instant, bytes: usize) -> Instant {
        let bps = self.rate_bps.load(Ordering::Relaxed).max(1);
        let cost = Duration::from_nanos((bytes as u64).saturating_mul(8).saturating_mul(1_000_000_000) / bps);
        let mut n = self.next.lock().unwrap_or_else(|e| e.into_inner());
        let floor = now.checked_sub(PACER_BURST).unwrap_or(now);
        let start = n.map_or(floor, |t| t.max(floor));
        *n = Some(start + cost);
        start
    }

    /// How long a wagon of `bytes` must wait before it goes out (zero while it is within the allowed burst).
    pub fn reserve(&self, bytes: usize) -> Duration {
        let now = Instant::now();
        self.reserve_at(now, bytes).saturating_duration_since(now)
    }
}

#[cfg(test)]
mod pacer_tests {
    use super::*;

    #[test]
    fn a_stream_is_spread_at_the_rate_and_the_idle_credit_is_limited() {
        let p = Pacer::new(100); // 100 Mbit/s = 12.5 MB/s: 1250 bytes take 100 us
        let t0 = Instant::now();
        // an idle pacer lets the first wagons go at once (the burst credit is 2 ms = 20 wagons of 1250 B)
        let mut waits = Vec::new();
        for _ in 0..200 {
            waits.push(p.reserve_at(t0, 1250).saturating_duration_since(t0));
        }
        assert_eq!(waits[0], Duration::ZERO);
        assert!(waits[10] == Duration::ZERO, "inside the burst");
        // 200 wagons of 1250 B = 250 KB at 12.5 MB/s = 20 ms, minus the 2 ms of credit
        let last = *waits.last().unwrap();
        assert!(last >= Duration::from_millis(17) && last <= Duration::from_millis(19), "{last:?}");
        // after a long silence the credit does not pile up beyond the burst
        let later = t0 + Duration::from_secs(10);
        let w: Vec<_> = (0..100).map(|_| p.reserve_at(later, 1250).saturating_duration_since(later)).collect();
        assert_eq!(w[0], Duration::ZERO);
        assert!(w[99] >= Duration::from_millis(7), "ten ms of data cannot all leave at once: {:?}", w[99]);
    }

    #[test]
    fn the_rate_is_what_was_asked_for() {
        assert_eq!(Pacer::new(300).rate_mbps(), 300);
        assert_eq!(Pacer::new(0).rate_mbps(), 1, "zero is not a rate");
    }
}
