//! Обмен профилем (ник и аватар) между узлами — делает сам узел, а не каждое приложение отдельно.
//!
//! Протокол тот же, что у телефонов (`avatar_msg.dart`), поэтому телефоны и узел понимают друг друга:
//!   `\u0001yandi-avh:{"h":хэш}` — «моя картинка сейчас такая» (пусто — картинки нет),
//!   `\u0001yandi-avq:{"h":хэш}` — «пришли картинку с этим хэшем»,
//!   `\u0001yandi-ava:{"h":хэш,"png":base64}` — сама картинка,
//!   `\u0001yandi-avn:{"n":ник}` — ник (пусто — убран); у старых приложений тоже скрывается как «аватарное».
//! Принимаем только от сохранённых контактов; картинку — только запрошенную, PNG ≤ 40 КБ, с совпавшим хэшем и в пределах размеров.

use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const H: &str = "\u{1}yandi-avh:";
const Q: &str = "\u{1}yandi-avq:";
const A: &str = "\u{1}yandi-ava:";
const N: &str = "\u{1}yandi-avn:";
const MAX_PNG: usize = 40 * 1024;
const MAX_SIDE: u32 = 1024;
const PENDING_TTL: Duration = Duration::from_secs(300);
const MAX_NICK: usize = 40;

/// Служебное сообщение профиля: не показывается в переписке.
pub fn is_service(text: &str) -> bool {
    text.starts_with("\u{1}yandi-av") || text.starts_with("\u{1}yandi-rx:") // профиль и реакции-смайлы телефонов
}

fn hex64(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

fn clean_nick(raw: &str) -> String {
    let s: String = raw.chars().filter(|c| !c.is_control()).collect();
    s.trim().chars().take(MAX_NICK).collect()
}

fn valid_png(b: &[u8]) -> bool {
    if b.len() < 16 || b.len() > MAX_PNG || b[..8] != [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
        return false;
    }
    let Ok(mut r) = image::ImageReader::new(std::io::Cursor::new(b)).with_guessed_format() else { return false };
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(16 * 1024 * 1024);
    r.limits(limits);
    r.decode().is_ok()
}

fn base_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".yandi")
}

/// Сохранённые контакты (короткие номера) из contacts.json.
fn contact_shorts() -> HashSet<String> {
    let raw: serde_json::Value = std::fs::read_to_string(crate::util::data_dir::data_dir().join("contacts.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::json!({}));
    raw["contacts"]
        .as_array()
        .map(|a| a.iter().filter_map(|c| c["short_id"].as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

// ── хранение чужого профиля ──

fn nicks_path(base: &Path) -> PathBuf {
    base.join("peer_nicks.json")
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct NickRec {
    n: String,
    /// когда контакт в последний раз сообщал ник (мс)
    t: u64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn load_recs(base: &Path) -> HashMap<String, NickRec> {
    std::fs::read_to_string(nicks_path(base)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn load_nicks(base: &Path) -> HashMap<String, String> {
    load_recs(base).into_iter().map(|(k, v)| (k, v.n)).collect()
}

fn save_recs(base: &Path, m: &HashMap<String, NickRec>) {
    let _ = std::fs::create_dir_all(base);
    if let Ok(s) = serde_json::to_vec(m) {
        let _ = crate::util::private_file::write_private(&nicks_path(base), &s);
    }
}

fn set_nick(base: &Path, short: &str, nick: &str) {
    let mut m = load_recs(base);
    let changed = if nick.is_empty() {
        m.remove(short).is_some()
    } else {
        m.insert(short.to_string(), NickRec { n: nick.to_string(), t: now_ms() }).is_none_or(|old| old.n != nick)
    };
    if changed {
        save_recs(base, &m);
    }
}

/// Срок хранения чужого профиля, когда человек больше не в контактах.
const KEEP_NON_CONTACT_MS: u64 = 30 * 24 * 3600 * 1000;

/// Локальная очистка кэша: ники и аватары тех, кого нет в контактах, стираются через 30 дней после последнего обновления
/// (до нового появления человека в сети). Сети не касается, у контактов и своего профиля ничего не трогает.
fn cleanup(base: &Path, contacts: &HashSet<String>, my_short: &str, now: u64) -> usize {
    let mut removed = 0;
    let mut recs = load_recs(base);
    let before = recs.len();
    recs.retain(|k, v| contacts.contains(k) || now.saturating_sub(v.t) < KEEP_NON_CONTACT_MS);
    if recs.len() != before {
        removed += before - recs.len();
        save_recs(base, &recs);
    }
    if let Ok(rd) = std::fs::read_dir(base.join("avatars")) {
        for e in rd.flatten() {
            let path = e.path();
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
            if stem == my_short || contacts.contains(&stem) || stem.len() != 16 {
                continue;
            }
            let age = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|d| d.as_millis() as u64).unwrap_or(0);
            if age >= KEEP_NON_CONTACT_MS && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// Очистка по таймеру (вызывается из фоновой рассылки).
fn cleanup_now() {
    cleanup(&base_dir(), &contact_shorts(), &my_short_cell(), now_ms());
}

/// Ник, который контакт задал себе сам (локальное имя в contacts.json его перекрывает на стороне показа).
pub fn peer_nick(short: &str) -> Option<String> {
    load_nicks(&base_dir()).remove(short)
}

fn avatar_path(base: &Path, short: &str) -> PathBuf {
    base.join("avatars").join(format!("{short}.png"))
}

fn remove_peer_avatar(base: &Path, short: &str) {
    for ext in ["png", "jpg", "gif"] {
        let _ = std::fs::remove_file(base.join("avatars").join(format!("{short}.{ext}")));
    }
}

fn peer_avatar_hash(base: &Path, short: &str) -> Option<String> {
    std::fs::read(avatar_path(base, short)).ok().map(|b| sha(&b))
}

// ── приём ──

/// Запрошенные картинки: «контакт|хэш» → когда просили (чужой не может заставить нас писать на диск что попало).
static PENDING: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());

fn mark_pending(key: String) {
    let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    p.retain(|(_, t)| t.elapsed() < PENDING_TTL);
    if p.len() < 256 && !p.iter().any(|(k, _)| *k == key) {
        p.push((key, Instant::now()));
    }
}

fn take_pending(key: &str) -> bool {
    let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    p.retain(|(_, t)| t.elapsed() < PENDING_TTL);
    let n = p.len();
    p.retain(|(k, _)| k != key);
    n != p.len()
}

/// Разобрать входящее служебное сообщение от контакта [short]. Возвращает тексты, которые надо отправить ему в ответ.
/// [mine] — наша картинка (PNG), если есть. Вне списка контактов — ничего не делаем.
fn process(base: &Path, short: &str, text: &str, is_contact: bool, mine: Option<&[u8]>) -> Vec<String> {
    if !is_contact || !is_service(text) {
        return vec![];
    }
    let body = |p: &str| -> Option<serde_json::Value> { text.strip_prefix(p).and_then(|j| serde_json::from_str(j).ok()) };
    if let Some(m) = body(H) {
        let h = m["h"].as_str().unwrap_or("");
        if h.is_empty() {
            remove_peer_avatar(base, short);
        } else if hex64(h) && peer_avatar_hash(base, short).as_deref() != Some(h) {
            mark_pending(format!("{short}|{h}"));
            return vec![format!("{Q}{}", serde_json::json!({"h": h}))];
        }
    } else if let Some(m) = body(Q) {
        let h = m["h"].as_str().unwrap_or("");
        if let Some(png) = mine {
            if hex64(h) && sha(png) == h && valid_png(png) {
                use base64::Engine;
                let b64 = base64::engine::general_purpose::STANDARD.encode(png);
                return vec![format!("{A}{}", serde_json::json!({"h": h, "png": b64}))];
            }
        }
    } else if let Some(m) = body(A) {
        use base64::Engine;
        let h = m["h"].as_str().unwrap_or("");
        let b64 = m["png"].as_str().unwrap_or("");
        if !hex64(h) || b64.is_empty() || b64.len() > MAX_PNG * 4 / 3 + 8 || !take_pending(&format!("{short}|{h}")) {
            return vec![];
        }
        let Ok(png) = base64::engine::general_purpose::STANDARD.decode(b64) else { return vec![] };
        if sha(&png) != h || !valid_png(&png) {
            return vec![];
        }
        remove_peer_avatar(base, short);
        let p = avatar_path(base, short);
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = crate::util::private_file::write_private(&p, &png);
    } else if let Some(m) = body(N) {
        set_nick(base, short, &clean_nick(m["n"].as_str().unwrap_or("")));
    }
    vec![]
}

/// Входящее сообщение чата: служебное разбирается здесь. Ответы отправляются самим вызывающим (чат).
pub fn on_incoming(from: &crate::util::HashId, text: &str) -> Vec<String> {
    if !is_service(text) {
        return vec![];
    }
    let short = hex::encode(&from.0[..8]);
    let mine = crate::mobile_profile::avatar_bytes(&my_short_cell()).map(|(b, _)| b);
    process(&base_dir(), &short, text, contact_shorts().contains(&short), mine.as_deref())
}

// ── рассылка ──

static MY_SHORT: Mutex<String> = Mutex::new(String::new());

fn my_short_cell() -> String {
    MY_SHORT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Что слать контакту, которому уже сообщали состояние [prev] (хэш, ник) — пусто, если ничего не изменилось.
fn plan(prev: Option<&(String, String)>, hash: &str, nick: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (ph, pn) = prev.map(|(h, n)| (h.as_str(), n.as_str())).unwrap_or(("", ""));
    if hash != ph || (prev.is_none() && !hash.is_empty()) {
        if !hash.is_empty() || prev.is_some() {
            out.push(format!("{H}{}", serde_json::json!({"h": hash})));
        }
    }
    if nick != pn {
        out.push(format!("{N}{}", serde_json::json!({"n": nick})));
    }
    out
}

/// Своё состояние: (хэш картинки — только если это PNG до 40 КБ, ник).
fn my_state() -> (String, String) {
    let short = my_short_cell();
    let hash = crate::mobile_profile::avatar_bytes(&short)
        .filter(|(b, ty)| *ty == "image/png" && b.len() <= MAX_PNG)
        .map(|(b, _)| sha(&b))
        .unwrap_or_default();
    let nick = crate::mobile_profile::snapshot(&short)["nick"].as_str().map(clean_nick).unwrap_or_default();
    (hash, nick)
}

// События, по которым узел сверяет профиль с контактами (таймеров нет): профиль сохранён, контакт подключился, узел запущен.
static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();
static DIRTY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static SEEN: Mutex<Vec<[u8; 32]>> = Mutex::new(Vec::new());

/// Профиль (ник или картинка) изменился: сверить всех контактов, которые сейчас на связи, и сообщить своим телефонам.
pub fn changed() {
    DIRTY.store(true, std::sync::atomic::Ordering::SeqCst);
    WAKE.notify_one();
    crate::mobile_api::announce_profile_changed();
    crate::group_client::nudge(); // визитка ПК в группах обновляется сразу, а не к следующему обходу
}

/// Контакт подключился (или подтвердил себя): если у него не тот хэш, что мы ему уже отдали, отправим разницу.
pub fn peer_seen(id: &crate::util::HashId) {
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if !seen.contains(&id.0) && seen.len() < 4096 {
        seen.push(id.0);
    }
    drop(seen);
    WAKE.notify_one();
}

type Sent = HashMap<String, (String, String)>;

/// Отправка профиля не подтверждается получателем (он мог быть заперт или потерять пакет): раз в этот срок, при встрече с контактом,
/// профиль отправляется заново целиком. После перезапуска узла «когда отправлял» неизвестно, и первая встреча тоже шлёт заново.
const RESEND_AFTER: Duration = Duration::from_secs(3600);

fn resend_due(last_sent: Option<Instant>) -> bool {
    last_sent.map_or(true, |t| t.elapsed() >= RESEND_AFTER)
}

fn sent_path() -> PathBuf {
    base_dir().join("profile_sent.json")
}

fn load_sent() -> Sent {
    std::fs::read(sent_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_sent(sent: &Sent) {
    if let Ok(b) = serde_json::to_vec(sent) {
        if let Some(d) = sent_path().parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = crate::util::private_file::write_private(&sent_path(), &b);
    }
}

/// Рассылка по событиям: какой хэш и какой ник кому уже отдан, помнится на диске — после перезапуска повторно не шлём.
pub fn spawn(chat: std::sync::Arc<crate::communication::ChatManager>, p2p: std::sync::Arc<crate::p2p::P2PTransport>, my_id: crate::util::HashId) {
    *MY_SHORT.lock().unwrap_or_else(|e| e.into_inner()) = hex::encode(&my_id.0[..8]);
    DIRTY.store(true, std::sync::atomic::Ordering::SeqCst); // старт: сверить тех, кто уже на связи
    crate::supervisor::supervise("profile_sync", crate::supervisor::Policy::restart(), move || {
        let chat = chat.clone();
        let p2p = p2p.clone();
        async move {
            let mut sent = load_sent();
            let mut sent_at: HashMap<String, Instant> = HashMap::new();
            let mut last_cleanup: Option<Instant> = None;
            loop {
                WAKE.notified().await;
                tokio::time::sleep(Duration::from_millis(400)).await; // слить пачку событий в одну сверку
                let full = DIRTY.swap(false, std::sync::atomic::Ordering::SeqCst);
                let seen: HashSet<[u8; 32]> = std::mem::take(&mut *SEEN.lock().unwrap_or_else(|e| e.into_inner())).into_iter().collect();
                if !full && seen.is_empty() {
                    continue;
                }
                let (hash, nick) = my_state();
                let contacts = contact_shorts();
                let mut changed = false;
                for peer in p2p.list_peers().await {
                    let short = hex::encode(&peer.id.0[..8]);
                    if !contacts.contains(&short) || (!full && !seen.contains(&peer.id.0)) {
                        continue;
                    }
                    // подтверждения доставки нет: давно не отправленный профиль (и первая встреча после старта) уходит заново целиком
                    let prev = if seen.contains(&peer.id.0) && resend_due(sent_at.get(&short).copied()) { None } else { sent.get(&short) };
                    let texts = plan(prev, &hash, &nick);
                    if texts.is_empty() {
                        continue;
                    }
                    let mut ok = true;
                    for t in texts {
                        if chat.send_service(peer.id, t).await.is_err() {
                            ok = false;
                            break;
                        }
                    }
                    if ok {
                        sent_at.insert(short.clone(), Instant::now());
                        sent.insert(short, (hash.clone(), nick.clone()));
                        changed = true;
                    }
                }
                let before = sent.len();
                sent.retain(|k, _| contacts.contains(k));
                if changed || sent.len() != before {
                    save_sent(&sent);
                }
                if last_cleanup.map_or(true, |t| t.elapsed() > Duration::from_secs(3600)) {
                    cleanup_now(); // раз в час, по случаю
                    last_cleanup = Some(Instant::now());
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(color: u8) -> Vec<u8> {
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(32, 32, image::Rgba([color, 10, 10, 255])))
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn ava(p: &[u8]) -> String {
        use base64::Engine;
        format!("{A}{}", serde_json::json!({"h": sha(p), "png": base64::engine::general_purpose::STANDARD.encode(p)}))
    }

    #[test]
    fn avatar_flow_requests_then_stores_only_what_was_asked() {
        let d = tempfile::tempdir().unwrap();
        let p = png(200);
        let h = sha(&p);
        // картинку, которую не просили, не берём
        assert!(process(d.path(), "aa11", &ava(&p), true, None).is_empty());
        assert!(peer_avatar_hash(d.path(), "aa11").is_none());
        // объявление → запрос
        let reply = process(d.path(), "aa11", &format!("{H}{{\"h\":\"{h}\"}}"), true, None);
        assert_eq!(reply, vec![format!("{Q}{{\"h\":\"{h}\"}}")]);
        // теперь пришедшая картинка принимается
        process(d.path(), "aa11", &ava(&p), true, None);
        assert_eq!(peer_avatar_hash(d.path(), "aa11").as_deref(), Some(h.as_str()));
        // такая же уже есть — повторно не просим
        assert!(process(d.path(), "aa11", &format!("{H}{{\"h\":\"{h}\"}}"), true, None).is_empty());
        // убрал картинку
        process(d.path(), "aa11", &format!("{H}{{\"h\":\"\"}}"), true, None);
        assert!(peer_avatar_hash(d.path(), "aa11").is_none());
    }

    #[test]
    fn strangers_and_bad_images_are_ignored() {
        let d = tempfile::tempdir().unwrap();
        let p = png(50);
        let h = sha(&p);
        assert!(process(d.path(), "bb22", &format!("{H}{{\"h\":\"{h}\"}}"), false, None).is_empty(), "не контакт");
        // подмена: хэш просили один, пришла другая картинка под тем же хэшем
        process(d.path(), "cc33", &format!("{H}{{\"h\":\"{h}\"}}"), true, None);
        let other = png(51);
        let forged = ava(&other).replacen(&sha(&other), &h, 1);
        process(d.path(), "cc33", &forged, true, None);
        assert!(peer_avatar_hash(d.path(), "cc33").is_none());
        // не PNG
        let junk = vec![1u8; 100];
        let jh = sha(&junk);
        process(d.path(), "dd44", &format!("{H}{{\"h\":\"{jh}\"}}"), true, None);
        process(d.path(), "dd44", &ava(&junk), true, None);
        assert!(peer_avatar_hash(d.path(), "dd44").is_none());
    }

    #[test]
    fn request_for_my_avatar_is_answered_only_for_the_current_one() {
        let d = tempfile::tempdir().unwrap();
        let mine = png(7);
        let h = sha(&mine);
        let r = process(d.path(), "ee55", &format!("{Q}{{\"h\":\"{h}\"}}"), true, Some(&mine));
        assert_eq!(r.len(), 1);
        assert!(r[0].starts_with(A));
        let other = sha(&png(8));
        assert!(process(d.path(), "ee55", &format!("{Q}{{\"h\":\"{other}\"}}"), true, Some(&mine)).is_empty());
    }

    #[test]
    fn nick_is_stored_cleaned_and_removable() {
        let d = tempfile::tempdir().unwrap();
        process(d.path(), "ff66", &format!("{N}{}", serde_json::json!({"n": "  Доктор\u{0}Хаус  "})), true, None);
        assert_eq!(load_nicks(d.path()).get("ff66").map(String::as_str), Some("ДокторХаус"));
        process(d.path(), "ff66", &format!("{N}{{\"n\":\"\"}}"), true, None);
        assert!(load_nicks(d.path()).get("ff66").is_none());
    }

    #[test]
    fn cleanup_forgets_only_old_non_contacts() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path();
        let day = 24 * 3600 * 1000u64;
        let now = now_ms();
        let mut recs = HashMap::new();
        recs.insert("aaaaaaaaaaaaaaaa".to_string(), NickRec { n: "Контакт".into(), t: now - 90 * day });
        recs.insert("bbbbbbbbbbbbbbbb".to_string(), NickRec { n: "Давний".into(), t: now - 40 * day });
        recs.insert("cccccccccccccccc".to_string(), NickRec { n: "Свежий".into(), t: now - 5 * day });
        save_recs(base, &recs);
        let contacts: HashSet<String> = ["aaaaaaaaaaaaaaaa".to_string()].into();
        assert_eq!(cleanup(base, &contacts, "mmmmmmmmmmmmmmmm", now), 1);
        let left = load_nicks(base);
        assert!(left.contains_key("aaaaaaaaaaaaaaaa"), "контакт остаётся при любом возрасте");
        assert!(!left.contains_key("bbbbbbbbbbbbbbbb"), "не контакт старше 30 дней стёрт");
        assert!(left.contains_key("cccccccccccccccc"), "свежий не контакт ещё хранится");
    }

    #[test]
    fn plan_sends_only_changes() {
        assert!(plan(None, "", "").is_empty());
        assert_eq!(plan(None, "h1", "Ник").len(), 2);
        let st = ("h1".to_string(), "Ник".to_string());
        assert!(plan(Some(&st), "h1", "Ник").is_empty());
        assert_eq!(plan(Some(&st), "h2", "Ник").len(), 1);
        assert_eq!(plan(Some(&st), "", "Ник").len(), 1, "картинку убрали — сообщаем пустой хэш");
        assert_eq!(plan(Some(&st), "h1", "").len(), 1);
    }

    #[test]
    fn a_profile_not_sent_for_an_hour_or_since_start_is_sent_again() {
        assert!(resend_due(None), "после старта узла отправка неизвестна");
        assert!(!resend_due(Some(Instant::now())));
        if let Some(old) = Instant::now().checked_sub(RESEND_AFTER + Duration::from_secs(1)) {
            assert!(resend_due(Some(old)));
        }
    }

    #[test]
    fn service_text_is_recognised() {
        assert!(is_service(&format!("{H}{{}}")));
        assert!(is_service(&format!("{N}{{}}")));
        assert!(!is_service("привет"));
        assert!(!is_service("\u{1}yandi-msg:{}"));
    }

    #[test]
    fn events_replace_timers_changed_marks_dirty_and_peers_are_collected_once() {
        DIRTY.store(false, std::sync::atomic::Ordering::SeqCst);
        SEEN.lock().unwrap().clear();
        changed();
        assert!(DIRTY.load(std::sync::atomic::Ordering::SeqCst));
        let id = crate::util::HashId([7u8; 32]);
        peer_seen(&id);
        peer_seen(&id);
        assert_eq!(SEEN.lock().unwrap().iter().filter(|x| **x == [7u8; 32]).count(), 1);
        // разницу плана считает plan(): тот же хэш и ник — ничего слать не надо, новый ник — одно сообщение
        let prev = ("h".to_string(), "Х".to_string());
        assert!(plan(Some(&prev), "h", "Х").is_empty());
        assert_eq!(plan(Some(&prev), "h", "111").len(), 1);
    }
}
