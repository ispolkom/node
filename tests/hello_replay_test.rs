//! Node Identity Binding Fix — Barrier 3 live exploit + regression test.
//!
//! Proves, with real UDP sockets on this one machine (no second machine,
//! no WAN, no NAT, no relay — none of that is needed or tested here):
//! a captured, byte-identical, validly-signed Hello packet from a REAL
//! pinned peer can be resent later from a DIFFERENT source address within
//! the existing ±5 minute freshness window, and — before this fix — would
//! silently redirect that peer's registered address (and DHT entry) to
//! the attacker's address, without the attacker ever possessing the real
//! peer's private key. `check_replay()` in `netlayer/transport.rs` closes
//! this by rejecting any (node_id, nonce) pair already accepted.

use std::collections::HashMap;
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tokio::time::{sleep, Duration};

use yandi::core::identity::NodeIdentity;
use yandi::netlayer::packet::{HelloPacket, Signature};
use yandi::netlayer::transport::P2PTransport;

fn make_identity() -> NodeIdentity {
    NodeIdentity::new()
}

/// Build a genuine, validly-signed Hello Request from `identity` — exactly
/// the bytes that identity's own real node would put on the wire.
fn build_signed_hello(identity: &NodeIdentity) -> Vec<u8> {
    let node_id = identity.node_id();
    let mut cid = [0u8; 8];
    cid.copy_from_slice(&node_id.0[..8]);
    let x25519_public: [u8; 32] = rand::random();

    let mut hello = HelloPacket::new_request(
        node_id,
        identity.signing_public_key,
        x25519_public,
        cid,
        0,
    );

    let challenge = hello.challenge_data();
    let signature = identity.sign(&challenge).expect("sign");
    let mut sig_bytes = [0u8; 64];
    sig_bytes.copy_from_slice(&signature);
    hello.signature = Signature(sig_bytes);

    hello.to_bytes().expect("serialize hello")
}

async fn spawn_bare_udp_socket() -> (UdpSocket, SocketAddr) {
    let sock = UdpSocket::bind("127.0.0.1:0").await.expect("bind attacker socket");
    let addr = sock.local_addr().expect("local_addr");
    (sock, addr)
}

async fn spawn_transport(
    identity: NodeIdentity,
    discovery_port: u16,
    data_port: u16,
) -> std::sync::Arc<P2PTransport> {
    P2PTransport::with_handlers(
        identity, 0,
        None, // exit_handler_tx
        None, // proxy_gateway_tx
        None, // proxy_request_tx
        None, // proxy_response_tx
        None, // proxy_tunnel_data_tx
        None, // nack_tx
        None, // socks5_gateway_tx
        None, // socks5_request_tx
        None, // socks5_response_tx
        None, // socks5_tunnel_data_tx
        None, // tun_wagon_tx
        None, // tun_wagon_resp_tx
        None, // p2p_tunnel_tx
        None, // chat_packet_tx
        None, // group_packet_tx
        None, // new_peer_tx
        None, // external_ip
        None, // topology
        None, // relay_request_tx
        None, // relay_response_tx
        None, // relay_data_tx
        discovery_port, data_port,
    )
    .await
    .expect("start transport")
}

/// A captured, valid Hello from a pinned peer, resent later from a
/// different address, must NOT be accepted as a fresh handshake — it must
/// not overwrite that peer's registered address.
#[tokio::test]
async fn replayed_hello_from_different_address_is_rejected() {
    let identity_a = make_identity();
    let victim_identity = make_identity(); // the real, pinned peer being impersonated by replay

    let transport_a = spawn_transport(identity_a, 19301, 19302).await;

    // Pin the victim's real node_id -> real signing key, exactly as
    // main.rs does from trusted_ai_peers.json.
    let mut pins = HashMap::new();
    pins.insert(victim_identity.node_id(), victim_identity.signing_public_key);
    transport_a.set_pinned_identities(pins).await;

    let discovery_addr_a = transport_a.discovery_addr();

    // The REAL peer's one genuine Hello, captured once (this is the exact
    // byte sequence the victim's real node would send).
    let captured_hello_bytes = build_signed_hello(&victim_identity);

    // 1) The real peer's own send, from its own real address.
    let (real_sock, real_addr) = spawn_bare_udp_socket().await;
    real_sock
        .send_to(&captured_hello_bytes, discovery_addr_a)
        .await
        .expect("send real hello");
    sleep(Duration::from_millis(300)).await;

    let peer_after_real = transport_a
        .get_peer(victim_identity.node_id())
        .await
        .expect("peer should be known after the real Hello");
    assert!(
        peer_after_real.addr.to_string().contains(&real_addr.port().to_string()),
        "peer addr should reflect the real sender's address after the first, genuine Hello"
    );

    // 2) An attacker, who never had the victim's private key, replays the
    // *exact same bytes* from a different socket/address.
    let (attacker_sock, attacker_addr) = spawn_bare_udp_socket().await;
    assert_ne!(attacker_addr.port(), real_addr.port());
    attacker_sock
        .send_to(&captured_hello_bytes, discovery_addr_a)
        .await
        .expect("send replayed hello");
    sleep(Duration::from_millis(300)).await;

    let peer_after_replay = transport_a
        .get_peer(victim_identity.node_id())
        .await
        .expect("peer still known");

    assert!(
        peer_after_replay.addr.to_string().contains(&real_addr.port().to_string()),
        "REPLAY SUCCEEDED: peer addr was hijacked to the attacker's address {} (should still be the real sender {})",
        attacker_addr,
        real_addr,
    );
    assert!(
        !peer_after_replay.addr.to_string().contains(&attacker_addr.port().to_string()),
        "REPLAY SUCCEEDED: peer addr now points at the attacker's address"
    );
}

/// Two genuinely DIFFERENT Hellos from the same real peer (fresh nonce
/// each time, exactly like a real reconnect after an IP change) must both
/// still be accepted — the fix must not break legitimate reconnection.
#[tokio::test]
async fn two_genuinely_fresh_hellos_from_same_peer_both_accepted() {
    let identity_a = make_identity();
    let peer_identity = make_identity();

    let transport_a = spawn_transport(identity_a, 19311, 19312).await;
    let discovery_addr_a = transport_a.discovery_addr();

    // First genuine Hello, from address 1.
    let (sock1, addr1) = spawn_bare_udp_socket().await;
    let hello1 = build_signed_hello(&peer_identity);
    sock1.send_to(&hello1, discovery_addr_a).await.expect("send hello1");
    sleep(Duration::from_millis(300)).await;

    let peer_after_1 = transport_a
        .get_peer(peer_identity.node_id())
        .await
        .expect("peer known after first hello");
    assert!(peer_after_1.addr.to_string().contains(&addr1.port().to_string()));

    // Second genuine Hello (fresh nonce/timestamp — build_signed_hello
    // calls HelloPacket::new_request again), from a NEW real address —
    // simulating the peer's own legitimate IP change / reconnect.
    let (sock2, addr2) = spawn_bare_udp_socket().await;
    let hello2 = build_signed_hello(&peer_identity);
    assert_ne!(hello1, hello2, "each build_signed_hello call must produce a fresh nonce");
    sock2.send_to(&hello2, discovery_addr_a).await.expect("send hello2");
    sleep(Duration::from_millis(300)).await;

    let peer_after_2 = transport_a
        .get_peer(peer_identity.node_id())
        .await
        .expect("peer known after second hello");
    assert!(
        peer_after_2.addr.to_string().contains(&addr2.port().to_string()),
        "legitimate reconnection from a new real address must still update the peer's address"
    );
}
