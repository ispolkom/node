//! Связь между людьми на тренировочной сети (`yandi testnet`), на настоящих процессах узла и настоящих UDP-пакетах:
//!   * сообщение доходит в обе стороны, отправитель видит «прочитано»;
//!   * поддельное НЕЗАШИФРОВАННОЕ сообщение «от друга» (отправитель вписан в заголовок) и старый открытый формат — не принимаются;
//!   * файл доходит целиком, байт в байт, и ложится в папку данных получателя (не в чужую папку);
//!   * звонок: вызов, входящий с настоящим звонящим, «принят», сигналы браузеров в обе стороны, «положил трубку»;
//!   * в журнале получателя нет открытых пакетов канала — только зашифрованные.
//! Домашняя папка владельца подменена временной. `#[ignore]` (около минуты):
//!     cargo test --offline --test testnet_comms_test -- --ignored
#![cfg(target_os = "linux")]
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::{json, Value};

fn yandi(stand: &Path, home: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_yandi")).arg("testnet").args(args).env("YANDI_TESTNET_DIR", stand).env("HOME", home).env_remove("XDG_DATA_HOME").output().unwrap();
    (out.status.code().unwrap_or(-1), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Первое текстовое сообщение веб-сокета, в котором есть `needle` (не дольше 20 с); прочие сигналы пропускаются.
async fn next_text(s: &mut Ws, needle: &str) -> Result<String, tokio::time::error::Elapsed> {
    use futures_util::StreamExt;
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match s.next().await {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t))) if t.contains(needle) => return t.to_string(),
                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(_))) => continue,
                Some(Ok(_)) => continue,
                other => panic!("websocket ended: {other:?}"),
            }
        }
    })
    .await
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

async fn history(http: &reqwest::Client, n: &Node, with: &str) -> Vec<Value> {
    let h: Value = http.get(format!("{}/api/chat/history/{with}", n.web)).header("cookie", &n.cookie).send().await.unwrap().json().await.unwrap();
    h["messages"].as_array().cloned().unwrap_or_default()
}

async fn send(http: &reqwest::Client, from: &Node, to: &Node, text: &str) -> Value {
    for _ in 0..20 {
        let r: Value = http.post(format!("{}/api/chat/send/{}", from.web, to.id)).header("cookie", &from.cookie).json(&json!({"text": text})).send().await.unwrap().json().await.unwrap();
        if r["status"] == "success" {
            return r;
        }
        // первая попытка может застать канал до выработки общего ключа: узел сам стучится и просит повторить
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("message was never sent");
}

async fn wait_text(http: &reqwest::Client, n: &Node, with: &str, text: &str) -> Option<Value> {
    for _ in 0..60 {
        if let Some(m) = history(http, n, with).await.into_iter().find(|m| m["text"] == text) {
            return Some(m);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    None
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts real node processes"]
async fn people_talk_and_send_files_only_over_encrypted_links() {
    for v in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "ALL_PROXY", "all_proxy"] {
        std::env::remove_var(v);
    }
    let tmp = tempfile::tempdir().unwrap();
    // YANDI_TEST_KEEP=1 — оставить папку сети для разбора журналов
    let root = if std::env::var_os("YANDI_TEST_KEEP").is_some() { let p = tmp.path().to_path_buf(); std::mem::forget(tmp); eprintln!("stand kept in {}", p.display()); p } else { tmp.path().to_path_buf() };
    let (stand, home) = (root.join("stand"), root.join("home"));
    std::fs::create_dir_all(&home).unwrap();
    let (code, out) = yandi(&stand, &home, &["up", "2"]);
    let _down = Down(stand.clone(), home.clone());
    assert_eq!(code, 0, "{out}");
    let http = reqwest::Client::new();
    let (a, b) = (node(&http, &stand, 1).await, node(&http, &stand, 2).await);

    // сообщения в обе стороны
    send(&http, &a, &b, "Привет от первого узла").await;
    assert!(wait_text(&http, &b, &a.id, "Привет от первого узла").await.is_some(), "A → B delivered");
    send(&http, &b, &a, "И тебе привет").await;
    assert!(wait_text(&http, &a, &b.id, "И тебе привет").await.is_some(), "B → A delivered");
    let mut read = false;
    for _ in 0..40 {
        if history(&http, &a, &b.id).await.iter().any(|m| m["text"] == "Привет от первого узла" && (m["status"] == "Read" || m["status"] == "Delivered")) {
            read = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(read, "the sender sees the delivery confirmation");

    // подделка: открытый пакет с отправителем «первый узел», прямо в порт данных второго
    let a_id = yandi::util::HashId::from_hex(&a.id).unwrap();
    let b_id = yandi::util::HashId::from_hex(&b.id).unwrap();
    let fake = yandi::communication::ChatMessage::new(a_id, b_id, "ПОДДЕЛКА".into());
    let pkt = yandi::p2p::P2PPacket::new(yandi::p2p::P2PPacketType::ChatMessage, a_id, false, serde_json::to_vec(&fake).unwrap());
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    for _ in 0..3 {
        sock.send_to(&pkt.to_bytes(), "127.0.0.1:26204").unwrap();
        // старый открытый формат (CommPacket 0x60…) — тоже
        let mut legacy = vec![0x60u8];
        legacy.extend_from_slice(&serde_json::to_vec(&fake).unwrap());
        sock.send_to(&legacy, "127.0.0.1:26204").unwrap();
    }
    assert!(wait_text(&http, &b, &a.id, "ПОДДЕЛКА").await.is_none(), "a forged plaintext message is not accepted");
    let log_b = std::fs::read_to_string(stand.join("node2/node.log")).unwrap();
    assert!(log_b.contains("unencrypted packet from") || log_b.contains("plaintext legacy packet"), "the refusal is logged");

    // файл: байт в байт, в папку данных получателя
    let data: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
    let boundary = "yandi-test-boundary";
    let mut body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"проверка.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
    body.extend_from_slice(&data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let r: Value = http.post(format!("{}/api/files/send-file/{}", a.web, b.id)).header("cookie", &a.cookie)
        .header("content-type", format!("multipart/form-data; boundary={boundary}")).body(body).send().await.unwrap().json().await.unwrap();
    assert_eq!(r["status"], "success", "{r}");
    let downloads = stand.join("node2/home/.local/share/yandi/files/downloads");
    let mut got = None;
    for _ in 0..120 {
        if let Ok(rd) = std::fs::read_dir(&downloads) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                // на диске имя — «<номер передачи>__<имя без не-латинских знаков>»
                if name.starts_with(r["file_id"].as_str().unwrap()) && !name.ends_with(".part") {
                    got = std::fs::read(e.path()).ok();
                }
            }
        }
        if got.as_ref().map(|g| g.len() == data.len()).unwrap_or(false) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let got = got.expect("the file arrived in the receiver's own data folder");
    assert_eq!(got.len(), data.len());
    assert!(got == data, "byte for byte");

    // звонок: вызов → входящий у второго (звонящий — настоящий) → «принят» у первого → сигналы браузеров туда и обратно → «положил трубку»
    let ws = |n: &Node, peer: &str| {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = format!("{}/api/media/ws/{}", n.web.replace("http://", "ws://"), peer).into_client_request().unwrap();
        req.headers_mut().insert("cookie", n.cookie.parse().unwrap());
        req
    };
    let (mut ws_a, _) = tokio_tungstenite::connect_async(ws(&a, &b.id[..16])).await.unwrap();
    let (mut ws_b, _) = tokio_tungstenite::connect_async(ws(&b, &a.id[..16])).await.unwrap();
    let call: Value = http.post(format!("{}/api/media/call/start", a.web)).header("cookie", &a.cookie)
        .json(&json!({"peer_id": b.id, "audio_enabled": true, "video_enabled": false, "display_name": "Первый"})).send().await.unwrap().json().await.unwrap();
    let call_id = call["call_id"].as_str().unwrap().to_string();
    let mut incoming = Value::Null;
    for _ in 0..40 {
        incoming = http.get(format!("{}/api/media/incoming-call", b.web)).header("cookie", &b.cookie).send().await.unwrap().json().await.unwrap();
        if incoming["call_id"] == call_id.as_str() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!((incoming["call_id"].as_str(), incoming["from_short_id"].as_str(), incoming["from_display_name"].as_str()), (Some(call_id.as_str()), Some(&a.id[..16]), Some("Первый")), "{incoming}");
    let acc: Value = http.post(format!("{}/api/media/call/{call_id}/accept", b.web)).header("cookie", &b.cookie).send().await.unwrap().json().await.unwrap();
    assert_eq!(acc["status"], "ok", "{acc}");
    use futures_util::SinkExt;
    let got = next_text(&mut ws_a, "call-accept").await.expect("the caller hears «accepted»");
    assert!(got.contains("call-accept") && got.contains(&call_id), "{got}");
    ws_a.send(tokio_tungstenite::tungstenite::Message::Text(r#"{"type":"offer","sdp":"v=0 проверка"}"#.into())).await.unwrap();
    let got = next_text(&mut ws_b, "offer").await.expect("the browser signal reaches the callee");
    assert!(got.contains("offer") && got.contains("проверка"), "{got}");
    ws_b.send(tokio_tungstenite::tungstenite::Message::Text(r#"{"type":"answer","sdp":"v=0 ответ"}"#.into())).await.unwrap();
    let got = next_text(&mut ws_a, "answer").await.expect("and back");
    assert!(got.contains("answer") && got.contains("ответ"), "{got}");
    let _: Value = http.post(format!("{}/api/media/call/end", a.web)).header("cookie", &a.cookie).json(&json!({"peer_id": b.id, "call_id": call_id})).send().await.unwrap().json().await.unwrap();
    let got = next_text(&mut ws_b, "hangup").await.expect("the callee hears the hang-up");
    assert!(got.contains("hangup"), "{got}");

    // в канале не было ни одного открытого пакета от своих
    for k in 1..=2 {
        let log = std::fs::read_to_string(stand.join(format!("node{k}/node.log"))).unwrap();
        assert!(!log.contains("Encryption failed for"), "node {k}");
    }
}
