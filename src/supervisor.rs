//! Присмотр за долгоживущими задачами: чтобы падение фоновой задачи больше не было тихим.
//!
//! Раньше паника внутри `tokio::spawn` убивала только свою задачу, и никто этого не замечал: например, умирала рассылка карточек, а
//! узел продолжал «работать». Теперь долгоживущие циклы запускаются через [`supervise`]: каждая такая задача имеет **имя**,
//! её завершение — **событие** с причиной (паника, неожиданный возврат, ошибка), а политика говорит, что делать:
//!
//! * [`Policy::Log`] — записать в журнал и оставить (для задач, которым перезапуск не нужен);
//! * [`Policy::Restart`] — перезапустить с растущим отступом; **не более `max` перезапусков за `window`**, дальше задача считается
//!   упавшей (`failed`) и больше не перезапускается (иначе неисправная задача крутилась бы бесконечно);
//! * [`Policy::Fatal`] — критичная задача: её потеря означает выход процесса с кодом `EXIT_CRITICAL` (перезапуск целиком —
//!   за внешним наблюдателем: systemd, служба Windows и т.п.).
//!
//! Состояние всех присматриваемых задач видно на `GET /api/kernel/status`. Для задач, которые запускают лишь на короткий срок
//! (по одной на соединение, на пробу и т.п.), присмотр не нужен — они остаются обычным `tokio::spawn`; проверка `scripts/check_spawn.sh`
//! не даёт числу «голых» запусков расти.
//!
//! Намеренно маленький модуль: никакой шины, поколений и квот — их добавят, когда в них появится реальная нужда (`docs/ROADMAP_KERNEL.md`).
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;

/// Код выхода процесса при потере критичной задачи.
pub const EXIT_CRITICAL: i32 = 70;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Policy {
    Log,
    /// не более `max` перезапусков за `window`; отступ растёт от `backoff` вдвое, но не больше `MAX_BACKOFF`
    Restart { max: u32, window: Duration, backoff: Duration },
    Fatal,
}

impl Policy {
    /// Обычный цикл: до 5 перезапусков за 10 минут, отступ от 1 секунды.
    pub fn restart() -> Policy {
        Policy::Restart { max: 5, window: Duration::from_secs(600), backoff: Duration::from_secs(1) }
    }
}

const MAX_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Running,
    Restarting,
    /// слишком много падений за короткое время — больше не перезапускается
    Failed,
    /// закончилась и не должна была перезапускаться
    Finished,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskInfo {
    pub name: String,
    pub state: State,
    pub restarts: u32,
    /// почему закончилась в последний раз (`panic: …`, `returned`, `cancelled`)
    pub last_exit: Option<String>,
    /// секунд с начала текущего запуска
    pub up_secs: u64,
}

struct Entry {
    state: State,
    restarts: u32,
    last_exit: Option<String>,
    since: Instant,
    /// остановка самого присмотрщика и его текущей задачи (нужна, если то же имя регистрируют заново)
    supervisor: Option<tokio::task::AbortHandle>,
    child: Option<tokio::task::AbortHandle>,
    /// номер регистрации: старый присмотрщик, потерявший его, больше ничего не пишет
    generation: u64,
}

fn registry() -> &'static Mutex<HashMap<&'static str, Entry>> {
    static R: OnceLock<Mutex<HashMap<&'static str, Entry>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

fn new_entry(generation: u64) -> Entry {
    Entry { state: State::Running, restarts: 0, last_exit: None, since: Instant::now(), supervisor: None, child: None, generation }
}

/// Записать в состояние, только если регистрация ещё «моя» (после замены того же имени старый присмотрщик молчит).
fn set(name: &'static str, generation: u64, f: impl FnOnce(&mut Entry)) -> bool {
    let mut g = registry().lock().unwrap_or_else(|e| e.into_inner());
    match g.get_mut(name) {
        Some(e) if e.generation == generation => {
            f(e);
            true
        }
        _ => false,
    }
}

/// Занять имя: если оно уже было, прежний присмотрщик и его задача останавливаются, счётчики начинаются заново.
fn claim(name: &'static str) -> u64 {
    static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let generation = GEN.fetch_add(1, Ordering::Relaxed);
    let mut g = registry().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(old) = g.insert(name, new_entry(generation)) {
        eprintln!("[kernel] задача «{name}» зарегистрирована заново: прежняя останавливается");
        for h in [old.supervisor, old.child].into_iter().flatten() {
            h.abort();
        }
    }
    generation
}

/// Состояние всех присматриваемых задач (по имени).
pub fn status() -> Vec<TaskInfo> {
    let g = registry().lock().unwrap_or_else(|e| e.into_inner());
    let mut v: Vec<TaskInfo> = g.iter().map(|(n, e)| TaskInfo { name: n.to_string(), state: e.state.clone(), restarts: e.restarts, last_exit: e.last_exit.clone(), up_secs: e.since.elapsed().as_secs() }).collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

fn panic_message(p: &Box<dyn std::any::Any + Send>) -> String {
    let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown panic".into());
    msg.chars().take(200).collect()
}

fn panic_text(e: tokio::task::JoinError) -> String {
    if e.is_cancelled() {
        return "cancelled".into();
    }
    format!("panic: {}", panic_message(&e.into_panic()))
}

/// Запустить долгоживущую задачу под присмотром. `make` создаёт задачу заново при каждом (пере)запуске, поэтому всё нужное
/// она берёт из клонов, подготовленных до вызова.
pub fn supervise<F, Fut>(name: &'static str, policy: Policy, make: F)
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    supervise_with_exit(name, policy, make, |code| std::process::exit(code));
}

/// То же, но с заменяемым «выходом процесса» (для проверок).
pub fn supervise_with_exit<F, Fut, X>(name: &'static str, policy: Policy, mut make: F, exit: X)
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
    X: Fn(i32) + Send + 'static,
{
    let generation = claim(name);
    let sup = tokio::spawn(async move {
        let mut starts: VecDeque<Instant> = VecDeque::new();
        let mut streak = 0u32;
        loop {
            starts.push_back(Instant::now());
            if !set(name, generation, |e| {
                e.state = State::Running;
                e.since = Instant::now();
            }) {
                return; // имя занято более новой регистрацией
            }
            let began = Instant::now();
            // `make()` вызывается под защитой: паника при создании задачи — такое же событие, как паника внутри неё
            let reason = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| make())) {
                Err(p) => format!("panic while creating the task: {}", panic_message(&p)),
                Ok(fut) => {
                    let child = tokio::spawn(fut);
                    let ah = child.abort_handle();
                    if !set(name, generation, |e| e.child = Some(ah)) {
                        child.abort();
                        return;
                    }
                    match child.await {
                        Ok(()) => "returned".to_string(),
                        Err(e) => panic_text(e),
                    }
                }
            };
            eprintln!("[kernel] задача «{name}» закончилась: {reason}");
            if !set(name, generation, |e| e.last_exit = Some(reason.clone())) {
                return;
            }
            match policy {
                Policy::Log => {
                    set(name, generation, |e| e.state = State::Finished);
                    return;
                }
                Policy::Fatal => {
                    eprintln!("[kernel] критичная задача «{name}» потеряна — процесс завершается с кодом {EXIT_CRITICAL}");
                    exit(EXIT_CRITICAL);
                    return;
                }
                Policy::Restart { max, window, backoff } => {
                    let now = Instant::now();
                    while starts.front().map(|t| now.duration_since(*t) > window).unwrap_or(false) {
                        starts.pop_front();
                    }
                    // запуск, проработавший дольше окна, считается здоровым: счёт неудач подряд обнуляется
                    streak = if began.elapsed() > window { 0 } else { streak + 1 };
                    if starts.len() as u32 > max {
                        eprintln!("[kernel] задача «{name}» падает слишком часто ({max} перезапусков за {} с) — больше не перезапускается", window.as_secs());
                        set(name, generation, |e| e.state = State::Failed);
                        return;
                    }
                    let wait = (backoff * 2u32.saturating_pow(streak.saturating_sub(1).min(10))).min(MAX_BACKOFF);
                    if !set(name, generation, |e| {
                        e.state = State::Restarting;
                        e.restarts += 1;
                    }) {
                        return;
                    }
                    tokio::time::sleep(wait).await;
                }
            }
        }
    });
    set(name, generation, |e| e.supervisor = Some(sup.abort_handle()));
}

/// `GET /api/kernel/status` — состояние присматриваемых задач (под проверкой входа владельца).
pub fn router<S: Clone + Send + Sync + 'static>() -> axum::Router<S> {
    async fn kernel_status() -> axum::Json<serde_json::Value> {
        let tasks = status();
        let failed = tasks.iter().filter(|t| t.state == State::Failed).count();
        axum::Json(serde_json::json!({"tasks": tasks, "failed": failed}))
    }
    axum::Router::new().route("/api/kernel/status", axum::routing::get(kernel_status))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    fn find(name: &str) -> TaskInfo {
        status().into_iter().find(|t| t.name == name).expect("task is registered")
    }

    async fn until(mut f: impl FnMut() -> bool) -> bool {
        for _ in 0..200 {
            if f() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    #[tokio::test]
    async fn a_panicking_loop_is_noticed_restarted_and_its_neighbours_are_not_touched() {
        let runs = Arc::new(AtomicU32::new(0));
        let r = runs.clone();
        supervise("t_panics_twice", Policy::Restart { max: 5, window: Duration::from_secs(60), backoff: Duration::from_millis(10) }, move || {
            let r = r.clone();
            async move {
                if r.fetch_add(1, Ordering::SeqCst) < 2 {
                    panic!("boom {}", r.load(Ordering::SeqCst));
                }
                std::future::pending::<()>().await;
            }
        });
        let neighbour = Arc::new(AtomicU32::new(0));
        let n = neighbour.clone();
        supervise("t_neighbour", Policy::restart(), move || {
            let n = n.clone();
            async move {
                loop {
                    n.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        });
        assert!(until(|| runs.load(Ordering::SeqCst) >= 3).await, "it was started again after both panics");
        assert!(until(|| find("t_panics_twice").state == State::Running && find("t_panics_twice").restarts == 2).await);
        let info = find("t_panics_twice");
        assert!(info.last_exit.unwrap().starts_with("panic: boom"), "the cause is recorded");
        let before = neighbour.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(neighbour.load(Ordering::SeqCst) > before, "the neighbour never stopped");
        assert_eq!(find("t_neighbour").restarts, 0);
    }

    #[tokio::test]
    async fn an_unexpected_return_counts_as_a_failure_and_a_crash_loop_ends_in_failed() {
        let runs = Arc::new(AtomicU32::new(0));
        let r = runs.clone();
        supervise("t_returns", Policy::Restart { max: 3, window: Duration::from_secs(60), backoff: Duration::from_millis(5) }, move || {
            let r = r.clone();
            async move {
                r.fetch_add(1, Ordering::SeqCst);
            }
        });
        assert!(until(|| find("t_returns").state == State::Failed).await, "too many failures: it stops for good");
        let seen = runs.load(Ordering::SeqCst);
        assert_eq!(seen, 4, "the first run and three restarts");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(runs.load(Ordering::SeqCst), seen, "a failed task is not started again");
        assert_eq!(find("t_returns").last_exit.as_deref(), Some("returned"));
    }

    #[tokio::test]
    async fn the_log_policy_leaves_a_finished_task_alone_and_a_fatal_one_ends_the_process() {
        supervise("t_once", Policy::Log, || async {});
        assert!(until(|| find("t_once").state == State::Finished).await);
        assert_eq!(find("t_once").restarts, 0);

        let code = Arc::new(AtomicU32::new(0));
        let c = code.clone();
        supervise_with_exit("t_critical", Policy::Fatal, || async { panic!("keys are corrupted") }, move |x| c.store(x as u32, Ordering::SeqCst));
        assert!(until(|| code.load(Ordering::SeqCst) == EXIT_CRITICAL as u32).await, "the process is asked to exit with the critical code");
        assert!(find("t_critical").last_exit.unwrap().contains("keys are corrupted"));
    }

    #[tokio::test]
    async fn a_panic_while_creating_the_task_is_an_event_too_and_the_supervisor_survives() {
        let tries = Arc::new(AtomicU32::new(0));
        let t = tries.clone();
        supervise("t_factory_panics", Policy::Restart { max: 5, window: Duration::from_secs(60), backoff: Duration::from_millis(5) }, move || {
            if t.fetch_add(1, Ordering::SeqCst) < 2 {
                panic!("factory exploded");
            }
            async { std::future::pending::<()>().await }
        });
        assert!(until(|| tries.load(Ordering::SeqCst) >= 3).await, "the supervisor kept going after the factory panicked");
        assert!(until(|| find("t_factory_panics").state == State::Running && find("t_factory_panics").restarts == 2).await);
        assert!(find("t_factory_panics").last_exit.unwrap().contains("factory exploded"));
    }

    #[tokio::test]
    async fn registering_the_same_name_again_replaces_the_old_task_and_starts_clean() {
        let old_alive = Arc::new(AtomicU32::new(0));
        let o = old_alive.clone();
        supervise("t_same_name", Policy::Restart { max: 1, window: Duration::from_secs(60), backoff: Duration::from_millis(5) }, move || {
            let o = o.clone();
            async move {
                o.fetch_add(1, Ordering::SeqCst);
                panic!("old one fails");
            }
        });
        assert!(until(|| find("t_same_name").state == State::Failed).await);
        let new_runs = Arc::new(AtomicU32::new(0));
        let n = new_runs.clone();
        supervise("t_same_name", Policy::Log, move || {
            let n = n.clone();
            async move {
                n.fetch_add(1, Ordering::SeqCst);
                std::future::pending::<()>().await;
            }
        });
        assert!(until(|| new_runs.load(Ordering::SeqCst) == 1).await);
        let info = find("t_same_name");
        assert_eq!((info.state, info.restarts, info.last_exit), (State::Running, 0, None), "no stale counters or reasons from the old registration");
        let seen = old_alive.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(old_alive.load(Ordering::SeqCst), seen, "the old supervisor is gone and never restarts anything");
        assert_eq!(find("t_same_name").state, State::Running, "and it cannot overwrite the new status");
    }

    #[test]
    fn the_status_is_json_ready() {
        let t = TaskInfo { name: "x".into(), state: State::Restarting, restarts: 2, last_exit: Some("returned".into()), up_secs: 3 };
        assert_eq!(serde_json::to_value(&t).unwrap()["state"], "restarting");
    }
}
