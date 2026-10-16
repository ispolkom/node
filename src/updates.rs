//! Уведомление об обновлениях узла: раз в несколько часов опрашиваются каналы (сайт, GitHub), проверяется подпись, срок и номер
//! выпуска; найденное новее установленного показывается владельцу (веб-страница, оболочка ПК). Сам узел ничего не ставит:
//! установка — `sudo apt install ./yandi-node_….deb`, после проверки суммы командой `yandi-release verify`.
//!
//! Настройка — файл `update.json` в каталоге данных: `{"enabled": true, "channels": [{"name": "...", "base": "https://.../"}]}`.
//! Без файла работают каналы по умолчанию. Состояние (самый большой виденный номер выпуска — защита от отката) — `update_state.json`.
use crate::update_check::{check, default_channels, is_newer, own_version, Channel};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Mutex;

const PUBKEY_HEX: &str = include_str!("../release/yandi-release-pubkey.hex");
const CHECK_EVERY_SECS: u64 = 6 * 3600;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Settings {
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default)]
    channels: Vec<Channel>,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self { enabled: true, channels: Vec::new() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct State {
    /// самый большой номер выпуска, подтверждённый подписью: меньший (откат) не принимается
    #[serde(default)]
    max_seq: u64,
    #[serde(default)]
    last_check: u64,
    #[serde(default)]
    available: Option<Value>,
    #[serde(default)]
    errors: Vec<String>,
    /// порог отката утрачен (файл состояния и его копия есть, но не читаются): кандидаты не принимаются, пока владелец не удалит оба файла
    #[serde(skip)]
    refuse: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn dir() -> std::path::PathBuf {
    crate::util::data_dir::data_dir()
}

fn settings() -> Settings {
    std::fs::read(dir().join("update.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn state_path() -> std::path::PathBuf {
    dir().join("update_state.json")
}

/// Прочитать состояние: основной файл и копию; порогом отката берётся БОЛЬШИЙ из читаемых номеров (оба пишутся одним значением, но
/// между записями возможен обрыв). Второе значение: файл был, но не читается ни он, ни копия — порог неизвестен.
fn load_state() -> (State, bool) {
    let main = state_path();
    let bak = main.with_extension("json.bak");
    let mut damaged = false;
    let mut best: Option<State> = None;
    for p in [&main, &bak] {
        match std::fs::read(p) {
            Ok(b) => match serde_json::from_slice::<State>(&b) {
                Ok(mut st) => {
                    // «доступное обновление» с диска — не проверенное предложение: заново оно берётся из подписанного манифеста
                    st.available = None;
                    if best.as_ref().map_or(true, |cur| st.max_seq > cur.max_seq) {
                        best = Some(st);
                    }
                }
                Err(_) => damaged = true,
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => damaged = true,
        }
    }
    match best {
        Some(st) => (st, false),
        None => (State::default(), damaged),
    }
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let st = g.get_or_insert_with(|| {
        let (mut st, damaged) = load_state();
        if damaged {
            st.refuse = true;
            st.errors = vec!["состояние проверки обновлений повреждено: порог номера выпуска неизвестен, новые выпуски не принимаются (удалите update_state.json и update_state.json.bak, чтобы начать заново)".into()];
            eprintln!("[update] update_state.json повреждён: проверка обновлений остановлена до восстановления");
        }
        st
    });
    let r = f(st);
    if !st.refuse {
        if let Ok(bytes) = serde_json::to_vec_pretty(&*st) {
            let main = state_path();
            // обе копии получают ОДНО новое значение: сначала копия, потом основной файл (обрыв между ними не теряет порог — берётся больший)
            if let Err(e) = crate::util::private_file::write_private(&main.with_extension("json.bak"), &bytes) {
                eprintln!("[update] не удалось сохранить копию состояния проверки: {e}");
            }
            if let Err(e) = crate::util::private_file::write_private(&main, &bytes) {
                eprintln!("[update] не удалось сохранить состояние проверки: {e}");
            }
        }
    }
    r
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn pubkey() -> Option<ed25519_dalek::VerifyingKey> {
    let hex_s = PUBKEY_HEX.trim();
    if !crate::release::lower_hex(hex_s, 64) {
        return None;
    }
    let bytes: [u8; 32] = hex::decode(hex_s).ok()?.try_into().ok()?;
    ed25519_dalek::VerifyingKey::from_bytes(&bytes).ok()
}

/// Что показывать владельцу.
pub fn status_json() -> Value {
    let s = settings();
    let st = with_state(|s| s.clone());
    // предложение с истёкшим сроком или не новее запущенного узла не выдаётся как доступное
    let t = now();
    let current = own_version();
    let available = st.available.clone().filter(|a| {
        let live = a["expires"].as_u64().is_none_or(|e| e == 0 || t <= e);
        live && a["version"].as_str().is_some_and(|v| is_newer(v, &current))
    });
    json!({
        "enabled": s.enabled,
        "current": own_version(),
        "last_check": st.last_check,
        "available": available,
        "errors": st.errors,
        "channels": if s.channels.is_empty() { default_channels() } else { s.channels },
    })
}

/// Опросить каналы сейчас; возвращает то же, что `status_json`.
pub async fn check_now() -> Value {
    let s = settings();
    if !s.enabled {
        return status_json();
    }
    let Some(vk) = pubkey() else {
        with_state(|st| st.errors = vec!["в сборку не вшит открытый ключ выпусков".into()]);
        return status_json();
    };
    let channels = if s.channels.is_empty() { default_channels() } else { s.channels };
    // порог отката утрачен — без него «новее» не определить: каналы не опрашиваются, ошибка остаётся видна
    if with_state(|st| st.refuse) {
        return status_json();
    }
    let min_seq = with_state(|st| st.max_seq);
    let t = now();
    let (found, errors) = check(&channels, &vk, min_seq, t).await;
    with_state(|st| {
        st.last_check = t;
        st.errors = errors;
        match found {
            Some(f) => {
                st.max_seq = st.max_seq.max(f.manifest.seq);
                st.available = if is_newer(&f.manifest.version, &own_version()) {
                    Some(json!({
                        "version": f.manifest.version,
                        "seq": f.manifest.seq,
                        "platform": f.manifest.platform,
                        "notes": f.manifest.notes,
                        "channel": f.channel.name,
                        "expires": f.manifest.expires,
                        "files": f.manifest.files.iter().map(|x| json!({"name": x.name, "sha256": x.sha256, "size": x.size})).collect::<Vec<_>>(),
                        "base": f.channel.base,
                    }))
                } else {
                    None
                };
            }
            None => {} // каналы недоступны: прежнее известное остаётся, ошибки видны
        }
    });
    status_json()
}

/// Фоновая проверка: первая — через минуту после старта (в проверках — `YANDI_UPDATE_FIRST_CHECK_SECS`), дальше раз в 6 часов.
pub fn spawn() {
    crate::supervisor::supervise("update_check", crate::supervisor::Policy::restart(), move || async move {
        let first: u64 = std::env::var("YANDI_UPDATE_FIRST_CHECK_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
        tokio::time::sleep(std::time::Duration::from_secs(first)).await;
        loop {
            let v = check_now().await;
            if let Some(a) = v.get("available").filter(|a| !a.is_null()) {
                println!("[update] доступна версия {} (№{}), канал {}", a["version"].as_str().unwrap_or("?"), a["seq"], a["channel"].as_str().unwrap_or("?"));
            }
            tokio::time::sleep(std::time::Duration::from_secs(CHECK_EVERY_SECS)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Порча файла состояния не молчит и не принимается за чистый первый запуск; копия возвращает порог.
    #[test]
    fn a_damaged_state_file_is_reported_and_the_backup_restores_the_floor() {
        let _g = crate::complaints::tests::SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let main = state_path();
        let bak = main.with_extension("json.bak");
        let _ = std::fs::create_dir_all(main.parent().unwrap());
        let _ = std::fs::remove_file(&bak);
        std::fs::write(&main, b"{not json").unwrap();
        let (st, damaged) = load_state();
        assert!(damaged && st.max_seq == 0, "порча замечена");
        std::fs::write(&bak, br#"{"max_seq":7}"#).unwrap();
        let (st, damaged) = load_state();
        assert!(!damaged && st.max_seq == 7, "порог взят из копии");
        // основной файл читается, но с меньшим номером (обрыв между записями): берётся больший из двух
        std::fs::write(&main, br#"{"max_seq":3}"#).unwrap();
        let (st2, _) = load_state();
        assert_eq!(st2.max_seq, 7, "меньшее значение не снижает порог");
        let _ = st;
        let _ = std::fs::remove_file(&main);
        let _ = std::fs::remove_file(&bak);
        let (_, damaged) = load_state();
        assert!(!damaged, "чистый первый запуск — не порча");
    }

    #[test]
    fn the_pinned_release_key_is_a_valid_public_key() {
        assert!(pubkey().is_some());
    }

    #[test]
    fn defaults_are_enabled_with_the_two_standard_channels() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert!(s.enabled && s.channels.is_empty());
        assert_eq!(default_channels().len(), 2);
    }
}
