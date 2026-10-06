//! «Белый адрес» по факту, а не по заявлению (решение владельца 2026-10-06: узлами сети становятся только узлы с белым IP).
//!
//! Узел может написать в своей карточке что угодно («публичный адрес есть»), а на деле сидеть за общим адресом оператора
//! (CGNAT) или за роутером без проброса порта — до такого узла никто не дойдёт, и цепочки и выходы через него сломаются.
//! Поэтому каждый узел **сам проверяет** карточки, которые принимает: подключается по TCP на заявленный адрес со стороны, с которой
//! узел раньше не говорил, и требует доказательство — подпись ключом из карточки под свежей случайной строкой. Не ответил —
//! карточка остаётся в каталоге «непроверенной» и не попадает ни в выбор выхода, ни в цепочки, ни в раздачу соседям.
//!
//! * Слушатель: у каждого узла с публичным адресом на TCP-порту основной связи (порты TCP и UDP независимы) отвечает на пробу.
//! * Проба: `[YPRB][случайная строка 16]` → `[YPRB][номер узла 32][подпись 64]`; подпись — под `адрес:порт` и строкой.
//! * Бюджет проб: не больше 8 одновременно и не чаще одной на карточку за 10 минут при неудаче; адрес пробы должен быть публичным
//!   (иначе чужая карточка заставила бы узлы стучаться в домашние сети) — кроме тренировочной сети (`YANDI_TESTNET=1`).
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::network_offers::NodeOffer;

const MAGIC: &[u8; 4] = b"YPRB";
const REPLY_LEN: usize = 4 + 32 + 64;
pub const PROBE_TIMEOUT_SECS: u64 = 6;
/// Сколько секунд проверка действует (потом карточка проверяется заново при следующем обмене).
pub const VERIFIED_SECS: u64 = 3600;
/// После неудачи заново не пробовать раньше чем через столько секунд.
pub const RETRY_SECS: u64 = 600;
pub const MAX_PARALLEL: usize = 8;

static INBOUND: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Сколько проб пришло на этот узел снаружи. Ноль после долгого времени при наличии соседей значит: до узла не достучаться
/// (он за NAT) — он должен стать клиентом через ретрансляторы.
pub fn inbound_probes() -> u64 {
    INBOUND.load(std::sync::atomic::Ordering::Relaxed)
}

fn proof_bytes(nonce: &[u8; 16], addr: &str) -> Vec<u8> {
    let mut b = b"yandi-probe-v1\0".to_vec();
    b.extend_from_slice(nonce);
    b.extend_from_slice(addr.as_bytes());
    b
}

/// Адрес, на который разрешено стучаться: публичный (или любой — в тренировочной сети).
pub fn probe_allowed(addr: &str) -> bool {
    let Ok(sa) = addr.parse::<std::net::SocketAddr>() else { return false };
    std::env::var(crate::testnet::ENV).is_ok() || crate::exit_policy::public_only(&sa.ip())
}

/// Слушатель проб на TCP-порту основной связи: отвечает подписью на любую строку (и ничего больше не умеет).
pub async fn serve(port: u16, node_id: [u8; 32], key: SigningKey, advertised: Option<Vec<String>>) {
    let l = match TcpListener::bind(("0.0.0.0", port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[reach] не удалось открыть порт проб {port}: {e}");
            return;
        }
    };
    loop {
        let Ok((mut s, _)) = l.accept().await else { continue };
        // на этом же порту слушает запасной путь по TCP/TLS (`tcp_carrier`): TLS начинается с байта 0x16, проба достижимости — с «Y»
        let mut first = [0u8; 1];
        if let Ok(Ok(1)) = tokio::time::timeout(std::time::Duration::from_secs(3), s.peek(&mut first)).await {
            if first[0] == 0x16 {
                if let Some(c) = crate::netlayer::tcp_carrier::global() {
                    tokio::spawn(async move {
                        let _ = c.accept(s).await;
                    });
                }
                continue;
            }
        }
        let key = key.clone();
        let advertised = advertised.clone();
        tokio::spawn(async move {
            let work = async {
                let mut req = [0u8; 20];
                s.read_exact(&mut req).await.ok()?;
                if &req[..4] != MAGIC {
                    return None;
                }
                let nonce: [u8; 16] = req[4..].try_into().ok()?;
                INBOUND.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                // подписываем под тот адрес, на который к нам пришли: проверяющий передаёт его в пробе не по сети, а знает сам,
                // поэтому подписываем под каждый из своих заявленных адресов и отвечаем подписью первого, сошедшегося — их ≤ 4
                let adv: Vec<String> = advertised.clone().or_else(|| crate::network_offers::own().map(|o| o.addr)).unwrap_or_default();
                let mut out = Vec::with_capacity(REPLY_LEN * 4);
                for a in &adv {
                    let mut r = MAGIC.to_vec();
                    r.extend_from_slice(&node_id);
                    r.extend_from_slice(&key.sign(&proof_bytes(&nonce, a)).to_bytes());
                    out.extend_from_slice(&r);
                }
                s.write_all(&out).await.ok()?;
                Some(())
            };
            let _ = tokio::time::timeout(std::time::Duration::from_secs(PROBE_TIMEOUT_SECS), work).await;
        });
    }
}

/// Проверить один адрес карточки: соединиться и проверить подпись. `Ok(())` — адрес достижим и принадлежит этому ключу.
pub async fn probe(card: &NodeOffer, addr: &str) -> Result<(), &'static str> {
    if !probe_allowed(addr) {
        return Err("address not allowed");
    }
    let key = hex::decode(&card.key).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()).and_then(|b| VerifyingKey::from_bytes(&b).ok()).ok_or("key")?;
    let want_id = hex::decode(&card.node_id).map_err(|_| "id")?;
    let work = async {
        let mut s = TcpStream::connect(addr).await.map_err(|_| "no connection")?;
        let mut nonce = [0u8; 16];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut nonce);
        let mut req = MAGIC.to_vec();
        req.extend_from_slice(&nonce);
        s.write_all(&req).await.map_err(|_| "write")?;
        // ответ — по одной подписи на каждый заявленный адрес узла (до 4); читаем, сколько пришло, и ищем сошедшуюся
        let mut buf = vec![0u8; REPLY_LEN * 4];
        let mut n = 0;
        while n < buf.len() {
            match s.read(&mut buf[n..]).await {
                Ok(0) | Err(_) => break,
                Ok(k) => n += k,
            }
        }
        for chunk in buf[..n].chunks_exact(REPLY_LEN) {
            if &chunk[..4] != MAGIC || chunk[4..36] != want_id[..] {
                continue;
            }
            let sig = Signature::from_bytes(chunk[36..].try_into().unwrap());
            if key.verify(&proof_bytes(&nonce, addr), &sig).is_ok() {
                return Ok(());
            }
        }
        Err("no proof")
    };
    tokio::time::timeout(std::time::Duration::from_secs(PROBE_TIMEOUT_SECS), work).await.map_err(|_| "timeout")?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card_for(sk: &SigningKey, id: [u8; 32], addr: &str) -> NodeOffer {
        NodeOffer {
            v: 1, node_id: hex::encode(id), key: hex::encode(sk.verifying_key().to_bytes()), country: None, country_source: "unknown".into(), public_ip: true,
            power: "high".into(), cpu_cores: 4, ram_gb: 8, latency_ms: None, addr: vec![addr.into()], exit: true, can_exit: true, relay: false, issued: 0, expires: 0, sig: String::new(),
        }
    }

    #[tokio::test]
    async fn a_reachable_node_that_owns_its_key_is_verified_and_everyone_else_is_not() {
        // вне тренировочной сети чужая карточка не заставит стучаться в домашние сети
        std::env::remove_var(crate::testnet::ENV);
        for a in ["127.0.0.1:80", "192.168.1.1:9000", "10.0.0.1:1", "[::1]:5", "not an address"] {
            assert!(!probe_allowed(a), "{a}");
        }
        assert!(probe_allowed("8.8.8.8:9000"));
        std::env::set_var(crate::testnet::ENV, "1"); // дальше пробуем петлю
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let addr = format!("127.0.0.1:{port}");
        let sk = SigningKey::from_bytes(&[5; 32]);
        tokio::spawn(serve_on(port, [5; 32], sk.clone(), Some(vec![addr.clone()])));
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(probe(&card_for(&sk, [5; 32], &addr), &addr).await, Ok(()));
        // чужой ключ в карточке: подпись не сойдётся
        let other = SigningKey::from_bytes(&[6; 32]);
        assert_eq!(probe(&card_for(&other, [5; 32], &addr), &addr).await, Err("no proof"));
        // чужой номер узла в карточке
        assert_eq!(probe(&card_for(&sk, [9; 32], &addr), &addr).await, Err("no proof"));
        // порт закрыт
        let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_addr = dead.local_addr().unwrap().to_string();
        drop(dead);
        assert_eq!(probe(&card_for(&sk, [5; 32], &dead_addr), &dead_addr).await, Err("no connection"));
    }

    /// то же, что `serve`, но с уже занятой в тесте петлёй
    async fn serve_on(port: u16, id: [u8; 32], key: SigningKey, adv: Option<Vec<String>>) {
        serve(port, id, key, adv).await
    }
}
