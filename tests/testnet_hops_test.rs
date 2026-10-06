//! Цепочки через несколько узлов на тренировочной сети из 4 копий: копия 4 открывает прокси в режиме `hops`, соединение идёт
//! `вход → средний → выход`. Проверяется на настоящих процессах:
//!   * данные доходят до цели в интернете (эхо-сервер на публичном адресе этого компьютера) и возвращаются;
//!   * выход видит просьбу от цепочки, но предыдущий узел в ней — НЕ отправитель (выход не знает, кто спрашивает);
//!   * цепочка построена из трёх узлов, отправитель среди них не значится.
//! `#[ignore]` (несколько минут):
//!     cargo test --offline --test testnet_hops_test -- --ignored
#![cfg(target_os = "linux")]
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

async fn login(http: &reqwest::Client, stand: &Path, k: usize) -> (String, String, String) {
    let pw = std::fs::read_to_string(stand.join("passwords.txt")).unwrap();
    let login = pw.lines().find_map(|l| l.strip_prefix("пароль входа:")).unwrap().trim().to_string();
    let web = format!("http://127.0.0.1:{}", 26000 + 100 * k);
    let r = http.post(format!("{web}/api/auth/login")).json(&json!({"login_password": login})).send().await.unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    let list: Value = http.get(format!("{web}/api/peers/trusted")).header("cookie", &cookie).send().await.unwrap().json().await.unwrap();
    let id = yandi::web::peers::decode_card(list["card"].as_str().unwrap()).unwrap().id;
    (web, cookie, id)
}

/// SOCKS5 CONNECT с логином и паролем; `Ok(поток)` или `Err(код ответа прокси)`.
async fn socks_connect(proxy: SocketAddr, user: &str, pass: &str, target: SocketAddr) -> Result<tokio::net::TcpStream, u8> {
    let mut s = tokio::net::TcpStream::connect(proxy).await.map_err(|_| 0xffu8)?;
    s.write_all(&[5, 1, 2]).await.unwrap();
    let mut m = [0u8; 2];
    s.read_exact(&mut m).await.map_err(|_| 0xfeu8)?;
    if m != [5, 2] {
        return Err(0xfd);
    }
    let mut auth = vec![1u8, user.len() as u8];
    auth.extend_from_slice(user.as_bytes());
    auth.push(pass.len() as u8);
    auth.extend_from_slice(pass.as_bytes());
    s.write_all(&auth).await.unwrap();
    let mut a = [0u8; 2];
    s.read_exact(&mut a).await.map_err(|_| 0xfcu8)?;
    if a[1] != 0 {
        return Err(0xfb);
    }
    let IpAddr::V4(ip) = target.ip() else { panic!("v4 only") };
    let mut req = vec![5u8, 1, 0, 1];
    req.extend_from_slice(&ip.octets());
    req.extend_from_slice(&target.port().to_be_bytes());
    s.write_all(&req).await.unwrap();
    let mut rep = [0u8; 10];
    tokio::time::timeout(Duration::from_secs(30), s.read_exact(&mut rep)).await.map_err(|_| 0xfau8)?.map_err(|_| 0xf9u8)?;
    if rep[1] != 0 {
        return Err(rep[1]);
    }
    Ok(s)
}


#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts real node processes"]
async fn a_connection_goes_through_three_nodes_and_the_exit_does_not_know_the_sender() {
    for v in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "ALL_PROXY", "all_proxy"] {
        std::env::remove_var(v);
    }
    std::env::set_var("YANDI_EXIT_PORTS", "any"); // эхо-сервер стоит не на веб-порту
    let probe = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    probe.connect("8.8.8.8:53").unwrap();
    let my_ip = probe.local_addr().unwrap().ip();
    assert!(yandi::exit_policy::public_only(&my_ip), "this check needs a public address on this computer (got {my_ip})");
    let echo = tokio::net::TcpListener::bind((my_ip, 0)).await.unwrap();
    let echo_addr = echo.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut c, _)) = echo.accept().await else { return };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while let Ok(n) = c.read(&mut buf).await {
                    if n == 0 || c.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });

    let tmp = tempfile::tempdir().unwrap();
    let (stand, home) = (tmp.path().join("stand"), tmp.path().join("home"));
    std::fs::create_dir_all(&home).unwrap();
    let (code, out) = yandi(&stand, &home, &["up", "4"]);
    let _down = Down(stand.clone(), home.clone());
    assert_eq!(code, 0, "{out}");
    let http = reqwest::Client::new();
    let (web4, ck4, id4) = login(&http, &stand, 4).await;

    // копия 4 ждёт карточки остальных трёх (обмен с соседями начинается через 15 с)
    let mut seen = false;
    for _ in 0..180 {
        let o: Value = http.get(format!("{web4}/api/network/offers")).header("cookie", &ck4).send().await.unwrap().json().await.unwrap();
        let n = o["offers"].as_array().unwrap().iter().filter(|c| c["node_id"] != id4.as_str() && c["exit"] == true && c["verified"] == true).count();
        if n >= 3 {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(seen, "copy 4 learns the cards of the three others");

    let r: Value = http.post(format!("{web4}/api/socks5/start/hops")).header("cookie", &ck4).send().await.unwrap().json().await.unwrap();
    assert_eq!(r["status"], "success", "{r}");
    let port = r["local_port"].as_u64().unwrap() as u16;
    let pass = r["password"].as_str().unwrap().to_string();
    let proxy: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    for _ in 0..40 {
        if tokio::net::TcpStream::connect(proxy).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let mut ok = None;
    for _ in 0..30 {
        match socks_connect(proxy, "yandi", &pass, echo_addr).await {
            Ok(s) => {
                ok = Some(s);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_secs(1)).await,
        }
    }
    let mut s = ok.expect("the connection goes through a circuit to an internet address");
    // много данных (несколько ячеек) туда и обратно — целиком и по порядку
    let payload: Vec<u8> = (0..20_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let (mut rd, mut wr) = s.split();
    let send = async { wr.write_all(&payload).await.unwrap() };
    let mut back = vec![0u8; payload.len()];
    let recv = async { tokio::time::timeout(Duration::from_secs(60), rd.read_exact(&mut back)).await.unwrap().unwrap() };
    tokio::join!(send, recv);
    assert_eq!(back, payload, "20 KB came back whole and in order");

    // журналы: цепочка из трёх узлов без отправителя; выход получил просьбу от цепочки, но не от отправителя
    let log4 = std::fs::read_to_string(stand.join("node4/node.log")).unwrap();
    let line = log4.lines().find(|l| l.contains("[hops] строю цепочку")).expect("the sender built a circuit");
    let names: Vec<&str> = line.split(": ").last().unwrap().split(" → ").collect();
    assert_eq!(names.len(), 3, "{line}");
    assert!(!names.iter().any(|n| id4.starts_with(n)), "the sender is not on its own path: {line}");
    let exit = names[2];
    let mut exit_line = None;
    for k in 1..=3 {
        let l = std::fs::read_to_string(stand.join(format!("node{k}/node.log"))).unwrap_or_default();
        if let Some(x) = l.lines().find(|x| x.contains("[hops] выход: просьба соединиться")) {
            exit_line = Some((k, x.to_string()));
        }
    }
    let (_k, el) = exit_line.expect("an exit received the request");
    let prev = el.split("от узла ").last().unwrap().trim();
    assert!(!id4.starts_with(prev), "the exit does not see the sender: {el}");
    let _ = exit;
}
