//! Правила по сайтам на тренировочной сети (2 копии, пользуемся первой): в режиме `rules` сайт, которого нет в списке «через выход», идёт напрямую
//! с этого компьютера (эхо-сервер на публичном адресе), а адрес самого компьютера и домашней сети через прокси не открывается.
//!     cargo test --offline --test testnet_rules_test -- --ignored
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
async fn an_unlisted_site_goes_directly_and_the_home_network_stays_closed() {
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
    let (code, out) = yandi(&stand, &home, &["up", "2"]);
    let _down = Down(stand.clone(), home.clone());
    assert_eq!(code, 0, "{out}");
    let http = reqwest::Client::new();
    let (web1, ck1, _) = login(&http, &stand, 2).await;
    let r: Value = http.post(format!("{web1}/api/socks5/start/rules")).header("cookie", &ck1).send().await.unwrap().json().await.unwrap();
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
    let mut s = socks_connect(proxy, "yandi", &pass, echo_addr).await.expect("an address not on the list goes directly");
    s.write_all(b"direct").await.unwrap();
    let mut back = [0u8; 6];
    tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut back)).await.unwrap().unwrap();
    assert_eq!(&back, b"direct");
    let home_target: SocketAddr = "127.0.0.1:26100".parse().unwrap();
    assert_eq!(socks_connect(proxy, "yandi", &pass, home_target).await.err(), Some(0x02), "the computer itself and the home network are not opened through the proxy");
}
