//! Проверка обновлений: подписанные манифесты по нескольким каналам (сайт и GitHub), выбор самого нового с верной подписью,
//! скачивание файлов с проверкой SHA-256. Подпись лежит в самом манифесте, поэтому канал доставки недоверенный: подмена файла на
//! одном из каналов не проходит проверку, а недоступность одного канала просто означает, что берётся другой.
//!
//! Адрес файла = `base` + имя из манифеста (для GitHub `base` оканчивается префиксом ассетов, например `.../download/deb-`).
use crate::release::{lower_hex, verify_manifest, Manifest, MAX_MANIFEST, MANIFEST_NAME, SIGNATURE_NAME};
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// Предел размера одного скачиваемого файла.
pub const MAX_DOWNLOAD: u64 = 300 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Channel {
    pub name: String,
    pub base: String,
}

/// Каналы по умолчанию для `.deb`: сайт и GitHub Releases.
pub fn default_channels() -> Vec<Channel> {
    vec![
        Channel { name: "yandi.su".into(), base: "https://yandi.su/update/deb/".into() },
        Channel { name: "github".into(), base: "https://github.com/ispolkom/node/releases/latest/download/deb-".into() },
    ]
}

/// Версия этого бинаря: `<Cargo>+g<число коммитов>` (число подставляет build.rs; 0 — сборка вне git).
pub fn own_version() -> String {
    format!("{}+g{}", env!("CARGO_PKG_VERSION"), env!("YANDI_BUILD_COUNT"))
}

/// `A.B.C+gN` → (A, B, C, N); нераспознанное — None.
fn parse_version(v: &str) -> Option<(u64, u64, u64, u64)> {
    let (core, build) = match v.split_once("+g") {
        Some((c, b)) => (c, b.parse::<u64>().ok()?),
        None => (v, 0),
    };
    let mut it = core.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((a, b, c, build))
}

/// Строго ли `candidate` новее `own` (нераспознанная версия новее не считается).
pub fn is_newer(candidate: &str, own: &str) -> bool {
    match (parse_version(candidate), parse_version(own)) {
        (Some(c), Some(o)) => c > o,
        _ => false,
    }
}

#[derive(Debug, Clone)]
pub struct Found {
    pub manifest: Manifest,
    pub channel: Channel,
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent(concat!("yandi-update/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

/// Только HTTPS (для проверок можно разрешить http переменной `YANDI_UPDATE_ALLOW_HTTP=1`).
fn allowed(url: &str) -> bool {
    url.starts_with("https://") || (url.starts_with("http://") && std::env::var("YANDI_UPDATE_ALLOW_HTTP").is_ok_and(|v| v == "1"))
}

async fn get_limited(c: &reqwest::Client, url: &str, limit: u64) -> Result<Vec<u8>, String> {
    if !allowed(url) {
        return Err("адрес канала должен быть https".into());
    }
    let mut resp = c.get(url).send().await.map_err(|e| format!("{url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("{url}: код {}", resp.status().as_u16()));
    }
    if resp.content_length().is_some_and(|n| n > limit) {
        return Err(format!("{url}: файл больше допустимого"));
    }
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("{url}: {e}"))? {
        if out.len() as u64 + chunk.len() as u64 > limit {
            return Err(format!("{url}: файл больше допустимого"));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Манифест одного канала: скачан, подпись и срок проверены.
pub async fn fetch_manifest(channel: &Channel, vk: &VerifyingKey, min_seq: u64, now: u64) -> Result<Manifest, String> {
    let c = client()?;
    let bytes = get_limited(&c, &format!("{}{}", channel.base, MANIFEST_NAME), MAX_MANIFEST as u64).await?;
    let sig = get_limited(&c, &format!("{}{}", channel.base, SIGNATURE_NAME), 512).await?;
    let sig = String::from_utf8(sig).map_err(|_| "подпись не текст".to_string())?;
    let m = verify_manifest(&bytes, &sig, vk, min_seq).map_err(|e| e.to_string())?;
    if m.expires != 0 && now > m.expires {
        return Err("срок действия манифеста истёк (свежего подписанного файла нет)".into());
    }
    Ok(m)
}

/// Опросить все каналы; вернуть самый новый (по seq) манифест с верной подписью и ошибки по каналам.
pub async fn check(channels: &[Channel], vk: &VerifyingKey, min_seq: u64, now: u64) -> (Option<Found>, Vec<String>) {
    let mut best: Option<Found> = None;
    let mut errors = Vec::new();
    for ch in channels {
        match fetch_manifest(ch, vk, min_seq, now).await {
            Ok(m) => {
                if best.as_ref().map_or(true, |b| m.seq > b.manifest.seq) {
                    best = Some(Found { manifest: m, channel: ch.clone() });
                }
            }
            Err(e) => errors.push(format!("{}: {e}", ch.name)),
        }
    }
    (best, errors)
}

/// Скачать файлы манифеста в `out` (по каналам по очереди, пока один не отдаст всё с верной суммой).
pub async fn download(found: &Found, channels: &[Channel], out: &Path) -> Result<Vec<String>, String> {
    use sha2::{Digest, Sha256};
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let c = client()?;
    // сначала канал, давший манифест, потом остальные
    let mut order: Vec<&Channel> = vec![&found.channel];
    order.extend(channels.iter().filter(|x| **x != found.channel));
    let mut names = Vec::new();
    for f in &found.manifest.files {
        if f.size > MAX_DOWNLOAD || !lower_hex(&f.sha256, 64) {
            return Err(format!("{}: недопустимые размер или сумма в манифесте", f.name));
        }
        let mut done = false;
        let mut last = String::new();
        for ch in &order {
            let url = format!("{}{}", ch.base, f.name);
            match get_limited(&c, &url, f.size.max(1)).await {
                Ok(bytes) => {
                    if bytes.len() as u64 != f.size || hex::encode(Sha256::digest(&bytes)) != f.sha256 {
                        last = format!("{}: содержимое не совпало с манифестом", ch.name);
                        continue;
                    }
                    let tmp = out.join(format!("{}.part", f.name));
                    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
                    std::fs::rename(&tmp, out.join(&f.name)).map_err(|e| e.to_string())?;
                    done = true;
                    break;
                }
                Err(e) => last = e,
            }
        }
        if !done {
            return Err(format!("{}: не удалось скачать ({last})", f.name));
        }
        names.push(f.name.clone());
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_core_then_build_count() {
        assert!(is_newer("0.2.1+g10", "0.2.0+g999"));
        assert!(is_newer("0.2.0+g481", "0.2.0+g480"));
        assert!(!is_newer("0.2.0+g480", "0.2.0+g480"));
        assert!(!is_newer("0.1.9+g999", "0.2.0+g1"));
        assert!(!is_newer("мусор", "0.2.0+g1"), "нераспознанная версия новее не считается");
        assert!(is_newer("0.3.0", "0.2.0+g5"));
    }

    #[test]
    fn channels_must_be_https_unless_the_test_switch_is_set() {
        assert!(allowed("https://yandi.su/update/deb/MANIFEST.json"));
        assert!(!allowed("http://yandi.su/update/deb/MANIFEST.json"));
        assert!(!allowed("ftp://x/y"));
    }
}
