//! Вход для телефона по TLS (решение владельца 2026-10-06): телефон ходит к своему компьютеру по защищённому TCP — для провайдера это
//! обычный HTTPS внутри одной страны, а не UDP за границу; TCP, в отличие от UDP, не теряет приоритет в часы пик.
//!
//! Узел принимает TLS на отдельном порту (рекомендуется 443; включается владельцем: `YANDI_MOBILE_TLS_PORT` или файл
//! `mobile_tls.json` `{"port": 443}`), снимает шифрование и передаёт байты локальному SOCKS5-прокси узла (тот же, что открывают
//! на странице настроек: «быстро», «анонимно» или личный шлюз). Пароль прокси — как и раньше, свой у каждого узла; сертификат —
//! тот же самоподписанный, что у входа для телефонов (`tls_cert`), телефон проверяет его по отпечатку из сопряжения.
//! Если прокси ещё не запущен, соединение закрывается сразу.
//!
//! **Сайт-маскировка.** Тот, кто не начал с приветствия SOCKS5 (первый байт 0x05), — случайный посетитель или проверяющий: он
//! получает обычную страницу сайта (своя из `decoy/index.html` в папке данных, иначе простая встроенная), а не обрыв и не
//! признак чужого протокола. Своего узнаёт только тот, кто знает пароль прокси, а это уже внутри SOCKS5.
//!
//! Ограничения: не больше `MAX_CONNECTIONS` одновременных соединений; соединение без движения дольше `IDLE_SECS` закрывается.
//! Телефон за NAT сам к компьютеру за NAT не достучится — для этого телефону нужна цепочка через ретранслятор (приложение с
//! ядром узла); этот вход рассчитан на компьютер с доступным адресом (белый IP или проброшенный порт).
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

pub const MAX_CONNECTIONS: usize = 64;
pub const IDLE_SECS: u64 = 300;

/// Порт входа по TLS: переменная окружения, иначе файл; `None` — выключено.
pub fn configured_port() -> Option<u16> {
    if let Some(p) = std::env::var("YANDI_MOBILE_TLS_PORT").ok().and_then(|v| v.parse::<u16>().ok()) {
        return (p > 0).then_some(p);
    }
    let s = std::fs::read_to_string(crate::util::data_dir::data_dir().join("mobile_tls.json")).ok()?;
    let p = serde_json::from_str::<serde_json::Value>(&s).ok()?["port"].as_u64()?;
    (p > 0 && p < 65536).then_some(p as u16)
}

/// Слушать TLS на `port` и передавать на `127.0.0.1:socks_port`.
pub async fn serve(port: u16, socks_port: u16, node_hex: String) -> anyhow::Result<()> {
    let identity = crate::netlayer::tls_cert::TlsIdentity::load_or_generate_default(&node_hex)?;
    let acceptor = TlsAcceptor::from(crate::netlayer::tls_cert::build_server_config(&identity)?);
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    println!("[mobile-tls] вход для телефона по TLS на порту {port}, отпечаток сертификата {}", identity.fingerprint_hex);
    run(listener, acceptor, socks_port).await;
    Ok(())
}

async fn run(listener: TcpListener, acceptor: TlsAcceptor, socks_port: u16) {
    let active = Arc::new(AtomicUsize::new(0));
    loop {
        let Ok((tcp, _)) = listener.accept().await else { continue };
        if active.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
            continue; // закрываем, не отвечая
        }
        active.fetch_add(1, Ordering::Relaxed);
        let (acceptor, active) = (acceptor.clone(), active.clone());
        tokio::spawn(async move {
            let _ = handle(tcp, acceptor, socks_port).await;
            active.fetch_sub(1, Ordering::Relaxed);
        });
    }
}

const DEFAULT_PAGE: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>Welcome</title></head><body style=\"font-family:sans-serif;max-width:40em;margin:3em auto\"><h1>Welcome</h1><p>This site is under construction. Please come back later.</p></body></html>";

fn decoy_page() -> String {
    std::fs::read_to_string(crate::util::data_dir::data_dir().join("decoy").join("index.html")).ok().filter(|s| s.len() < 512 * 1024).unwrap_or_else(|| DEFAULT_PAGE.to_string())
}

/// Ответ сайта-маскировки на один HTTP-запрос: `/` (и `/index.html`) — страница, всё прочее — обычная 404.
fn decoy_response(request: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(request);
    let first = text.lines().next().unwrap_or("");
    let mut it = first.split_whitespace();
    let (method, path) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
    let (status, body) = match (method, path) {
        ("GET" | "HEAD", "/" | "/index.html") => ("200 OK", decoy_page()),
        ("GET" | "HEAD", _) => ("404 Not Found", "<html><head><title>404 Not Found</title></head><body><center><h1>404 Not Found</h1></center><hr><center>nginx</center></body></html>".to_string()),
        _ => ("400 Bad Request", "<html><head><title>400 Bad Request</title></head><body><center><h1>400 Bad Request</h1></center><hr><center>nginx</center></body></html>".to_string()),
    };
    let head = format!("HTTP/1.1 {status}\r\nServer: nginx\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let mut out = head.into_bytes();
    if method != "HEAD" {
        out.extend_from_slice(body.as_bytes());
    }
    out
}

async fn handle(tcp: TcpStream, acceptor: TlsAcceptor, socks_port: u16) -> std::io::Result<()> {
    let _ = tcp.set_nodelay(true);
    let mut tls = tokio::time::timeout(Duration::from_secs(10), acceptor.accept(tcp)).await.map_err(|_| std::io::Error::other("tls timeout"))??;
    // первые байты решают: приветствие SOCKS5 (0x05) — свой, всё остальное — посетитель сайта
    let mut first = vec![0u8; 4096];
    let n = tokio::time::timeout(Duration::from_secs(10), tls.read(&mut first)).await.map_err(|_| std::io::Error::other("no data"))??;
    if n == 0 {
        return Ok(());
    }
    if first[0] != 0x05 {
        tls.write_all(&decoy_response(&first[..n])).await?;
        let _ = tls.shutdown().await;
        return Ok(());
    }
    let mut back = TcpStream::connect(("127.0.0.1", socks_port)).await?;
    let _ = back.set_nodelay(true);
    back.write_all(&first[..n]).await?;
    let (mut tr, mut tw) = tokio::io::split(tls);
    let (mut br, mut bw) = back.into_split();
    let idle = Duration::from_secs(IDLE_SECS);
    let up = async {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match tokio::time::timeout(idle, tr.read(&mut buf)).await {
                Ok(Ok(n)) if n > 0 => {
                    if bw.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
        let _ = bw.shutdown().await;
    };
    let down = async {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match tokio::time::timeout(idle, br.read(&mut buf)).await {
                Ok(Ok(n)) if n > 0 => {
                    if tw.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
        let _ = tw.shutdown().await;
    };
    tokio::join!(up, down);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_rustls::TlsConnector;

    async fn echo_server() -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut c, _)) = l.accept().await else { return };
                tokio::spawn(async move {
                    let mut b = [0u8; 4096];
                    while let Ok(n) = c.read(&mut b).await {
                        if n == 0 || c.write_all(&b[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn the_phone_gets_the_local_proxy_through_tls_only_with_the_right_pinned_certificate() {
        let dir = tempfile::tempdir().unwrap();
        let identity = crate::netlayer::tls_cert::TlsIdentity::load_or_generate_in(dir.path(), &"ab".repeat(32)).unwrap();
        let acceptor = TlsAcceptor::from(crate::netlayer::tls_cert::build_server_config(&identity).unwrap());
        let socks = echo_server().await; // вместо SOCKS-прокси — эхо: проверяем сам тоннель
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(run(l, acceptor.clone(), socks));
        let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();

        // правильный отпечаток: данные идут туда и обратно, много и целиком
        let good = TlsConnector::from(crate::netlayer::tls_cert::build_client_config_pinned(&identity.fingerprint_hex).unwrap());
        let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let s = good.connect(name.clone(), tcp).await.expect("pinned handshake");
        let mut payload: Vec<u8> = (0..100_000u32).map(|i| (i % 253) as u8).collect();
        payload[0] = 0x05; // как приветствие SOCKS5: так узел узнаёт своего
        let (mut rd, mut wr) = tokio::io::split(s);
        let p2 = payload.clone();
        let w = tokio::spawn(async move {
            wr.write_all(&p2).await.unwrap();
            wr
        });
        let mut back = vec![0u8; payload.len()];
        rd.read_exact(&mut back).await.unwrap();
        assert_eq!(back, payload, "100 KB came back whole");
        let _ = w.await;

        // посетитель сайта (браузер или проверяющий) по TLS получает обычную страницу, а лишнее — обычную 404
        for (req, want) in [("GET / HTTP/1.1\r\nHost: x\r\n\r\n", "200 OK"), ("GET /admin HTTP/1.1\r\nHost: x\r\n\r\n", "404 Not Found"), ("POST / HTTP/1.1\r\nHost: x\r\n\r\n", "400 Bad Request")] {
            let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            let mut v = good.connect(name.clone(), tcp).await.unwrap();
            v.write_all(req.as_bytes()).await.unwrap();
            let mut all = Vec::new();
            let _ = tokio::time::timeout(Duration::from_secs(5), v.read_to_end(&mut all)).await;
            let text = String::from_utf8_lossy(&all);
            assert!(text.starts_with(&format!("HTTP/1.1 {want}")), "{req:?} → {text}");
            assert!(text.contains("Server: nginx"));
        }

        // чужой отпечаток (подмена узла): рукопожатие не проходит
        let bad = TlsConnector::from(crate::netlayer::tls_cert::build_client_config_pinned(&"00".repeat(32)).unwrap());
        let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        assert!(bad.connect(name.clone(), tcp).await.is_err(), "a certificate with another fingerprint is refused");

        // обычный TCP без TLS (зонд цензора) ничего не получает
        let mut raw = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        raw.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        let mut b = [0u8; 64];
        let n = tokio::time::timeout(Duration::from_secs(12), raw.read(&mut b)).await.unwrap_or(Ok(0)).unwrap_or(0);
        assert!(n <= 7, "no content for a non-TLS probe (at most a TLS alert)");

        // прокси не запущен: после рукопожатия соединение закрывается сразу
        let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_port = dead.local_addr().unwrap().port();
        drop(dead);
        let l2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port2 = l2.local_addr().unwrap().port();
        tokio::spawn(run(l2, acceptor, dead_port));
        let tcp = TcpStream::connect(("127.0.0.1", port2)).await.unwrap();
        let mut s2 = good.connect(name.clone(), tcp).await.unwrap();
        s2.write_all(&[5, 1, 2]).await.unwrap();
        let mut b = [0u8; 8];
        assert_eq!(tokio::time::timeout(Duration::from_secs(5), s2.read(&mut b)).await.unwrap().unwrap_or(0), 0, "closed at once");
    }
}
