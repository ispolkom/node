//! Враждебный собеседник: уже прошёл рукопожатие, имеет настоящий ключ сеанса и присылает расшифровываемые, но враждебные сообщения.
//!
//! Это самая опасная поверхность после подписи: расшифрованные данные идут в десятки обработчиков (DHT, ретрансляция, туннели, прокси, чат, звонки…),
//! и паника в одном из них останавливает приём для всех. Тест по-настоящему проходит рукопожатие с узлом по UDP, а затем шлёт тысячи сообщений всех
//! возможных типов (первый байт 0…255) разной длины и содержимого. Требуется: ни одной паники, узел жив и по-прежнему отвечает на пульс по тому же сеансу.
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout, Duration};

use yandi::core::identity::NodeIdentity;
use yandi::netlayer::encryption::EncryptionManager;
use yandi::netlayer::packet::{HelloPacket, HelloType, Signature};
use yandi::netlayer::peer::PeerInfo;
use yandi::netlayer::transport::{HelloEvent, P2PTransport};

static PANICS: AtomicUsize = AtomicUsize::new(0);

fn rng(seed: u64) -> impl FnMut() -> u64 {
    let mut s = seed;
    move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    }
}

/// Настоящее рукопожатие с узлом по UDP: приглашение с одноразовым ключом → подтверждение → общий ключ.
async fn handshake(disc: SocketAddr, victim_node_id: yandi::util::HashId) -> (yandi::util::HashId, UdpSocket, EncryptionManager) {
    let who = NodeIdentity::new();
    let id = who.node_id();
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut em = EncryptionManager::new(id);
    let mut cid = [0u8; 8];
    cid.copy_from_slice(&id.0[..8]);
    let mut hello = HelloPacket::new_request(id, who.signing_public_key, [0u8; 32], cid, 0);
    hello.x25519_public = em.generate_hello_ephemeral(hello.nonce);
    let sig = who.sign(&hello.challenge_data()).unwrap();
    let mut b = [0u8; 64];
    b.copy_from_slice(&sig);
    hello.signature = Signature(b);
    sock.send_to(&hello.to_bytes().unwrap(), disc).await.unwrap();
    let mut buf = vec![0u8; 65535];
    let (n, _) = timeout(Duration::from_secs(5), sock.recv_from(&mut buf)).await.expect("ack in time").unwrap();
    let ack = HelloPacket::from_bytes(&buf[..n]).expect("ack parses");
    assert_eq!(ack.hello_type, HelloType::Ack);
    em.complete_hello_initiator(ack.nonce, victim_node_id, &ack.x25519_public).expect("session");
    (id, sock, em)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hostile_peer_with_a_real_session_cannot_stop_the_node() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        PANICS.fetch_add(1, Ordering::SeqCst);
        eprintln!("PANIC: {info}");
    }));

    // приёмники, чтобы ветки передачи данных дальше тоже работали (и их тоже проверяли)
    let (chat_tx, mut chat_rx) = mpsc::channel(100_000);
    let (group_tx, mut group_rx) = mpsc::channel(100_000);
    let (tunnel_tx, mut tunnel_rx) = mpsc::channel(100_000);
    let (nack_tx, mut nack_rx) = mpsc::channel(100_000);
    let (relay_req_tx, mut relay_req_rx) = mpsc::channel(100_000);
    let (relay_data_tx, mut relay_data_rx) = mpsc::channel(100_000);
    tokio::spawn(async move { while chat_rx.recv().await.is_some() {} });
    tokio::spawn(async move { while group_rx.recv().await.is_some() {} });
    tokio::spawn(async move { while tunnel_rx.recv().await.is_some() {} });
    tokio::spawn(async move { while nack_rx.recv().await.is_some() {} });
    tokio::spawn(async move { while relay_req_rx.recv().await.is_some() {} });
    tokio::spawn(async move { while relay_data_rx.recv().await.is_some() {} });

    let victim_id = NodeIdentity::new();
    let victim_node_id = victim_id.node_id();
    let victim = P2PTransport::with_handlers(
        victim_id, 0, None, None, None, None, None, Some(nack_tx), None, None, None, None, None, None, Some(tunnel_tx), Some(chat_tx), Some(group_tx), None, None, None,
        Some(relay_req_tx), None, Some(relay_data_tx), 19541, 19542,
    )
    .await
    .unwrap();

    // «главный цикл» узла: отвечает на приглашения (как это делает main.rs)
    let mut hello_rx = victim.subscribe_hello();
    let v2 = victim.clone();
    tokio::spawn(async move {
        while let Ok(ev) = hello_rx.recv().await {
            if let HelloEvent::Request { from, packet } = ev {
                let _ = v2.send_hello_ack(&from.to_string(), packet.nonce, packet.node_id, packet.x25519_public).await;
            }
        }
    });

    // собеседник: настоящее рукопожатие
    let disc = SocketAddr::from(([127, 0, 0, 1], victim.discovery_addr().port()));
    let data = SocketAddr::from(([127, 0, 0, 1], victim.data_addr().port()));
    let (attacker_id, sock, mut em) = handshake(disc, victim_node_id).await;
    let victim_peer = PeerInfo::new(victim_node_id, "victim");
    let mut buf = vec![0u8; 65535];

    // враждебные сообщения всех типов
    let mut r = rng(0xDEADBEEFCAFEF00D);
    let lens = [0usize, 1, 2, 3, 8, 9, 16, 17, 31, 32, 33, 40, 64, 100, 200, 1200];
    let mut sent = 0u32;
    let (lo, hi) = match std::env::var("FUZZ_RANGE").ok().and_then(|v| v.split_once('-').map(|(a, b)| (a.parse::<u8>().unwrap_or(0), b.parse::<u8>().unwrap_or(255)))) {
        Some(r) => r,
        None => (0, 255),
    };
    for ty in lo..=hi {
        for &len in &lens {
            for shape in 0..3u8 {
                let mut payload = vec![ty];
                for _ in 0..len {
                    payload.push(match shape {
                        0 => r() as u8,
                        1 => 0xff,
                        _ => 0x00,
                    });
                }
                let frame = em.encrypt(&victim_peer, &payload).unwrap();
                sock.send_to(&frame, data).await.unwrap();
                sent += 1;
                if sent % 200 == 0 {
                    sleep(Duration::from_millis(15)).await; // не переполнять приёмный буфер
                }
            }
        }
    }
    sleep(Duration::from_millis(3000)).await;
    assert_eq!(PANICS.load(Ordering::SeqCst), 0, "паника в обработчиках расшифрованных сообщений (отправлено {sent})");

    // Враждебный собеседник мог навредить только себе: «обновление порта» с номером u64::MAX навсегда заморозило ЕГО порты (по протоколу принимается только больший номер),
    // и ответы ему идут на случайный порт. Узел при этом обязан быть жив для остальных: честный второй собеседник после атаки проходит рукопожатие и получает ответ на пульс.
    let (honest_id, hsock, mut hem) = handshake(disc, victim_node_id).await;
    let _ = honest_id;
    let mut hb = vec![0x01u8];
    hb.extend_from_slice(&777u64.to_be_bytes());
    hb.extend_from_slice(&0u64.to_be_bytes());
    let mut answered = false;
    for _ in 0..10 {
        hsock.send_to(&hem.encrypt(&victim_peer, &hb).unwrap(), data).await.unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline && !answered {
            if let Ok(Ok((n, _))) = timeout(Duration::from_millis(300), hsock.recv_from(&mut buf)).await {
                if let Ok((_from, plain)) = hem.decrypt_by_peer_id(&buf[..n]) {
                    if plain.first() == Some(&0x02) && plain.len() >= 9 && plain[1..9] == 777u64.to_be_bytes() {
                        answered = true;
                    }
                }
            }
        }
        if answered {
            break;
        }
    }
    if !answered {
        eprintln!("DIAG: враждебный собеседник ещё в таблице узла: {}", victim.get_peers_map().await.contains_key(&attacker_id));
    }
    assert!(answered, "после {sent} враждебных сообщений узел не принимает нового собеседника / не отвечает на пульс");
    assert_eq!(PANICS.load(Ordering::SeqCst), 0);
    let _ = std::panic::take_hook();
    std::panic::set_hook(prev);
}
