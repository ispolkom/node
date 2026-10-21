//! Каталог «Узлы» для страницы поиска (docs/ROADMAP_R9.md §3): только узлы, которые этот узел сейчас видит в сети.
//! Без адресов, ключей и IP: короткий номер, регион (если узел его объявил или он есть в подписанном списке) и роль.
//! Гость видит только узлы из подписанного списка с ролью `guest`. У автономного узла здесь только его меш.
use serde_json::{json, Value};

/// Строки каталога для онлайн-узлов `online` (полные номера). `guest_view` — смотрит гость.
pub fn rows(online: &[[u8; 32]], me: &[u8; 32], guest_view: bool) -> Vec<Value> {
    let now = crate::network_offers::now_secs();
    let doc = crate::bootstrap::load_cached(crate::bootstrap::SIGNER_KEYS, now);
    let mut out: Vec<Value> = Vec::new();
    for id in online {
        if id == me {
            continue;
        }
        let hex_id = hex::encode(id);
        let entry = doc.as_ref().and_then(|d| d.entries.iter().find(|e| e.id.eq_ignore_ascii_case(&hex_id)));
        let roles: Vec<String> = entry.map(|e| e.roles.clone()).unwrap_or_default();
        if guest_view && !roles.iter().any(|r| r == "guest") {
            continue;
        }
        let region = crate::network_offers::offer_of(&hex_id)
            .and_then(|o| o.country)
            .or_else(|| entry.and_then(|e| e.region.clone()))
            .unwrap_or_default();
        let role = if crate::autonomy::enabled() {
            "mesh".to_string()
        } else if roles.is_empty() {
            "node".to_string()
        } else {
            roles.join(",")
        };
        out.push(json!({ "short_id": &hex_id[..16], "region": region, "role": role, "online": true }));
    }
    out.sort_by(|a, b| (a["region"].as_str(), a["short_id"].as_str()).cmp(&(b["region"].as_str(), b["short_id"].as_str())));
    out.dedup_by(|a, b| a["short_id"] == b["short_id"]);
    out
}
