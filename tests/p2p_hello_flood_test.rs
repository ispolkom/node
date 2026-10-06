//! Наплыв поддельных личностей на канал переписки (p2p): с одного адреса принимается не больше положенного числа новых узлов в минуту.
//! (Все сценарии в одной функции: порты p2p-транспорта берутся из переменных окружения процесса.)
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tokio::time::{sleep, Duration};

use yandi::core::identity::NodeIdentity;
use yandi::p2p::hello::P2PHelloPacket;
use yandi::p2p::transport::P2PTransport;

fn signed_hello(identity: &NodeIdentity) -> Vec<u8> {
    let mut hello = P2PHelloPacket::new_request(identity.node_id(), rand::random(), "127.0.0.1:0".to_string(), identity.signing_public_key);
    hello.sign(identity).expect("sign");
    hello.to_bytes().expect("serialize")
}

fn loopback_of(bind_addr: &str) -> SocketAddr {
    let port: u16 = bind_addr.rsplit(':').next().unwrap().parse().unwrap();
    SocketAddr::from(([127, 0, 0, 1], port))
}

async fn settle(t: &P2PTransport) -> (u64, u64) {
    let mut last = (u64::MAX, u64::MAX);
    let mut stable = 0;
    for _ in 0..300 {
        let now = (t.hellos_admitted().await, t.hellos_refused().await);
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

#[tokio::test]
async fn p2p_flood_of_fresh_identities_is_limited_and_honest_nodes_still_get_in() {
    std::env::set_var("YANDI_P2P_DISCOVERY_PORT", "19511");
    std::env::set_var("YANDI_P2P_DATA_PORT", "19512");
    let node = P2PTransport::new(NodeIdentity::new(), 0).await.expect("start transport");
    let target = loopback_of(&node.discovery_addr());

    let flooder = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    for _ in 0..2000 {
        flooder.send_to(&signed_hello(&NodeIdentity::new()), target).await.unwrap();
    }
    let (admitted, refused) = settle(&node).await;
    let in_table = node.list_peers().await.len();
    assert!(admitted <= 10, "с одного адреса пропущено {admitted} новых личностей (предел 10 в минуту; обработано {})", admitted + refused);
    assert!(admitted >= 1, "первые приветствия принимаются");
    assert!(refused >= 100, "наплыв отклоняется (отклонено {refused})");
    assert!(in_table <= 10, "в таблице {in_table} записей");

    // честный узел с другого адреса принимается
    let honest = UdpSocket::bind("127.0.0.2:0").await.unwrap();
    let friend = NodeIdentity::new();
    honest.send_to(&signed_hello(&friend), target).await.unwrap();
    sleep(Duration::from_millis(500)).await;
    assert!(node.get_peer(&friend.node_id()).await.is_some(), "честный новый узел с другого адреса принят");
}
