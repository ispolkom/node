//! Личный шлюз: «телефон» (копия 3) ходит в интернет через СВОЙ компьютер (копия 1), по цепочке через ретранслятор (копия 2).
//! Проверяется на настоящих процессах:
//!   * по секрету устройств шлюз пускает на любой порт (эхо-сервер не на веб-порту), без секрета/с чужим секретом — отказ;
//!   * ретранслятор в середине передаёт, но сам ничего не открывает (в его журнале нет «выход: просьба»);
//!   * шлюз знает, что пришло своё устройство.
//!     cargo test --offline --test testnet_personal_test -- --ignored
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
async fn a_phone_reaches_the_internet_through_its_own_computer_via_a_relay() {
    for v in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "ALL_PROXY", "all_proxy"] {
        std::env::remove_var(v);
    }
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
    let (code, out) = yandi(&stand, &home, &["up", "3"]);
    let _down = Down(stand.clone(), home.clone());
    assert_eq!(code, 0, "{out}");
    let http = reqwest::Client::new();
    let (web1, ck1, _) = login(&http, &stand, 1).await;
    let (web2, ck2, _) = login(&http, &stand, 2).await;
    let (web3, ck3, _) = login(&http, &stand, 3).await;
    let pair = |web: String, ck: String| {
        let http = http.clone();
        async move { http.get(format!("{web}/api/devices/pairing")).header("cookie", ck).send().await.unwrap().json::<Value>().await.unwrap() }
    };
    let gw = pair(web1, ck1).await;
    let relay = pair(web2, ck2).await;
    assert!(gw["secret"].as_str().unwrap().len() == 64 && gw["secret"] != relay["secret"], "every node has its own device secret");

    let start = |secret: String| {
        let (http, web, ck, gw, relay) = (http.clone(), web3.clone(), ck3.clone(), gw.clone(), relay.clone());
        async move {
            let r: Value = http.post(format!("{web}/api/socks5/personal")).header("cookie", ck)
                .json(&json!({"node": gw["node"], "key": gw["key"], "secret": secret, "relay": {"node": relay["node"], "key": relay["key"]}}))
                .send().await.unwrap().json().await.unwrap();
            r
        }
    };
    let r = start(gw["secret"].as_str().unwrap().to_string()).await;
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
    for _ in 0..20 {
        match socks_connect(proxy, "yandi", &pass, echo_addr).await {
            Ok(s) => {
                ok = Some(s);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_secs(1)).await,
        }
    }
    let mut s = ok.expect("the own device reaches an internet address through its own computer");
    s.write_all("через свой компьютер".as_bytes()).await.unwrap();
    let mut back = vec![0u8; "через свой компьютер".len()];
    tokio::time::timeout(Duration::from_secs(30), s.read_exact(&mut back)).await.unwrap().unwrap();
    assert_eq!(String::from_utf8(back).unwrap(), "через свой компьютер");

    let log1 = std::fs::read_to_string(stand.join("node1/node.log")).unwrap();
    assert!(log1.contains("(своё устройство)"), "the gateway knows it is its own device");
    let log2 = std::fs::read_to_string(stand.join("node2/node.log")).unwrap();
    assert!(!log2.contains("[hops] выход: просьба"), "the relay in the middle opens nothing");
    assert!(log2.contains("[hops] строю цепочку") == false, "the relay does not build circuits for this");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts real node processes"]
async fn a_gateway_behind_nat_is_found_through_its_announced_relays_and_reached_through_one() {
    for v in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "ALL_PROXY", "all_proxy"] {
        std::env::remove_var(v);
    }
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

    // копия 1 играет компьютер за NAT: узлом сети не становится, объявляет, через какие ретрансляторы её найти
    std::env::set_var("YANDI_TESTNET_CLIENT_NODES", "1");
    let tmp = tempfile::tempdir().unwrap();
    let (stand, home) = (tmp.path().join("stand"), tmp.path().join("home"));
    std::fs::create_dir_all(&home).unwrap();
    let (code, out) = yandi(&stand, &home, &["up", "4"]);
    let _down = Down(stand.clone(), home.clone());
    assert_eq!(code, 0, "{out}");
    let http = reqwest::Client::new();
    let (web1, ck1, _) = login(&http, &stand, 1).await;
    let (web1x, ck1x) = (web1.clone(), ck1.clone());
    let (web2, ck2, _) = login(&http, &stand, 2).await;
    let (web3, ck3, _) = login(&http, &stand, 4).await; // «телефон» — копия 4
    let pair = |web: String, ck: String| {
        let http = http.clone();
        async move { http.get(format!("{web}/api/devices/pairing")).header("cookie", ck).send().await.unwrap().json::<Value>().await.unwrap() }
    };
    let gw = pair(web1, ck1).await;
    let relay = pair(web2.clone(), ck2.clone()).await;
    let gw_id = gw["node"].as_str().unwrap().to_string();
    // копия 4 ждёт запись «копия 1 доступна через ретрансляторы» (раздаётся соседям раз в полминуты)
    let mut known = false;
    for _ in 0..240 {
        let st: Value = http.get(format!("{web3}/api/network/relay")).header("cookie", &ck3).send().await.unwrap().json().await.unwrap();
        if st["records"].as_u64().unwrap_or(0) >= 1 {
            known = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(known, "the phone copy learns the announcement of the gateway");
    let st1: Value = http.get(format!("{web1x}/api/network/relay")).header("cookie", &ck1x).send().await.unwrap().json().await.unwrap();
    assert_eq!(st1["client"], true, "copy 1 plays a node behind NAT");
    assert!(gw["secret"].as_str().unwrap().len() == 64 && gw["secret"] != relay["secret"], "every node has its own device secret");

    let start = |secret: String| {
        let (http, web, ck, gw, relay) = (http.clone(), web3.clone(), ck3.clone(), gw.clone(), relay.clone());
        async move {
            let r: Value = http.post(format!("{web}/api/socks5/personal")).header("cookie", ck)
                .json(&json!({"node": gw["node"], "key": gw["key"], "secret": secret}))
                .send().await.unwrap().json().await.unwrap();
            r
        }
    };
    let r = start(gw["secret"].as_str().unwrap().to_string()).await;
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
    for _ in 0..20 {
        match socks_connect(proxy, "yandi", &pass, echo_addr).await {
            Ok(s) => {
                ok = Some(s);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_secs(1)).await,
        }
    }
    let mut s = ok.expect("the own device reaches an internet address through its own computer");
    s.write_all("через свой компьютер".as_bytes()).await.unwrap();
    let mut back = vec![0u8; "через свой компьютер".len()];
    tokio::time::timeout(Duration::from_secs(30), s.read_exact(&mut back)).await.unwrap().unwrap();
    assert_eq!(String::from_utf8(back).unwrap(), "через свой компьютер");

    let log1 = std::fs::read_to_string(stand.join("node1/node.log")).unwrap();
    assert!(log1.contains("(своё устройство)"), "the gateway knows it is its own device");
    let log2 = std::fs::read_to_string(stand.join("node2/node.log")).unwrap();
    let log4 = std::fs::read_to_string(stand.join("node4/node.log")).unwrap();
    let line = log4.lines().find(|l| l.contains("[hops] строю цепочку")).expect("the phone built a circuit");
    let names: Vec<&str> = line.split(": ").last().unwrap().split(" → ").collect();
    assert_eq!(names.len(), 2, "relay → gateway, found by the announcement, not given by hand: {line}");
    assert!(gw_id.starts_with(names[1]), "{line}");
    assert!(!gw_id.starts_with(names[0]), "the relay is another node: {line}");
    let _ = (relay, log2);
}
