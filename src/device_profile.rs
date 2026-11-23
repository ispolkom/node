//! Device profile: the nickname and avatar of each paired phone.
//!
//! This lets people appear in the node's “People” list under the names and pictures they chose,
//! rather than as “YANDI Mobile”. The image is processed like the owner's avatar: rebuilt from pixels without metadata.
//! A person can hide the profile (`hide`); other users then see the device without its nickname or picture.

use crate::mobile_profile::{sanitize_avatar, MAX_AVATAR_IN, MAX_NICK};
use axum::http::StatusCode;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Clone, Default, Serialize, Deserialize)]
struct Entry {
    #[serde(default)]
    nick: String,
    #[serde(default)]
    avatar_hash: String,
    #[serde(default)]
    updated_ms: u64,
    #[serde(default)]
    hide: bool,
}

static STORE: Mutex<Option<(PathBuf, HashMap<String, Entry>)>> = Mutex::new(None);

fn dir() -> PathBuf {
    crate::util::data_dir::data_dir().join("device_profiles")
}

fn index_path() -> PathBuf {
    dir().join("index.json")
}

fn avatar_path(peer: &str) -> PathBuf {
    dir().join(format!("{}.png", peer))
}

fn valid_peer(peer: &str) -> bool {
    peer.len() == 64 && peer.bytes().all(|b| b.is_ascii_hexdigit())
}

fn with_store<R>(f: impl FnOnce(&mut HashMap<String, Entry>) -> R) -> R {
    let mut g = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let root = dir();
    // The data directory can change (tests or a node-profile switch), so reload the state.
    if g.as_ref().map_or(true, |(p, _)| *p != root) {
        let map = std::fs::read_to_string(index_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        *g = Some((root, map));
    }
    f(&mut g.as_mut().unwrap().1)
}

fn save(map: &HashMap<String, Entry>) -> Result<(), (StatusCode, String)> {
    let fail = || (StatusCode::INTERNAL_SERVER_ERROR, "не удалось сохранить профиль".to_string());
    std::fs::create_dir_all(dir()).map_err(|_| fail())?;
    let s = serde_json::to_vec(map).map_err(|_| fail())?;
    crate::util::private_file::write_private(&index_path(), &s).map_err(|_| fail())
}

/// Save a device profile. `None` means unchanged; an empty image string removes the image.
pub fn put(peer: &str, nick: Option<&str>, avatar_b64: Option<&str>, hide: Option<bool>) -> Result<Value, (StatusCode, String)> {
    let bad = |m: &str| (StatusCode::BAD_REQUEST, m.to_string());
    if !valid_peer(peer) {
        return Err(bad("неверный номер устройства"));
    }
    let nick: Option<String> = match nick {
        Some(n) => {
            let n: String = n.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(MAX_NICK).collect();
            if crate::content_policy::find_link(&n).is_some() {
                return Err(bad("ссылки в YANDI не передаются: в нике не должно быть адреса сайта"));
            }
            if crate::content_policy::is_reserved_name(&n) {
                return Err(bad("это имя занято узлом: так подписывает сообщения администрация"));
            }
            Some(n)
        }
        None => None,
    };
    // Validate and process the image before writing anything.
    let image: Option<Option<Vec<u8>>> = match avatar_b64 {
        None => None,
        Some("") => Some(None),
        Some(b64) => {
            let raw = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|_| bad("картинка не в base64"))?;
            if raw.is_empty() || raw.len() > MAX_AVATAR_IN {
                return Err(bad("картинка слишком большая"));
            }
            Some(Some(sanitize_avatar(&raw).ok_or_else(|| bad("это не картинка или она слишком большая"))?))
        }
    };
    with_store(|m| {
        let mut e = m.get(peer).cloned().unwrap_or_default();
        if let Some(n) = nick {
            e.nick = n;
        }
        if let Some(h) = hide {
            e.hide = h;
        }
        match image {
            Some(Some(bytes)) => {
                std::fs::create_dir_all(dir()).map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "не удалось сохранить картинку".to_string()))?;
                crate::util::private_file::write_private(&avatar_path(peer), &bytes)
                    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "не удалось сохранить картинку".to_string()))?;
                e.avatar_hash = hex::encode(Sha256::digest(&bytes));
            }
            Some(None) => {
                let _ = std::fs::remove_file(avatar_path(peer));
                e.avatar_hash.clear();
            }
            None => {}
        }
        e.updated_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        m.insert(peer.to_string(), e.clone());
        save(m)?;
        Ok(json!({"nick": e.nick, "avatar_hash": e.avatar_hash, "updated_ms": e.updated_ms, "hide": e.hide}))
    })
}

/// How long a profile must remain before it can be considered orphaned: a newly registered device gets time to set its nickname
/// before cleanup sees it in the registry (registration and cleanup cannot erase a fresh profile).
pub const ORPHAN_MIN_AGE_MS: u64 = 10 * 60 * 1000;

/// Forget profiles (nickname and picture) for devices no longer on the node: a guest may disappear on restart while its profile remains on disk.
/// `registered` contains the device IDs still present on the node. Profiles younger than `ORPHAN_MIN_AGE_MS` are preserved. Returns the count removed.
pub fn retain_registered(registered: &std::collections::HashSet<String>) -> usize {
    retain_registered_aged(registered, ORPHAN_MIN_AGE_MS)
}

pub(crate) fn retain_registered_aged(registered: &std::collections::HashSet<String>, min_age_ms: u64) -> usize {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let gone: Vec<String> = with_store(|m| {
        m.iter().filter(|(k, e)| !registered.contains(*k) && now.saturating_sub(e.updated_ms) >= min_age_ms).map(|(k, _)| k.clone()).collect()
    });
    for p in &gone {
        forget(p);
    }
    // Images without an index entry (left by interrupted cleanup): remove only old files.
    let known: std::collections::HashSet<String> = with_store(|m| m.keys().cloned().collect());
    let mut files = 0;
    if let Ok(rd) = std::fs::read_dir(dir()) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(peer) = name.strip_suffix(".png") else { continue };
            if !valid_peer(peer) || registered.contains(peer) || known.contains(peer) {
                continue;
            }
            let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|d| d.as_millis() as u64 >= min_age_ms).unwrap_or(false);
            if old {
                let _ = std::fs::remove_file(e.path());
                files += 1;
            }
        }
    }
    gone.len() + files
}

/// Profile visible to others; a hidden profile is empty.
pub fn public(peer: &str) -> (String, String) {
    with_store(|m| match m.get(peer) {
        Some(e) if !e.hide => (e.nick.clone(), e.avatar_hash.clone()),
        _ => (String::new(), String::new()),
    })
}

/// The device's own profile, including the hide flag.
pub fn own(peer: &str) -> Value {
    with_store(|m| {
        let e = m.get(peer).cloned().unwrap_or_default();
        json!({"nick": e.nick, "avatar_hash": e.avatar_hash, "updated_ms": e.updated_ms, "hide": e.hide})
    })
}

/// Device picture for others; hidden profiles have none.
pub fn avatar(peer: &str) -> Option<Vec<u8>> {
    if !valid_peer(peer) || with_store(|m| m.get(peer).map_or(true, |e| e.hide || e.avatar_hash.is_empty())) {
        return None;
    }
    std::fs::read(avatar_path(peer)).ok()
}

/// The node forgot the device: remove its profile and picture.
pub fn forget(peer: &str) {
    if !valid_peer(peer) {
        return;
    }
    let _ = std::fs::remove_file(avatar_path(peer));
    with_store(|m| {
        if m.remove(peer).is_some() {
            let _ = save(m);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> String {
        let mut src = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(200, 200, image::Rgba([200, 30, 30, 255])))
            .write_to(&mut std::io::Cursor::new(&mut src), image::ImageFormat::Png)
            .unwrap();
        base64::engine::general_purpose::STANDARD.encode(src)
    }

    #[test]
    fn nick_and_picture_are_saved_hidden_on_request_and_forgotten() {
        let _g = crate::complaints::tests::SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let peer = "ab".repeat(32);
        put(&peer, Some("  Аня  "), Some(&png()), None).unwrap();
        let (nick, hash) = public(&peer);
        assert_eq!(nick, "Аня");
        assert_eq!(hash.len(), 64);
        let img = avatar(&peer).unwrap();
        assert_eq!(&img[..4], &[0x89, b'P', b'N', b'G']);
        // A hidden profile is empty for others but visible to its owner.
        put(&peer, None, None, Some(true)).unwrap();
        assert_eq!(public(&peer), (String::new(), String::new()));
        assert!(avatar(&peer).is_none());
        assert_eq!(own(&peer)["nick"], "Аня");
        forget(&peer);
        assert_eq!(own(&peer)["nick"], "");
    }

    #[test]
    fn links_garbage_and_oversize_are_refused() {
        let peer = "cd".repeat(32);
        assert!(put(&peer, Some("заходи на example.com"), None, None).is_err());
        assert!(put(&peer, None, Some(&base64::engine::general_purpose::STANDARD.encode(b"MZ\x00\x00 not an image")), None).is_err());
        assert!(put("не-номер", Some("x"), None, None).is_err());
        let huge = base64::engine::general_purpose::STANDARD.encode(vec![0u8; MAX_AVATAR_IN + 1]);
        assert!(put(&peer, None, Some(&huge), None).is_err());
    }
}

#[cfg(test)]
mod orphan_tests {
    use super::*;

    #[test]
    fn profiles_of_vanished_devices_are_swept_only_when_old_and_registered_ones_stay() {
        let _g = crate::complaints::tests::SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (keep, gone) = ("a1".repeat(32), "b2".repeat(32));
        put(&keep, Some("Остаётся"), None, None).unwrap();
        put(&gone, Some("Пропал"), None, None).unwrap();
        // Treat records from other tests as registered; check only this test's entries.
        let mut reg: std::collections::HashSet<String> = with_store(|m| m.keys().cloned().collect());
        reg.remove(&gone);
        // A fresh profile is not erased even when the device is absent from the snapshot: registration may lag behind cleanup.
        assert_eq!(retain_registered(&reg), 0, "свежий профиль не тронут");
        assert_eq!(public(&gone).0, "Пропал");
        // An old orphan is erased; a registered profile remains.
        assert_eq!(retain_registered_aged(&reg, 0), 1);
        assert_eq!(public(&keep).0, "Остаётся");
        assert_eq!(public(&gone).0, "", "ник пропавшего не остался");
        forget(&keep);
    }
}
