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
    VERSION_MARK.trim_start_matches("YANDI-VERSION-MARK:").to_string()
}

/// Метка версии в самом бинаре: сборщик пакета читает её (`strings`), чтобы версия пакета была в точности версией внутри него,
/// а не числом коммитов на момент упаковки.
pub static VERSION_MARK: &str = concat!("YANDI-VERSION-MARK:", env!("CARGO_PKG_VERSION"), "+g", env!("YANDI_BUILD_COUNT"));

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
    let mut b = reqwest::Client::builder();
    // в проверках (разрешён http) системные прокси не нужны: адреса местные
    if std::env::var("YANDI_UPDATE_ALLOW_HTTP").is_ok_and(|v| v == "1") {
        b = b.no_proxy();
    }
    // без общего срока (пакет оболочки — десятки мегабайт, на медленном канале дольше минуты): оборвать надо молчание, а не размер
    b.read_timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent(concat!("yandi-update/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

/// Только HTTPS (для проверок можно разрешить http переменной `YANDI_UPDATE_ALLOW_HTTP=1`).
fn allowed(url: &str) -> bool {
    allowed_with(url, std::env::var("YANDI_UPDATE_ALLOW_HTTP").is_ok_and(|v| v == "1"))
}

fn allowed_with(url: &str, allow_http: bool) -> bool {
    url.starts_with("https://") || (allow_http && url.starts_with("http://"))
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
    use crate::release::{build_manifest, derive_signing_key, key_id, manifest_bytes, sign};
    use axum::{routing::get, Router};
    use std::collections::HashMap;
    use std::sync::Arc;

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
        assert!(allowed_with("https://yandi.su/update/deb/MANIFEST.json", false));
        assert!(!allowed_with("http://yandi.su/update/deb/MANIFEST.json", false));
        assert!(allowed_with("http://127.0.0.1:1/", true));
        assert!(!allowed_with("ftp://x/y", true));
    }

    fn key() -> ed25519_dalek::SigningKey {
        derive_signing_key(
            "один два три четыре пять шесть семь восемь девять десять одиннадцать двенадцать",
            argon2::Params::new(64, 1, 1, Some(32)).unwrap(),
        )
    }

    /// Каталог выпуска: пакет + подписанный манифест (версия, номер, срок).
    fn release_dir(version: &str, seq: u64, expires: u64, body: &[u8]) -> HashMap<String, Vec<u8>> {
        let dir = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(dir.into_path()).unwrap();
        std::fs::write(dir.join("yandi-node.deb"), body).unwrap();
        let k = key();
        let mut m = build_manifest(&dir, version, seq, 1_000, &key_id(&k.verifying_key()), &["yandi-node.deb".into()]).unwrap();
        m.platform = "deb".into();
        m.expires = expires;
        let bytes = manifest_bytes(&m);
        let sig = sign(&bytes, &k).unwrap();
        let mut files = HashMap::new();
        files.insert("MANIFEST.json".to_string(), bytes);
        files.insert("MANIFEST.json.sig".to_string(), sig.into_bytes());
        files.insert("yandi-node.deb".to_string(), body.to_vec());
        files
    }

    /// Местный HTTP-«сайт»: отдаёт файлы по именам; возвращает базовый адрес.
    async fn site(files: HashMap<String, Vec<u8>>) -> String {
        let files = Arc::new(files);
        let app = Router::new().route(
            "/*name",
            get(move |axum::extract::Path(name): axum::extract::Path<String>| {
                let files = files.clone();
                async move {
                    match files.get(name.trim_start_matches('/')) {
                        Some(b) => Ok(b.clone()),
                        None => Err(axum::http::StatusCode::NOT_FOUND),
                    }
                }
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::task::spawn(async move {
            let _ = axum::serve(l, app).await;
        });
        std::env::set_var("YANDI_UPDATE_ALLOW_HTTP", "1");
        format!("http://{addr}/")
    }

    fn ch(name: &str, base: &str) -> Channel {
        Channel { name: name.into(), base: base.into() }
    }

    #[tokio::test]
    async fn the_newest_correctly_signed_manifest_wins_and_a_dead_channel_is_only_reported() {
        let vk = key().verifying_key();
        let old = site(release_dir("0.2.0+g100", 3, 0, b"old package")).await;
        let new = site(release_dir("0.2.0+g200", 5, 0, b"new package")).await;
        let dead = ch("dead", "http://127.0.0.1:9/");
        let (found, errors) = check(&[dead, ch("old", &old), ch("new", &new)], &vk, 0, 10).await;
        let found = found.unwrap_or_else(|| panic!("манифест не найден: {errors:?}"));
        assert_eq!(found.manifest.seq, 5);
        assert_eq!(found.channel.name, "new");
        assert_eq!(errors.len(), 1, "недоступный канал виден в ошибках: {errors:?}");
        assert!(is_newer(&found.manifest.version, "0.2.0+g150"));
    }

    #[tokio::test]
    async fn a_forged_expired_or_rolled_back_manifest_is_refused() {
        let vk = key().verifying_key();
        // подделка: манифест правильный, подпись чужая
        let mut forged = release_dir("0.2.0+g300", 9, 0, b"evil");
        use ed25519_dalek::Signer;
        let other = derive_signing_key(
            "другой ключ совсем иной набор слов для подписи один два три четыре пять шесть",
            argon2::Params::new(64, 1, 1, Some(32)).unwrap(),
        );
        let sig = other.sign(&[b"x".as_slice(), &forged["MANIFEST.json"]].concat());
        forged.insert("MANIFEST.json.sig".into(), hex::encode(sig.to_bytes()).into_bytes());
        let b_forged = site(forged).await;
        // просрочен
        let b_expired = site(release_dir("0.2.0+g301", 10, 5, b"stale")).await;
        // откат: номер меньше уже виденного
        let b_old = site(release_dir("0.2.0+g50", 2, 0, b"old")).await;
        for (name, base) in [("forged", b_forged), ("expired", b_expired), ("rollback", b_old)] {
            let (found, errors) = check(&[ch(name, &base)], &vk, 4, 100).await;
            assert!(found.is_none(), "{name}: должен быть отвергнут");
            assert_eq!(errors.len(), 1, "{name}");
        }
    }

    #[tokio::test]
    async fn a_package_that_does_not_match_the_manifest_is_taken_from_the_other_channel() {
        let vk = key().verifying_key();
        let good = release_dir("0.2.0+g200", 5, 0, b"the real package");
        let mut tampered = good.clone();
        tampered.insert("yandi-node.deb".into(), b"the real package but EVIL".to_vec());
        let b_bad = site(tampered).await;
        let b_good = site(good).await;
        let channels = [ch("bad", &b_bad), ch("good", &b_good)];
        let (found, _) = check(&channels, &vk, 0, 10).await;
        let found = found.unwrap();
        assert_eq!(found.channel.name, "bad", "первый канал дал верно подписанный манифест");
        let out = tempfile::tempdir().unwrap();
        let names = download(&found, &channels, out.path()).await.expect("скачано со второго канала");
        assert_eq!(names, vec!["yandi-node.deb".to_string()]);
        assert_eq!(std::fs::read(out.path().join("yandi-node.deb")).unwrap(), b"the real package");
    }
}
