//! Единый профиль пользователя (ник и аватар): хранится на узле, один на ПК и все привязанные телефоны.
//! Телефон подтягивает его при подключении и может поменять — побеждает последняя правка (по `updated_ms`).
//! Картинка проходит ту же обработку, что и аватар с ПК: пересборка из пикселей, без метаданных.

use crate::core::profile::UserProfile;
use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Ник — до 40 знаков, без управляющих символов.
pub const MAX_NICK: usize = 40;
/// Картинка от телефона до 64 КБ (после обработки она будет ~10 КБ).
pub const MAX_AVATAR_IN: usize = 64 * 1024;

static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn avatar_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".yandi/avatars")
}

fn clean_nick(raw: &str) -> String {
    let s: String = raw.chars().filter(|c| !c.is_control()).collect();
    s.trim().chars().take(MAX_NICK).collect()
}

/// Файл аватара своего узла: (путь, тип). Ищется в том же порядке, что и в веб-интерфейсе.
fn avatar_file(short: &str) -> Option<(PathBuf, &'static str)> {
    for (ext, ty) in [("png", "image/png"), ("jpg", "image/jpeg"), ("gif", "image/gif")] {
        let p = avatar_dir().join(format!("{short}.{ext}"));
        if p.exists() {
            return Some((p, ty));
        }
    }
    None
}

fn remove_avatars(short: &str) {
    for ext in ["png", "jpg", "gif"] {
        let _ = std::fs::remove_file(avatar_dir().join(format!("{short}.{ext}")));
    }
}

/// Текущий профиль: ник (пусто — не задан), хэш аватара (пусто — нет) и версия.
pub fn snapshot(short: &str) -> Value {
    let (nick, ms) = match UserProfile::load() {
        Ok(Some(p)) => {
            let nick = if p.display_name == p.short_id || p.display_name == short { String::new() } else { p.display_name };
            (nick, p.updated_ms.max(p.updated_at.saturating_mul(1000)))
        }
        _ => (String::new(), 0),
    };
    let hash = avatar_file(short)
        .and_then(|(p, _)| std::fs::read(p).ok())
        .map(|b| hex::encode(Sha256::digest(&b)))
        .unwrap_or_default();
    json!({"nick": nick, "avatar_hash": hash, "updated_ms": ms})
}

#[derive(Deserialize, Default)]
pub struct PutReq {
    /// новый ник; не указан — не меняется
    #[serde(default)]
    pub nick: Option<String>,
    /// новая картинка (base64); пустая строка — убрать; не указана — не меняется
    #[serde(default)]
    pub avatar_b64: Option<String>,
}

/// Применить правку профиля. Ошибка — готовая причина для телефона.
pub fn apply(short: &str, req: &PutReq) -> Result<Value, (StatusCode, String)> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let bad = |m: &str| (StatusCode::BAD_REQUEST, m.to_string());
    let nick = match &req.nick {
        Some(n) => {
            let n = clean_nick(n);
            if n.is_empty() {
                return Err(bad("ник не может быть пустым"));
            }
            Some(n)
        }
        None => None,
    };
    // картинку проверяем и обрабатываем до любых записей
    let image: Option<Option<Vec<u8>>> = match req.avatar_b64.as_deref() {
        None => None,
        Some("") => Some(None),
        Some(b64) => {
            let raw = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|_| bad("картинка не в base64"))?;
            if raw.is_empty() || raw.len() > MAX_AVATAR_IN {
                return Err(bad("картинка слишком большая"));
            }
            let out = sanitize_avatar(&raw).ok_or_else(|| bad("это не картинка или она слишком большая"))?;
            Some(Some(out))
        }
    };
    let mut profile = match UserProfile::load() {
        Ok(Some(p)) => p,
        _ => UserProfile::new(short.to_string()),
    };
    if let Some(img) = &image {
        match img {
            Some(bytes) => {
                let dir = avatar_dir();
                std::fs::create_dir_all(&dir).map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "не удалось сохранить картинку".into()))?;
                remove_avatars(short);
                std::fs::write(dir.join(format!("{short}.png")), bytes)
                    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "не удалось сохранить картинку".into()))?;
                profile.update(nick.clone(), Some(format!("/api/avatar/{short}")));
            }
            None => {
                remove_avatars(short);
                profile.update(nick.clone(), Some(String::new()));
            }
        }
    } else {
        profile.update(nick.clone(), None);
    }
    profile.save().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "не удалось сохранить профиль".into()))?;
    Ok(snapshot(short))
}

/// Картинка, пересобранная из пикселей (метаданных нет), PNG до 40 КБ: тот же предел, что у сообщения-аватара на телефоне.
pub const MAX_AVATAR_OUT: usize = 40 * 1024;

pub fn sanitize_avatar(raw: &[u8]) -> Option<Vec<u8>> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(raw)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(96 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    for side in [128u32, 96, 64] {
        let small = image::DynamicImage::ImageRgba8(img.thumbnail(side, side).to_rgba8());
        let mut out = Vec::new();
        small.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
        if out.len() <= MAX_AVATAR_OUT {
            return Some(out);
        }
    }
    None
}

pub fn avatar_bytes(short: &str) -> Option<(Vec<u8>, &'static str)> {
    let (p, ty) = avatar_file(short)?;
    std::fs::read(p).ok().map(|b| (b, ty))
}

pub fn reply_json(r: Result<Value, (StatusCode, String)>) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err((c, m)) => (c, Json(json!({"error": m}))).into_response(),
    }
}

pub fn reply_avatar(a: Option<(Vec<u8>, &'static str)>) -> Response {
    match a {
        Some((b, ty)) => ([(header::CONTENT_TYPE, ty)], b).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avatar_is_rebuilt_from_pixels_and_bounded() {
        let mut src = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(400, 300, image::Rgba([10, 120, 30, 255])))
            .write_to(&mut std::io::Cursor::new(&mut src), image::ImageFormat::Png)
            .unwrap();
        let out = sanitize_avatar(&src).unwrap();
        assert!(out.len() <= MAX_AVATAR_OUT);
        assert_eq!(&out[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        let back = image::load_from_memory(&out).unwrap();
        assert!(back.width() <= 128 && back.height() <= 128);
        assert!(sanitize_avatar(b"not an image").is_none());
        // чужая «картинка» с PNG-подписью, но без данных, тоже отвергается
        assert!(sanitize_avatar(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]).is_none());
    }

    #[test]
    fn nick_is_cleaned_and_bounded() {
        assert_eq!(clean_nick("  Доктор\u{0}\nХаус  "), "ДокторХаус");
        assert_eq!(clean_nick(&"я".repeat(100)).chars().count(), MAX_NICK);
        assert_eq!(clean_nick(" \t "), "");
    }
}
