//! Смена ключей в работающей сети (настоящие узлы).
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

struct Node {
    web: String,
    cookie: String,
    id: String,
}

async fn node(http: &reqwest::Client, stand: &Path, k: usize) -> Node {
    let pw = std::fs::read_to_string(stand.join("passwords.txt")).unwrap();
    let login = pw.lines().find_map(|l| l.strip_prefix("пароль входа:")).unwrap().trim().to_string();
    let web = format!("http://127.0.0.1:{}", 26000 + 100 * k);
    let r = http.post(format!("{web}/api/auth/login")).json(&json!({"login_password": login})).send().await.unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    let list: Value = http.get(format!("{web}/api/peers/trusted")).header("cookie", &cookie).send().await.unwrap().json().await.unwrap();
    let id = yandi::web::peers::decode_card(list["card"].as_str().unwrap()).unwrap().id;
    Node { web, cookie, id }
}

async fn send(http: &reqwest::Client, from: &Node, to: &Node, text: &str) {
    for _ in 0..30 {
        let r: Value = http.post(format!("{}/api/chat/send/{}", from.web, to.id)).header("cookie", &from.cookie).json(&json!({"text": text})).send().await.unwrap().json().await.unwrap();
        if r["status"] == "success" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("message was never sent: {text}");
}

async fn texts(http: &reqwest::Client, n: &Node, with: &str) -> std::collections::HashSet<String> {
    let h: Value = http.get(format!("{}/api/chat/history/{with}", n.web)).header("cookie", &n.cookie).send().await.unwrap().json().await.unwrap();
    h["messages"].as_array().cloned().unwrap_or_default().into_iter().filter_map(|m| m["text"].as_str().map(String::from)).collect()
}

/// Смена ключа в работающей сети: возраст ключа 15 с, поток сообщений в обе стороны около минуты — ни одно сообщение не теряется,
/// а в журналах видно, что узлы сами договорились о новых ключах.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts real node processes (about two minutes)"]
async fn keys_are_replaced_in_a_running_network_without_losing_messages() {
    for v in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "ALL_PROXY", "all_proxy"] {
        std::env::remove_var(v);
    }
    std::env::set_var("YANDI_REKEY_AFTER_SECS", "15");
    let tmp = tempfile::tempdir().unwrap();
    let (stand, home) = (tmp.path().join("stand"), tmp.path().join("home"));
    std::fs::create_dir_all(&home).unwrap();
    let (code, out) = yandi(&stand, &home, &["up", "2"]);
    let _down = Down(stand.clone(), home.clone());
    assert_eq!(code, 0, "{out}");
    let http = reqwest::Client::new();
    let (a, b) = (node(&http, &stand, 1).await, node(&http, &stand, 2).await);

    let mut sent_ab = vec![];
    let mut sent_ba = vec![];
    for i in 0..30 {
        let (t1, t2) = (format!("a→b {i}"), format!("b→a {i}"));
        send(&http, &a, &b, &t1).await;
        send(&http, &b, &a, &t2).await;
        sent_ab.push(t1);
        sent_ba.push(t2);
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    // все сообщения дошли в обе стороны (даём время на доставку последних)
    let mut missing = vec![];
    for _ in 0..40 {
        let got_b = texts(&http, &b, &a.id).await;
        let got_a = texts(&http, &a, &b.id).await;
        missing = sent_ab.iter().filter(|t| !got_b.contains(*t)).chain(sent_ba.iter().filter(|t| !got_a.contains(*t))).cloned().collect();
        if missing.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    if !missing.is_empty() {
        // при провале сохраняем журналы узлов (папка стенда временная и исчезнет)
        let keep = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/rekey-fail-logs");
        let _ = std::fs::create_dir_all(&keep);
        for k in 1..=2 {
            let _ = std::fs::copy(stand.join(format!("node{k}/node.log")), keep.join(format!("node{k}.log")));
        }
        eprintln!("журналы узлов сохранены в {}", keep.display());
    }
    assert!(missing.is_empty(), "потеряны сообщения при смене ключей: {missing:?}");

    // узлы действительно меняли ключи
    let mut rekeys = 0;
    for k in 1..=2 {
        let log = std::fs::read_to_string(stand.join(format!("node{k}/node.log"))).unwrap_or_default();
        rekeys += log.matches("is old — asking for a new one").count();
    }
    assert!(rekeys >= 2, "узлы не меняли ключи (запросов: {rekeys})");
}
