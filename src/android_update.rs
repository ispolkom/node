//! Сообщение телефонам об обновлении приложения. Узел сам читает подписанный манифест Android (папка `update/android/` на yandi.su — та же,
//! из которой скачивают APK вручную), проверяет подпись ключом выпусков, платформу, срок и имя файла, и отдаёт телефону только вывод:
//! есть ли версия новее его собственной. Телефон на сайт не ходит (его адрес сайту не виден), сам APK не ставится: человек скачивает его
//! со страницы загрузки, а подпись самого APK проверяет Android.
use crate::update_check::{check, Channel};
use serde_json::{json, Value};
use std::sync::Mutex;

/// Страница загрузки, куда ведёт сообщение (ссылка в нём одна и та же, не из чужих данных).
pub const DOWNLOAD_PAGE: &str = "https://yandi.su/ru/network/downloads/android/";
const APK_NAME: &str = "yandi.apk";
const PUBKEY_HEX: &str = include_str!("../release/yandi-release-pubkey.hex");

static AVAILABLE: Mutex<Option<Value>> = Mutex::new(None);

fn channels() -> Vec<Channel> {
    vec![Channel { name: "yandi.su".into(), base: "https://yandi.su/update/android/".into() }]
}

/// `A.B.C-bN` (версия приложения и сборка) → (A, B, C, N).
pub fn parse_app_version(v: &str) -> Option<(u64, u64, u64, u64)> {
    let (core, build) = match v.split_once("-b") {
        Some((c, b)) => (c, b.parse::<u64>().ok()?),
        None => (v, 0),
    };
    let mut it = core.split('.');
    let t = (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?, build);
    if it.next().is_some() {
        return None;
    }
    Some(t)
}

/// Подходит ли манифест как выпуск Android: платформа и единственный файл `yandi.apk` с допустимыми размером и суммой.
fn acceptable(m: &crate::release::Manifest) -> Option<Value> {
    if m.platform != "android" || m.files.len() != 1 || m.files[0].name != APK_NAME {
        return None;
    }
    let f = &m.files[0];
    if f.size == 0 || f.size > crate::update_check::MAX_DOWNLOAD || !crate::release::lower_hex(&f.sha256, 64) {
        return None;
    }
    Some(json!({"version": m.version, "seq": m.seq, "notes": m.notes, "size": f.size, "sha256": f.sha256}))
}

/// Опросить канал и запомнить самый новый проверенный выпуск (при ошибке прежнее известное остаётся).
pub async fn refresh() {
    let hex_s = PUBKEY_HEX.trim();
    let Some(vk) = (crate::release::lower_hex(hex_s, 64))
        .then(|| hex::decode(hex_s).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()).and_then(|b| ed25519_dalek::VerifyingKey::from_bytes(&b).ok()))
        .flatten()
    else {
        return;
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (found, _errors) = check(&channels(), &vk, 0, now).await;
    if let Some(f) = found {
        if let Some(v) = acceptable(&f.manifest) {
            let mut g = AVAILABLE.lock().unwrap_or_else(|e| e.into_inner());
            // откат на более старый подписанный выпуск не принимается
            if g.as_ref().is_none_or(|old| old["seq"].as_u64().unwrap_or(0) <= f.manifest.seq) {
                *g = Some(v);
            }
        }
    }
}

/// Ответ телефону: есть ли версия новее `own` (версия приложения вида `1.0.0-b3`).
pub fn answer(own: &str) -> Value {
    let Some(mine) = parse_app_version(own) else {
        return json!({"available": null});
    };
    let latest = AVAILABLE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match latest {
        Some(v) if v["version"].as_str().and_then(parse_app_version).is_some_and(|theirs| theirs > mine) => {
            json!({"available": {"version": v["version"], "notes": v["notes"], "size": v["size"], "page": DOWNLOAD_PAGE}})
        }
        _ => json!({"available": null}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_versions_compare_by_number_then_build() {
        assert_eq!(parse_app_version("1.0.0-b3"), Some((1, 0, 0, 3)));
        assert!(parse_app_version("1.0.0-b4") > parse_app_version("1.0.0-b3"));
        assert!(parse_app_version("1.1.0-b1") > parse_app_version("1.0.9-b9"));
        assert_eq!(parse_app_version("1.0"), None);
        assert_eq!(parse_app_version("x.y.z"), None);
        assert_eq!(parse_app_version("1.0.0-bq"), None);
    }

    fn manifest(platform: &str, name: &str) -> crate::release::Manifest {
        crate::release::Manifest {
            format: 1,
            product: "yandi-node".into(),
            version: "1.0.0-b5".into(),
            seq: 40,
            created: 1,
            key_id: String::new(),
            files: vec![crate::release::FileEntry { name: name.into(), size: 10, sha256: "a".repeat(64) }],
            platform: platform.into(),
            expires: 0,
            notes: "n".into(),
        }
    }

    #[test]
    fn only_an_android_manifest_with_the_one_apk_is_accepted() {
        assert!(acceptable(&manifest("android", "yandi.apk")).is_some());
        assert!(acceptable(&manifest("deb", "yandi.apk")).is_none(), "пакет узла не выдаётся за приложение");
        assert!(acceptable(&manifest("android", "evil.apk")).is_none());
    }

    #[test]
    fn the_phone_is_told_only_about_a_newer_version_and_gets_our_own_page() {
        let _g = crate::complaints::tests::SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        *AVAILABLE.lock().unwrap() = acceptable(&manifest("android", "yandi.apk"));
        assert!(answer("1.0.0-b4")["available"]["version"] == "1.0.0-b5");
        assert_eq!(answer("1.0.0-b4")["available"]["page"], DOWNLOAD_PAGE);
        assert!(answer("1.0.0-b5")["available"].is_null());
        assert!(answer("2.0.0-b1")["available"].is_null());
        assert!(answer("мусор")["available"].is_null());
        *AVAILABLE.lock().unwrap() = None;
    }
}
