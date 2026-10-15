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

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let st = g.get_or_insert_with(|| {
        std::fs::read(state_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    });
    let r = f(st);
    if let Ok(bytes) = serde_json::to_vec_pretty(&*st) {
        let _ = crate::util::private_file::write_private(&state_path(), &bytes);
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
    json!({
        "enabled": s.enabled,
        "current": own_version(),
        "last_check": st.last_check,
        "available": st.available,
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
