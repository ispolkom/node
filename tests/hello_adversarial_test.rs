//! Подписанные, но враждебные приветствия: подпись можно поставить на любые значения, поэтому после проверки подписи обработчик получает поля, придуманные
//! злоумышленником (странные адреса, предельные длины, мусорные символы, любые возможности). Паника в обработчике тихо останавливает приём приветствий —
//! узел перестаёт принимать новых. Тест шлёт сотни таких приветствий в обе сети и требует: паник нет, приём жив, честный узел после атаки принят.
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::net::UdpSocket;
use tokio::time::{sleep, Duration};

use yandi::core::identity::NodeIdentity;
use yandi::netlayer::packet::{HelloPacket, Signature};
use yandi::netlayer::transport::P2PTransport as NetTransport;
use yandi::p2p::hello::P2PHelloPacket;
use yandi::p2p::transport::P2PTransport;

static PANICS: AtomicUsize = AtomicUsize::new(0);

const NASTY: &[&str] = &[
    "0.0.0.0:0", "0.0.0.0", "not an address", "[::]:99999", "1.2.3.4:65536", "999.999.999.999:1", ":::", "[::1", "%", "0.0.0.0:-1", "@", "a:b:c:d", "[fe80::1%eth0]:80",
    "😀😀😀:80", "\u{7f}\u{1}\u{2}", "0.0.0.0:99999999999999999999", "255.255.255.255:65535", "[::ffff:1.2.3.4]:80", "localhost:0", "-1", "0x7f.0.0.1:80", " ",
    "1.2.3.4:", ":80", "[]:80", "[:::]:80", "1.2.3.4:80:90", "http://x", "\n\r\t",
];

fn long(n: usize) -> String {
    "a".repeat(n)
}

fn signed_net_hello(strings: [Option<String>; 5], jurisdiction: Option<String>, caps: u16) -> Vec<u8> {
    let id = NodeIdentity::new();
    let mut cid = [0u8; 8];
    cid.copy_from_slice(&id.node_id().0[..8]);
    let mut h = HelloPacket::new_request(id.node_id(), id.signing_public_key, rand::random(), cid, caps);
    let [wan, lan, disc, ext6, p2p] = strings;
    h.wan_address = wan;
    h.lan_address = lan;
    h.discovery_endpoint = disc;
    h.ipv6_external = ext6;
    h.p2p_data_addr = p2p;
    h.jurisdiction = yandi::netlayer::packet::normalize_jurisdiction(jurisdiction);
    h.p2p_x25519_public = Some(rand::random());
    let sig = id.sign(&h.challenge_data()).unwrap();
    let mut b = [0u8; 64];
    b.copy_from_slice(&sig);
    h.signature = Signature(b);
    h.to_bytes().unwrap_or_default()
}

fn signed_p2p_hello(addr: String) -> Vec<u8> {
    let id = NodeIdentity::new();
    let mut h = P2PHelloPacket::new_request(id.node_id(), rand::random(), addr, id.signing_public_key);
    h.sign(&id).unwrap();
    h.to_bytes().unwrap_or_default()
}

fn honest_net(id: &NodeIdentity) -> Vec<u8> {
    let mut cid = [0u8; 8];
    cid.copy_from_slice(&id.node_id().0[..8]);
    let mut h = HelloPacket::new_request(id.node_id(), id.signing_public_key, rand::random(), cid, 0);
    let sig = id.sign(&h.challenge_data()).unwrap();
    let mut b = [0u8; 64];
    b.copy_from_slice(&sig);
    h.signature = Signature(b);
    h.to_bytes().unwrap()
}

fn loopback_of(bind_addr: &str) -> SocketAddr {
    let port: u16 = bind_addr.rsplit(':').next().unwrap().parse().unwrap();
    SocketAddr::from(([127, 0, 0, 1], port))
}

async fn from_ip(n: u8) -> UdpSocket {
    UdpSocket::bind(format!("127.0.0.{n}:0")).await.unwrap()
}

#[tokio::test]
async fn hostile_but_signed_hellos_do_not_kill_either_hello_listener() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        PANICS.fetch_add(1, Ordering::SeqCst);
        eprintln!("PANIC: {info}");
    }));

    std::env::set_var("YANDI_P2P_DISCOVERY_PORT", "19531");
    std::env::set_var("YANDI_P2P_DATA_PORT", "19532");
    let net = NetTransport::with_handlers(NodeIdentity::new(), 0, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, 19533, 19534).await.unwrap();
    let p2p = P2PTransport::new(NodeIdentity::new(), 0).await.unwrap();
    let net_addr = SocketAddr::from(([127, 0, 0, 1], net.discovery_addr().port()));
    let p2p_addr = loopback_of(&p2p.discovery_addr());

    // каждый пакет — со своего адреса 127.0.0.N (чтобы не упереться в допуск по адресу)
    let mut n = 1u8;
    let mut next = || {
        n = if n >= 250 { 1 } else { n + 1 };
        n
    };
    let mut sent = 0;
    for s in NASTY {
        for (i, set) in [[Some(s.to_string()), None, None, None, None], [None, Some(s.to_string()), None, None, None], [None, None, Some(s.to_string()), None, None], [None, None, None, Some(s.to_string()), None], [None, None, None, None, Some(s.to_string())]].into_iter().enumerate() {
            let sock = from_ip(next()).await;
            sock.send_to(&signed_net_hello(set, Some(s.to_string()), if i % 2 == 0 { 0xFFFF } else { 0 }), net_addr).await.unwrap();
            sent += 1;
        }
        let sock = from_ip(next()).await;
        sock.send_to(&signed_p2p_hello(s.to_string()), p2p_addr).await.unwrap();
        sent += 1;
    }
    // предельные длины
    for len in [1usize, 8, 254, 255] {
        let sock = from_ip(next()).await;
        let l = long(len);
        sock.send_to(&signed_net_hello([Some(l.clone()), Some(l.clone()), Some(l.clone()), Some(l.clone()), Some(l.clone())], Some(long(len.min(8))), 0xABCD), net_addr).await.unwrap();
        let sock = from_ip(next()).await;
        sock.send_to(&signed_p2p_hello(long(len)), p2p_addr).await.unwrap();
        sent += 2;
    }
    sleep(Duration::from_millis(2500)).await;
    assert_eq!(PANICS.load(Ordering::SeqCst), 0, "паника в обработчике приветствий (отправлено {sent})");
    // враждебные приветствия действительно дошли до обработчика (прошли подпись и допуск), а не отсеялись раньше
    let (net_in, p2p_in) = (net.hellos_admitted().await, p2p.hellos_admitted().await);
    assert!(net_in >= 100, "до обработчика основной сети дошло только {net_in} приветствий — тест не проверяет то, что должен");
    assert!(p2p_in >= 25, "до обработчика канала переписки дошло только {p2p_in} приветствий");

    // приём жив: честный новый узел после атаки принят в обеих сетях
    let honest = NodeIdentity::new();
    let h = from_ip(251).await;
    h.send_to(&honest_net(&honest), net_addr).await.unwrap();
    let honest_p2p = NodeIdentity::new();
    let h2 = from_ip(252).await;
    h2.send_to(&signed_p2p_hello_for(&honest_p2p), p2p_addr).await.unwrap();
    sleep(Duration::from_millis(1000)).await;
    assert!(net.get_peers_map().await.contains_key(&honest.node_id()), "после атаки основная сеть перестала принимать узлы");
    assert!(p2p.get_peer(&honest_p2p.node_id()).await.is_some(), "после атаки канал переписки перестал принимать узлы");
    assert_eq!(PANICS.load(Ordering::SeqCst), 0);
    let _ = std::panic::take_hook();
    std::panic::set_hook(prev);
}

fn signed_p2p_hello_for(id: &NodeIdentity) -> Vec<u8> {
    let mut h = P2PHelloPacket::new_request(id.node_id(), rand::random(), "127.0.0.1:9998".to_string(), id.signing_public_key);
    h.sign(id).unwrap();
    h.to_bytes().unwrap()
}
