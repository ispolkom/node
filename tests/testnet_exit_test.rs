//! Выход в интернет через другой узел на тренировочной сети (`yandi testnet`): копия 2 открывает у себя прокси SOCKS5 через копию 1
//! («якорь»), на настоящих процессах. Проверяется:
//!   * прокси открыт только на этом компьютере и только со своим паролем узла (не на внешнем адресе, не с общим «yandi123»);
//!   * через выход доходит до адреса в интернете (здесь — эхо-сервер на публичном адресе этого компьютера, чтобы не зависеть от сети);
//!   * до адресов самого компьютера выхода (петля) выход не пускает — ответ «запрещено правилами»;
//!   * владелец выхода выключил выход — никто не проходит.
//! Домашняя папка владельца подменена временной. `#[ignore]` (около минуты):
//!     cargo test --offline --test testnet_exit_test -- --ignored
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
async fn a_node_goes_online_through_a_trusted_exit_but_never_into_the_exit_owners_home() {
    for v in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "ALL_PROXY", "all_proxy"] {
        std::env::remove_var(v);
    }
    // публичный адрес этого компьютера (адрес по маршруту в интернет); без него проверку дохода до «интернета» сделать нельзя
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
                let mut buf = [0u8; 1024];
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
    let (web1, ck1, id1) = login(&http, &stand, 1).await;
    let (web2, ck2, _id2) = login(&http, &stand, 2).await;
    let s: Value = http.get(format!("{web1}/api/exit/settings")).header("cookie", &ck1).send().await.unwrap().json().await.unwrap();
    assert_eq!(s["mode"], "trusted", "the default: only trusted nodes may exit");

    // копия 2 открывает прокси через копию 1
    let r: Value = http.post(format!("{web2}/api/socks5/start/{}", &id1[..16])).header("cookie", &ck2).send().await.unwrap().json().await.unwrap();
    assert_eq!(r["status"], "success", "{r}");
    let port = r["local_port"].as_u64().unwrap() as u16;
    let pass = r["password"].as_str().unwrap().to_string();
    assert_eq!(r["listen_addr"], format!("127.0.0.1:{port}"));
    assert!(pass.len() >= 16 && pass != "yandi123");
    let proxy: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    for _ in 0..40 {
        if tokio::net::TcpStream::connect(proxy).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    // снаружи прокси не виден; без пароля не пускает
    assert!(tokio::net::TcpStream::connect((my_ip, port)).await.is_err(), "the proxy is not open on the public address");
    assert_eq!(socks_connect(proxy, "yandi", "yandi123", echo_addr).await.err(), Some(0xfb), "the old shared password does not work");

    // через выход — до адреса в интернете, данные туда и обратно
    let mut ok = None;
    for _ in 0..20 {
        match socks_connect(proxy, "yandi", &pass, echo_addr).await {
            Ok(s) => {
                ok = Some(s);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
    let mut s = ok.expect("the exit connects to an internet address");
    s.write_all("привет через выход".as_bytes()).await.unwrap();
    let mut back = vec![0u8; "привет через выход".len()];
    tokio::time::timeout(Duration::from_secs(20), s.read_exact(&mut back)).await.unwrap().unwrap();
    assert_eq!(String::from_utf8(back).unwrap(), "привет через выход");

    // в дом владельца выхода — нельзя: его страница узла на петле
    let home_target: SocketAddr = "127.0.0.1:26100".parse().unwrap();
    assert_eq!(socks_connect(proxy, "yandi", &pass, home_target).await.err(), Some(0x02), "not allowed by the exit's rules");
    let log1 = std::fs::read_to_string(stand.join("node1/node.log")).unwrap();
    assert!(log1.contains("адрес этого компьютера или домашней сети"), "the exit logs the refusal");

    // владелец выхода его выключил — никто не проходит
    let r: Value = http.post(format!("{web1}/api/exit/settings")).header("cookie", &ck1).json(&json!({"mode": "off"})).send().await.unwrap().json().await.unwrap();
    assert_eq!(r["ok"], true);
    assert_eq!(socks_connect(proxy, "yandi", &pass, echo_addr).await.err(), Some(0x02), "exit switched off");
    let bad = http.post(format!("{web1}/api/exit/settings")).header("cookie", &ck1).json(&json!({"mode": "everyone"})).send().await.unwrap();
    assert_eq!(bad.status().as_u16(), 422);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts real node processes"]
async fn a_node_finds_its_exit_by_itself_from_the_exchanged_cards() {
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
                let mut buf = [0u8; 1024];
                while let Ok(n) = c.read(&mut buf).await {
                    if n == 0 || c.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });

    // «служба страны» для проверки выхода: отвечает той страной, которую нам потом назовёт карточка (или NL, если карточка без страны)
    let geo = tokio::net::TcpListener::bind((my_ip, 0)).await.unwrap();
    let geo_addr = geo.local_addr().unwrap();
    let geo_country = std::sync::Arc::new(std::sync::Mutex::new("NL".to_string()));
    let gc = geo_country.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut c, _)) = geo.accept().await else { return };
            let country = gc.lock().unwrap().clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let _ = c.read(&mut buf).await;
                let _ = c.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n{country}\n").as_bytes()).await;
            });
        }
    });
    std::env::set_var("YANDI_GEO_PROBE", geo_addr.to_string());

    let tmp = tempfile::tempdir().unwrap();
    let (stand, home) = (tmp.path().join("stand"), tmp.path().join("home"));
    std::fs::create_dir_all(&home).unwrap();
    let (code, out) = yandi(&stand, &home, &["up", "2"]);
    let _down = Down(stand.clone(), home.clone());
    assert_eq!(code, 0, "{out}");
    let http = reqwest::Client::new();
    let (web1, ck1, id1) = login(&http, &stand, 1).await;
    let (web2, ck2, _id2) = login(&http, &stand, 2).await;

    // копия 2 ждёт карточку копии 1 (обмен с соседями начинается через 15 с) — своей страны пока нет, поэтому «auto» без страны
    let mut seen = false;
    for _ in 0..120 {
        let o: Value = http.get(format!("{web2}/api/network/offers")).header("cookie", &ck2).send().await.unwrap().json().await.unwrap();
        if o["offers"].as_array().unwrap().iter().any(|c| c["node_id"] == id1.as_str() && c["exit"] == true && c["verified"] == true) {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(seen, "the second copy learns that the first one offers an exit");
    let card: Value = http.get(format!("{web2}/api/network/offers")).header("cookie", &ck2).send().await.unwrap().json().await.unwrap();
    let claimed = card["offers"].as_array().unwrap().iter().find(|c| c["node_id"] == id1.as_str()).unwrap()["country"].as_str().map(String::from);
    if let Some(c) = &claimed {
        *geo_country.lock().unwrap() = c.clone();
    }

    let r: Value = http.post(format!("{web2}/api/socks5/start/auto")).header("cookie", &ck2).send().await.unwrap().json().await.unwrap();
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
    for _ in 0..40 {
        match socks_connect(proxy, "yandi", &pass, echo_addr).await {
            Ok(s) => {
                ok = Some(s);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
    let mut s = ok.expect("the exit was found by itself and connects to an internet address");
    s.write_all(b"auto").await.unwrap();
    let mut back = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(20), s.read_exact(&mut back)).await.unwrap().unwrap();
    assert_eq!(&back, b"auto");

    // измерение страны выхода: узел сам спросил «из какой ты страны» через этот выход (закреплённое соединение) и записал ответ
    let want_line = format!("измеренная страна {} — заявленная подтверждена", claimed.clone().unwrap_or_else(|| "NL".into()));
    let mut found = false;
    for _ in 0..60 {
        let log2 = std::fs::read_to_string(stand.join("node2/node.log")).unwrap_or_default();
        if log2.contains(&want_line) {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(found, "the node measured the exit's country through the exit itself");

    let _ = (web1, ck1);
}
