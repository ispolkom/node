//! Наплыв поддельных личностей: тысячи подписанных приветствий от разных (бесплатно созданных) узлов с одного адреса не должны раздувать таблицу пиров.
//!
//! Без допуска каждое такое приветствие занимало место в таблице пиров, в DHT и в списке виденных одноразовых номеров. Здесь живой UDP на одной машине:
//! с одного адреса принимается не больше положенного числа новых личностей в минуту, а узел с другого адреса и уже известные узлы продолжают работать.
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tokio::time::{sleep, Duration};

use yandi::core::identity::NodeIdentity;
use yandi::netlayer::packet::{HelloPacket, Signature};
use yandi::netlayer::transport::P2PTransport;

fn signed_hello(identity: &NodeIdentity) -> Vec<u8> {
    let node_id = identity.node_id();
    let mut cid = [0u8; 8];
    cid.copy_from_slice(&node_id.0[..8]);
    let mut hello = HelloPacket::new_request(node_id, identity.signing_public_key, rand::random(), cid, 0);
    let sig = identity.sign(&hello.challenge_data()).expect("sign");
    let mut b = [0u8; 64];
    b.copy_from_slice(&sig);
    hello.signature = Signature(b);
    hello.to_bytes().expect("serialize")
}

async fn spawn_transport(discovery_port: u16, data_port: u16) -> std::sync::Arc<P2PTransport> {
    P2PTransport::with_handlers(
        NodeIdentity::new(), 0, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, discovery_port, data_port,
    )
    .await
    .expect("start transport")
}

/// Ждать, пока узел перестанет обрабатывать приветствия (число обработанных не меняется 600 мс подряд, но не дольше 30 с).
async fn settle(node: &P2PTransport) -> (u64, u64) {
    let mut last = (u64::MAX, u64::MAX);
    let mut stable = 0;
    for _ in 0..300 {
        let now = (node.hellos_admitted().await, node.hellos_refused().await);
        if now == last {
            stable += 1;
            if stable >= 6 {
                return now;
            }
        } else {
            stable = 0;
            last = now;
        }
        sleep(Duration::from_millis(100)).await;
    }
    last
}

async fn bind(ip: &str) -> (UdpSocket, SocketAddr) {
    let s = UdpSocket::bind(format!("{ip}:0")).await.expect("bind");
    let a = s.local_addr().unwrap();
    (s, a)
}

#[tokio::test]
async fn a_flood_of_fresh_identities_from_one_address_does_not_fill_the_peer_table() {
    let node = spawn_transport(19411, 19412).await;
    let target = node.discovery_addr();
    let (flooder, _) = bind("127.0.0.1").await;
    for _ in 0..2000 {
        flooder.send_to(&signed_hello(&NodeIdentity::new()), target).await.unwrap();
    }
    let (admitted, refused) = settle(&node).await;
    let known = node.get_peers_map().await.len();
    assert!(admitted <= 10, "с одного адреса пропущено {admitted} новых личностей (предел 10 в минуту; обработано {})", admitted + refused);
    assert!(admitted >= 1 && known >= 1 && known <= 10, "первые приветствия принимаются: пропущено {admitted}, в таблице {known}");
    assert!(refused >= 100, "наплыв отклоняется допуском (отклонено {refused})");

    // другой адрес (другой источник) по-прежнему принимается: общая квота не исчерпана
    let (honest, _) = bind("127.0.0.2").await;
    let friend = NodeIdentity::new();
    honest.send_to(&signed_hello(&friend), target).await.unwrap();
    sleep(Duration::from_millis(500)).await;
    assert!(node.get_peers_map().await.contains_key(&friend.node_id()), "честный новый узел с другого адреса принят");

    // уже известный узел обновляется (в пределах своей частоты)
    let before = node.get_peers_map().await.len();
    honest.send_to(&signed_hello(&friend), target).await.unwrap();
    sleep(Duration::from_millis(300)).await;
    assert_eq!(node.get_peers_map().await.len(), before, "повтор известного узла не добавляет записей");
}

#[tokio::test]
async fn a_known_peer_cannot_use_hellos_to_flood() {
    let node = spawn_transport(19421, 19422).await;
    let target = node.discovery_addr();
    let (s, _) = bind("127.0.0.1").await;
    let me = NodeIdentity::new();
    for _ in 0..300 {
        s.send_to(&signed_hello(&me), target).await.unwrap();
    }
    let (admitted, refused) = settle(&node).await;
    assert_eq!(node.get_peers_map().await.len(), 1);
    // из присланных приветствий одного и того же узла пропущено не больше положенного в минуту (первое — новая личность, остальные — обновления, до 20)
    assert!(admitted <= 21, "лимит частоты известного узла не сработал: пропущено {admitted}, обработано {}", admitted + refused);
    assert!(refused >= 50, "отклонено только {refused}");
}
