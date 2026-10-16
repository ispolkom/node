//! Смайлы-реакции личных чатов (без монеток): хранятся на узле у страницы ПК; тот же конверт, что у приложения —
//! `\u0001yandi-rx:{"m": номер сообщения, "e": смайл}`; пустой смайл снимает реакцию.
//!
//! Номер сообщения — так, как его знают обе стороны: у компьютера это msg_id (hex), а у телефона — cmid. Для чата с телефоном владельца
//! номер чужого сообщения узел запоминает при его приёме (`remember_rid`): иначе по нему нельзя было бы ответить смайлом.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const MARKER: &str = "\u{1}yandi-rx:";
/// Тот же набор, что в приложении (`MsgChannel.reactionSet`).
pub const SET: [&str; 6] = ["👍", "👎", "😊", "😢", "😂", "❤️"];
const MAX_PER_PEER: usize = 2000;
const MAX_RIDS: usize = 5000;

#[derive(Serialize, Deserialize, Default, Clone)]
struct Rx {
    #[serde(default)]
    me: String,
    #[serde(default)]
    peer: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Data {
    /// короткий номер собеседника → номер сообщения (hex) → реакции
    by_peer: HashMap<String, HashMap<String, Rx>>,
    /// номер входящего сообщения на узле (hex) → его номер (cmid) у телефона
    rids: HashMap<String, String>,
}

static STORE: crate::util::sealed_json::SealedJson<Data> =
    crate::util::sealed_json::SealedJson::new("chat_reactions.json", "chat_reactions.enc", "chat_reactions");

fn with<R>(f: impl FnOnce(&mut Data) -> R) -> R {
    STORE.with(f)
}

/// Закрытая среда: ключ области выдан / среда заперта / среда стёрта.
pub fn unlock(key: [u8; 32]) {
    STORE.unlock(key)
}
pub fn lock() {
    STORE.lock()
}
pub fn erase() {
    STORE.erase()
}

/// Переписка с собеседником удалена: его реакции и соответствия номеров больше не хранятся.
pub fn forget_peer(peer_short: &str) {
    with(|d| {
        d.by_peer.remove(peer_short);
    });
}

/// Вся переписка удалена.
pub fn forget_all() {
    with(|d| *d = Data::default());
}

pub fn envelope(msg_key: &str, emoji: &str) -> String {
    format!("{MARKER}{}", serde_json::json!({"m": msg_key, "e": emoji}))
}

/// Разбор конверта: (номер сообщения, смайл) — пустой смайл снимает реакцию; неизвестный смайл и мусор отвергаются.
pub fn parse(text: &str) -> Option<(String, String)> {
    let j: serde_json::Value = serde_json::from_str(text.strip_prefix(MARKER)?).ok()?;
    let (m, e) = (j["m"].as_str()?, j["e"].as_str()?);
    if m.is_empty() || m.len() > 128 || (!e.is_empty() && !SET.contains(&e)) {
        return None;
    }
    Some((m.to_string(), e.to_string()))
}

fn set(d: &mut Data, peer: &str, msg: &str, emoji: &str, mine: bool) {
    let map = d.by_peer.entry(peer.to_string()).or_default();
    if !map.contains_key(msg) && map.len() >= MAX_PER_PEER {
        return;
    }
    let r = map.entry(msg.to_string()).or_default();
    if mine {
        r.me = emoji.to_string();
    } else {
        r.peer = emoji.to_string();
    }
    if r.me.is_empty() && r.peer.is_empty() {
        map.remove(msg);
    }
}

/// Собеседник поставил (или снял) смайл на сообщение [msg] (hex номера на узле).
pub fn set_peer(peer_short: &str, msg: &str, emoji: &str) {
    with(|d| set(d, peer_short, msg, emoji, false));
}

/// Я поставил (или снял) смайл.
pub fn set_me(peer_short: &str, msg: &str, emoji: &str) {
    with(|d| set(d, peer_short, msg, emoji, true));
}

/// Реакции переписки: номер сообщения → (мой, собеседника).
pub fn of_peer(peer_short: &str) -> HashMap<String, (String, String)> {
    with(|d| d.by_peer.get(peer_short).map(|m| m.iter().map(|(k, v)| (k.clone(), (v.me.clone(), v.peer.clone()))).collect()).unwrap_or_default())
}

pub fn remember_rid(local_hex: &str, cmid: &str) {
    if cmid.is_empty() || cmid.len() > 128 {
        return;
    }
    with(|d| {
        if d.rids.len() >= MAX_RIDS {
            let drop: Vec<String> = d.rids.keys().take(d.rids.len() - MAX_RIDS + 1).cloned().collect();
            for k in drop {
                d.rids.remove(&k);
            }
        }
        d.rids.insert(local_hex.to_string(), cmid.to_string());
    });
}

/// Номер, под которым собеседник знает моё сообщение или его собственное: для чужого — запомненный cmid, иначе тот же hex.
pub fn key_for_peer(local_hex: &str) -> String {
    with(|d| d.rids.get(local_hex).cloned()).unwrap_or_else(|| local_hex.to_string())
}

/// Телефон прислал реакцию на сообщение [cmid]: 64 hex — это сообщение компьютера (cmid = его номер), иначе — собственное сообщение
/// телефона, под выведенным номером.
pub fn local_for_phone(device: &str, cmid: &str) -> String {
    if cmid.len() == 64 && cmid.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        cmid.to_string()
    } else {
        hex::encode(crate::mobile_self::incoming_id(device, cmid).0)
    }
}

/// Входящее сообщение от узла-собеседника: реакция разбирается здесь (в переписке она не показывается).
pub fn on_incoming(from: &crate::util::HashId, text: &str) {
    if !text.starts_with(MARKER) {
        return;
    }
    if let Some((m, e)) = parse(text) {
        set_peer(&hex::encode(&from.0[..8]), &m, &e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_roundtrip_and_validation() {
        assert_eq!(parse(&envelope("ab", "👍")), Some(("ab".into(), "👍".into())));
        assert_eq!(parse(&envelope("ab", "")), Some(("ab".into(), "".into())));
        assert!(parse(&envelope("ab", "🦄")).is_none());
        assert!(parse(&envelope("", "👍")).is_none());
        assert!(parse("\u{1}yandi-rx:не json").is_none());
        assert!(parse("привет").is_none());
    }

    #[test]
    fn each_side_keeps_one_reaction_and_empty_removes_the_record() {
        let mut d = Data::default();
        set(&mut d, "p", "m1", "👍", true);
        set(&mut d, "p", "m1", "😊", false);
        set(&mut d, "p", "m1", "❤️", true);
        let r = d.by_peer["p"]["m1"].clone();
        assert_eq!((r.me.as_str(), r.peer.as_str()), ("❤️", "😊"));
        set(&mut d, "p", "m1", "", true);
        set(&mut d, "p", "m1", "", false);
        assert!(d.by_peer["p"].is_empty());
    }

    #[test]
    fn phone_reaction_targets_pc_message_or_own_derived_id() {
        let pc_msg = "a".repeat(64);
        assert_eq!(local_for_phone("dev", &pc_msg), pc_msg);
        let own = local_for_phone("dev", "17ab-c");
        assert_eq!(own.len(), 64);
        assert_ne!(own, "17ab-c");
    }

    #[test]
    fn reactions_do_not_outlive_the_history_they_belong_to() {
        set_peer("aaaa1111aaaa1111", "m1", "👍");
        set_peer("bbbb2222bbbb2222", "m2", "😂");
        forget_peer("aaaa1111aaaa1111");
        assert!(of_peer("aaaa1111aaaa1111").is_empty());
        assert_eq!(of_peer("bbbb2222bbbb2222").len(), 1);
        forget_all();
        assert!(of_peer("bbbb2222bbbb2222").is_empty());
    }
}
