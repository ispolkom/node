//! Локальные имя и аватар контакта: то, что Я задал человеку у себя и что видно только мне.
//!
//! Две разные вещи:
//! * глобальное — ник и аватар, которые человек задал себе (профиль устройства, визитка в группе); их видят все, кто его знает;
//! * локальное — как Я его назвал и какую картинку ему поставил; никому не отправляется. Локальное главнее глобального, но только там,
//!   где человек опознан как мой контакт; у остальных виден его собственный ник и аватар.
//!
//! Хранится в закрытой среде (SealedJson): без пароля владельца не читается. Ключ — первые 16 hex-знаков номера человека: он и
//! телефона (64 знака), и контакта (16 знаков) одинаков.

use crate::util::sealed_json::SealedJson;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct Override {
    #[serde(default)]
    pub name: String,
    /// PNG до 12 КБ (как визитка в группе), base64
    #[serde(default)]
    pub avatar_b64: String,
}

type Map = HashMap<String, Override>;

static STORE: SealedJson<Map> = SealedJson::new("local_overrides.json", "local_overrides.enc", "local_overrides");
const MAX_ENTRIES: usize = 5000;

pub fn unlock(key: [u8; 32]) {
    STORE.unlock(key)
}
pub fn lock() {
    STORE.lock()
}
pub fn erase() {
    STORE.erase()
}

/// Ключ человека: первые 16 hex-знаков номера (в нижнем регистре); не номер — `None`.
pub fn key(id: &str) -> Option<String> {
    let id = id.trim().to_ascii_lowercase();
    (id.len() >= 16 && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_hexdigit())).then(|| id[..16].to_string())
}

/// Имя человека: без ссылок и управляющих знаков, один пробел между словами, не длиннее 64 знаков.
pub fn clean_name(raw: &str) -> String {
    let t = crate::content_policy::strip_links(raw).replace(crate::content_policy::REMOVED, "");
    let t: String = t.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    t.chars().take(64).collect::<String>().trim().to_string()
}

/// Как показать человека: локальное имя (я задал), иначе глобальное (он себе), иначе запасное (имя устройства).
pub fn display_name(local: &str, global: &str, fallback: &str) -> String {
    [local, global].iter().find(|n| !n.trim().is_empty()).map(|n| n.to_string()).unwrap_or_else(|| fallback.to_string())
}

/// Локальное имя, если задано.
pub fn name(id: &str) -> Option<String> {
    let k = key(id)?;
    STORE.with(|m| m.get(&k).map(|o| o.name.clone()).filter(|n| !n.is_empty()))
}

/// Локальная картинка (PNG), если задана.
pub fn avatar(id: &str) -> Option<Vec<u8>> {
    let k = key(id)?;
    let b64 = STORE.with(|m| m.get(&k).map(|o| o.avatar_b64.clone()).filter(|a| !a.is_empty()))?;
    base64::engine::general_purpose::STANDARD.decode(b64).ok()
}

pub fn has_avatar(id: &str) -> bool {
    key(id).is_some_and(|k| STORE.with(|m| m.get(&k).is_some_and(|o| !o.avatar_b64.is_empty())))
}

/// Задать локальное имя и/или картинку. `Some("")` — снять; `None` — не менять. Картинка перерисовывается (как визитка в группе):
/// чужие метаданные и необычные форматы не сохраняются.
pub fn set(id: &str, name: Option<&str>, avatar_b64: Option<&str>) -> Result<(), String> {
    let k = key(id).ok_or("неверный номер человека")?;
    let new_name = match name {
        Some(n) => Some(clean_name(n)),
        None => None,
    };
    let new_avatar = match avatar_b64 {
        Some("") => Some(String::new()),
        Some(b) => {
            if b.len() > 3 * 1024 * 1024 {
                return Err("картинка слишком большая".into());
            }
            let raw = base64::engine::general_purpose::STANDARD.decode(b).map_err(|_| "картинка не разобрана".to_string())?;
            let png = crate::mobile_profile::sanitize_small(&raw).ok_or("это не картинка, либо слишком большая")?;
            Some(base64::engine::general_purpose::STANDARD.encode(png))
        }
        None => None,
    };
    STORE.with(|m| {
        if !m.contains_key(&k) && m.len() >= MAX_ENTRIES {
            return Err("слишком много локальных имён".to_string());
        }
        let e = m.entry(k.clone()).or_default();
        if let Some(n) = new_name {
            e.name = n;
        }
        if let Some(a) = new_avatar {
            e.avatar_b64 = a;
        }
        if e.name.is_empty() && e.avatar_b64.is_empty() {
            m.remove(&k);
        }
        Ok(())
    })
}

/// Человек удалён из контактов: локальные имя и картинка стираются.
pub fn forget(id: &str) {
    if let Some(k) = key(id) {
        STORE.with(|m| {
            m.remove(&k);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> String {
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(8, 8, image::Rgba([200, 30, 30, 255])));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        base64::engine::general_purpose::STANDARD.encode(out)
    }

    #[test]
    fn a_phone_and_a_contact_share_one_key_and_names_are_cleaned() {
        let full = "ab".repeat(32);
        assert_eq!(key(&full), key(&full[..16]));
        assert_eq!(key(&full.to_uppercase()).as_deref(), Some(&full[..16]));
        assert!(key("не номер").is_none() && key("abc").is_none());
        assert_eq!(clean_name("  Кор \n https://example.org/x  Вася\u{7}  "), "Кор Вася");
        assert_eq!(clean_name(&"я".repeat(200)).chars().count(), 64);
    }

    #[test]
    fn local_beats_global_and_global_beats_the_device_name() {
        assert_eq!(display_name("Вася", "Кор", "📱 Телефон 1738"), "Вася", "локальное главнее");
        assert_eq!(display_name("", "Кор", "📱 Телефон 1738"), "Кор", "без локального — ник, который человек задал себе");
        assert_eq!(display_name("  ", "", "📱 Телефон 1738"), "📱 Телефон 1738", "ни того ни другого — имя устройства");
    }

    #[test]
    fn local_name_and_avatar_are_set_cleared_and_kept_separate_from_each_other() {
        let id = "c1".repeat(32);
        assert!(name(&id).is_none() && avatar(&id).is_none());
        set(&id, Some("Вася"), None).unwrap();
        assert_eq!(name(&id).as_deref(), Some("Вася"));
        assert!(!has_avatar(&id));
        set(&id, None, Some(&png())).unwrap();
        assert_eq!(name(&id).as_deref(), Some("Вася"), "имя не тронуто");
        let a = avatar(&id).expect("картинка");
        assert_eq!(&a[1..4], b"PNG");
        set(&id, Some(""), None).unwrap(); // снять имя: картинка остаётся
        assert!(name(&id).is_none() && has_avatar(&id));
        set(&id, None, Some("")).unwrap(); // снять и картинку — запись исчезает
        assert!(!has_avatar(&id) && name(&id).is_none());
        assert!(set(&id, None, Some("не-base64!!")).is_err());
        assert!(set(&id, None, Some(&base64::engine::general_purpose::STANDARD.encode(b"just text"))).is_err(), "не картинка");
        assert!(set("zz", Some("x"), None).is_err());
        set(&id, Some("Удалим"), None).unwrap();
        forget(&id);
        assert!(name(&id).is_none());
    }
}
