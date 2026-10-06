//! Карточки узлов и списки сети по странам на тренировочной сети (`yandi testnet`), на настоящих процессах: каждая копия по самооценке
//! при запуске публикует подписанную карточку, копии обмениваются карточками по основной связи — и через несколько минут каждая знает
//! все три; список страны отдаёт их всех; подпись каждой карточки проверяется на стороне читателя; то, чем узел помогает, обновляется
//! без ожидания получаса. Домашняя папка владельца подменена временной. `#[ignore]` (несколько минут):
//!     cargo test --offline --test testnet_offers_test -- --ignored
#![cfg(target_os = "linux")]
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::{json, Value};

fn yandi(stand: &Path, home: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_yandi")).arg("testnet").args(args).env("YANDI_TESTNET_DIR", stand).env("HOME", home).env_remove("XDG_DATA_HOME").output().unwrap();
    (out.status.code().unwrap_or(-1), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

struct Down(PathBuf, PathBuf);
impl Drop for Down {
    fn drop(&mut self) {
        let _ = yandi(&self.0, &self.1, &["down"]);
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts real node processes"]
async fn every_copy_learns_every_card_and_country_lists_are_built_from_signed_self_assessments() {
    for v in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "ALL_PROXY", "all_proxy"] {
        std::env::remove_var(v);
    }
    let tmp = tempfile::tempdir().unwrap();
    let (stand, home) = (tmp.path().join("stand"), tmp.path().join("home"));
    std::fs::create_dir_all(&home).unwrap();
    let (code, out) = yandi(&stand, &home, &["up", "3"]);
    let _down = Down(stand.clone(), home.clone());
    assert_eq!(code, 0, "{out}");
    let http = reqwest::Client::new();
    let pw = std::fs::read_to_string(stand.join("passwords.txt")).unwrap();
    let login = pw.lines().find_map(|l| l.strip_prefix("пароль входа:")).unwrap().trim().to_string();
    let mut cookies = vec![];
    for k in 1..=3 {
        let r = http.post(format!("http://127.0.0.1:{}/api/auth/login", 26000 + 100 * k)).json(&json!({"login_password": login})).send().await.unwrap();
        cookies.push(r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string());
    }
    let offers = |k: usize, q: &str| {
        let (h, c) = (http.clone(), cookies[k - 1].clone());
        let url = format!("http://127.0.0.1:{}/api/network/offers{q}", 26000 + 100 * k);
        async move { h.get(url).header("cookie", c).send().await.unwrap().json::<Value>().await.unwrap() }
    };
    // все узнают всех
    let mut done = false;
    for _ in 0..60 {
        let mut all = true;
        for k in 1..=3 {
            all &= offers(k, "").await["offers"].as_array().map(|a| a.len()).unwrap_or(0) == 3;
        }
        if all {
            done = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    assert!(done, "every copy learns all three cards");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let v = offers(1, "").await;
    let me = serde_json::from_value::<yandi::network_offers::NodeOffer>(v["me"].clone()).unwrap();
    assert_eq!(me.check(now), Ok(()), "the reader checks the signature");
    assert!(matches!(me.power.as_str(), "low" | "medium" | "high") && me.cpu_cores > 0 && me.ram_gb > 0);
    assert!(me.addr.iter().any(|a| a.starts_with("127.0.0.1:")), "{:?}", me.addr);
    for o in v["offers"].as_array().unwrap() {
        // страница добавляет к карточке отметку «проверена обратным подключением» — это не часть подписанной карточки
        let mut raw = o.clone();
        raw.as_object_mut().unwrap().remove("verified");
        let o: yandi::network_offers::NodeOffer = serde_json::from_value(raw).unwrap();
        assert_eq!(o.check(now), Ok(()), "every card in the list is signed and fresh");
        assert!(o.country.is_none() || o.country_source == "ip_lookup", "no manual choice: the country comes from the self-assessment");
    }
    // список страны: все копии на одном компьютере — одна страна (или «неизвестна», если сервис адресов недоступен)
    let country = me.country.clone().unwrap_or_else(|| "??".into());
    let list = offers(2, &format!("?country={}", country.to_lowercase())).await;
    assert_eq!(list["offers"].as_array().unwrap().len(), 3, "{list}");
    assert_eq!(list["countries"][&country], json!(3));
    // то, чем узел помогает, обновляется без ожидания получаса: копия 1 выключает выход — через минуту её карточка у других без выхода
    let r = http.post("http://127.0.0.1:26100/api/exit/settings").header("cookie", &cookies[0]).json(&json!({"mode": "off"})).send().await.unwrap();
    assert!(r.status().is_success());
    let mut seen_off = false;
    for _ in 0..40 {
        let l = offers(3, "").await;
        if l["offers"].as_array().unwrap().iter().any(|o| o["node_id"] == json!(me.node_id) && o["exit"] == json!(false)) {
            seen_off = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    assert!(seen_off, "the change reached the other copies");
}
