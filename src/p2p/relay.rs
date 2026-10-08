//! A UDP relay for the chat channel, for the pairs hole punching cannot connect (a symmetric NAT on either side; measured 0/100 in the lab).
//!
//! Shape (like TURN): a node that two others both have a session with allocates, for each of the two, a pair of UDP ports (one for the
//! discovery socket, one for the data socket) and forwards the datagrams arriving on one side's ports out of the other side's ports. For both
//! sides the other node is then simply "the relay's address, port N": the ordinary signed hello, the encryption, the chat and the keepalive work
//! on top of it unchanged, and the relay never sees anything but ciphertext it cannot read.
//!
//!   A --RelayReq(B)--> R     (sealed, A's session with R)
//!   R --RelayGrant(token, B, ports for A)--> A      R --RelayGrant(token, A, ports for B)--> B
//!   A and B send a few probes to their ports on R (this opens their NATs towards R and tells R their addresses), then the lower id starts the hello.
//!
//! What bounds it: only enabled relays serve (the owner's setting); an allocation exists only for two identities that both have a fresh,
//! public address on record at R; a datagram is forwarded only if it comes from the IP its side was seen at; one allocation per pair, a few per
//! node, a hard total; a byte budget per allocation per second; idle and hard lifetimes; nothing is ever sent to a stranger, and what is forwarded is
//! never larger than what came in (no amplification); no queues (a datagram is forwarded at once or dropped), so a slow receiver costs nothing.
use super::*;
use std::net::IpAddr;
use std::sync::atomic::AtomicU64;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

const PORTS_DEFAULT: (u16, u16) = (26200, 26999);
const MAX_ALLOCS: usize = 128;
const MAX_ALLOCS_PER_NODE: usize = 4;
const MAX_ROUTES: usize = 32;
pub(super) const IDLE_LIFE: Duration = Duration::from_secs(120);
const HARD_LIFE: Duration = Duration::from_secs(3600);
const BYTES_PER_SEC: usize = 2 * 1024 * 1024;
const REQ_EVERY: Duration = Duration::from_secs(5);
const GRANT_EVERY: Duration = Duration::from_secs(5);
const ROUTE_LIFE: Duration = Duration::from_secs(3600);

/// the port range of the relay sockets (opened in the machine's firewall by its owner); `YANDI_P2P_RELAY_PORTS=lo-hi` overrides
fn port_range() -> (u16, u16) {
    std::env::var("YANDI_P2P_RELAY_PORTS")
        .ok()
        .and_then(|v| {
            let (a, b) = v.split_once('-')?;
            let (a, b): (u16, u16) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
            (a >= 1024 && a <= b).then_some((a, b))
        })
        .unwrap_or(PORTS_DEFAULT)
}

struct Alloc {
    a: HashId,
    b: HashId,
    ports: [u16; 4],
    /// where the two sides are recorded; updated when an authenticated request shows a side at a new address
    ips: Arc<StdMutex<[IpAddr; 2]>>,
    created: Instant,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
pub(super) struct Route {
    pub relay: HashId,
    pub disc: SocketAddr,
    pub data: SocketAddr,
    pub until: Instant,
}

#[derive(Default)]
pub(super) struct RelayState {
    /// relay side: by token
    allocs: StdMutex<HashMap<[u8; 16], Alloc>>,
    /// relay side: when this pair was last granted
    granted: StdMutex<HashMap<(HashId, HashId), Instant>>,
    /// client side: when we last asked for a relay to this peer
    asked: StdMutex<HashMap<HashId, Instant>>,
    /// client side: the peers we currently reach through a relay
    pub(super) routes: StdMutex<HashMap<HashId, Route>>,
}

fn pair_key(a: &HashId, b: &HashId) -> (HashId, HashId) {
    if a.0 <= b.0 { (*a, *b) } else { (*b, *a) }
}

async fn bind_in_range() -> Option<(Arc<UdpSocket>, u16)> {
    let (lo, hi) = port_range();
    for _ in 0..40 {
        let port = lo + (rand::random::<u16>() % (hi - lo + 1));
        if let Ok(s) = UdpSocket::bind(("0.0.0.0", port)).await {
            return Some((Arc::new(s), port));
        }
    }
    None
}

/// Forwards between the four sockets: 0 = A's discovery port, 1 = A's data port, 2 = B's discovery port, 3 = B's data port. A datagram that arrives
/// on 0 leaves from 2 towards where B was seen, and so on. `ips` are the addresses the two sides were recorded at.
pub(super) async fn forward_loop(socks: [Arc<UdpSocket>; 4], ips: Arc<StdMutex<[IpAddr; 2]>>, deadline: Instant, idle: Duration) {
    let mut pins: [Option<SocketAddr>; 4] = [None; 4];
    let mut bufs = [vec![0u8; 65536], vec![0u8; 65536], vec![0u8; 65536], vec![0u8; 65536]];
    let (mut window, mut used) = (Instant::now(), 0usize);
    let mut last = Instant::now();
    let [b0, b1, b2, b3] = &mut bufs;
    loop {
        let left = deadline.saturating_duration_since(Instant::now()).min(idle.saturating_sub(last.elapsed()));
        if left.is_zero() {
            return;
        }
        let (i, n, src) = tokio::select! {
            r = socks[0].recv_from(b0) => match r { Ok((n, s)) => (0, n, s), Err(_) => continue },
            r = socks[1].recv_from(b1) => match r { Ok((n, s)) => (1, n, s), Err(_) => continue },
            r = socks[2].recv_from(b2) => match r { Ok((n, s)) => (2, n, s), Err(_) => continue },
            r = socks[3].recv_from(b3) => match r { Ok((n, s)) => (3, n, s), Err(_) => continue },
            _ = tokio::time::sleep(left) => return,
        };
        let side = i / 2;
        if src.ip() != ips.lock().unwrap()[side] {
            continue; // not from where this side was recorded: ignored, without a word
        }
        pins[i] = Some(src);
        let other = (1 - side) * 2 + i % 2;
        let Some(dst) = pins[other] else { continue }; // the other side has not shown itself yet
        if window.elapsed() >= Duration::from_secs(1) {
            window = Instant::now();
            used = 0;
        }
        used = used.saturating_add(n);
        if used > BYTES_PER_SEC {
            continue;
        }
        last = Instant::now();
        let data = match i {
            0 => &b0[..n],
            1 => &b1[..n],
            2 => &b2[..n],
            _ => &b3[..n],
        };
        let _ = socks[other].send_to(data, dst).await;
    }
}

fn grant_payload(token: &[u8; 16], peer: &HashId, ports: (u16, u16)) -> Vec<u8> {
    let mut v = token.to_vec();
    v.extend_from_slice(&peer.0);
    v.extend_from_slice(&ports.0.to_be_bytes());
    v.extend_from_slice(&ports.1.to_be_bytes());
    v
}

fn parse_grant(p: &[u8]) -> Option<([u8; 16], HashId, u16, u16)> {
    if p.len() != 16 + 32 + 4 {
        return None;
    }
    let token: [u8; 16] = p[..16].try_into().ok()?;
    let peer = HashId(p[16..48].try_into().ok()?);
    let d = u16::from_be_bytes([p[48], p[49]]);
    let c = u16::from_be_bytes([p[50], p[51]]);
    (d != 0 && c != 0).then_some((token, peer, d, c))
}

impl P2PTransport {
    /// Client: ask `via` (a node both we and the target have a session with, normally the one that introduced us) to relay between us and `target`.
    pub(super) async fn request_relay(&self, target: HashId, via: HashId) {
        let me = self.identity.node_id();
        if target == me || via == target {
            return;
        }
        {
            let mut asked = self.relay.asked.lock().unwrap();
            if asked.get(&target).is_some_and(|t| t.elapsed() < REQ_EVERY) {
                return;
            }
            asked.insert(target, Instant::now());
            if asked.len() > 1024 {
                asked.retain(|_, t| t.elapsed() < Duration::from_secs(60));
            }
        }
        println!("[relay] 🧭 no direct way to {}: asking {} to relay", hex::encode(&target.0[..8]), hex::encode(&via.0[..8]));
        let pkt = P2PPacket::new(P2PPacketType::RelayReq, me, false, target.0.to_vec());
        let _ = self.send_packet_dual_path(via, pkt).await;
    }

    /// Do we already reach this peer through a relay?
    pub(super) fn relay_route_to(&self, peer: &HashId) -> bool {
        self.relay.routes.lock().unwrap().get(peer).is_some_and(|r| r.until > Instant::now())
    }

    /// Relay: `a` asks to be connected to the node in the payload.
    pub(super) async fn handle_relay_req(&self, a: HashId, payload: &[u8]) {
        if !crate::relay_net::relay_enabled_cached() {
            return;
        }
        let Ok(t) = <[u8; 32]>::try_from(payload) else { return };
        let b = HashId(t);
        let me = self.identity.node_id();
        if b == a || b == me || a == me {
            return;
        }
        let key = pair_key(&a, &b);
        // both must be known here, recently and at public addresses
        let (ea, eb) = {
            let peers = self.peers.lock().await;
            match (peers.get(&a).and_then(|p| super::punch::endpoints(p)), peers.get(&b).and_then(|p| super::punch::endpoints(p))) {
                (Some(x), Some(y)) => (x, y),
                _ => return,
            }
        };
        let ip_of = |n: &HashId| if *n == a { ea.0.ip() } else { eb.0.ip() };
        // one allocation per pair: a second request is answered with the same grant (rate-limited), and shows where the sides are NOW
        let existing = {
            let allocs = self.relay.allocs.lock().unwrap();
            allocs.iter().find(|(_, x)| pair_key(&x.a, &x.b) == key && !x.task.is_finished()).map(|(tok, x)| {
                *x.ips.lock().unwrap() = [ip_of(&x.a), ip_of(&x.b)];
                (*tok, x.a, x.ports)
            })
        };
        {
            let mut g = self.relay.granted.lock().unwrap();
            if g.get(&key).is_some_and(|t| t.elapsed() < GRANT_EVERY) {
                return;
            }
            g.insert(key, Instant::now());
            if g.len() > 2048 {
                g.retain(|_, t| t.elapsed() < Duration::from_secs(60));
            }
        }
        let (token, a_side, ports) = match existing {
            Some(x) => x,
            None => {
                {
                    let allocs = self.relay.allocs.lock().unwrap();
                    let live = allocs.values().filter(|x| !x.task.is_finished()).count();
                    let per = |n: &HashId| allocs.values().filter(|x| !x.task.is_finished() && (x.a == *n || x.b == *n)).count();
                    if live >= MAX_ALLOCS || per(&a) >= MAX_ALLOCS_PER_NODE || per(&b) >= MAX_ALLOCS_PER_NODE {
                        return;
                    }
                }
                let mut socks = Vec::new();
                let mut ports = [0u16; 4];
                for i in 0..4 {
                    let Some((s, p)) = bind_in_range().await else { return };
                    ports[i] = p;
                    socks.push(s);
                }
                let socks: [Arc<UdpSocket>; 4] = socks.try_into().ok().unwrap();
                let token: [u8; 16] = rand::random();
                let deadline = Instant::now() + HARD_LIFE;
                // side 0 of the allocation is `a` (the one who asked first)
                let ips = Arc::new(StdMutex::new([ea.0.ip(), eb.0.ip()]));
                let task = tokio::spawn(forward_loop(socks, ips.clone(), deadline, IDLE_LIFE));
                println!("[relay] 🔁 allocated ports {:?} between {} and {}", ports, hex::encode(&a.0[..8]), hex::encode(&b.0[..8]));
                self.relay.allocs.lock().unwrap().insert(token, Alloc { a, b, ports, ips, created: Instant::now(), task });
                (token, a, ports)
            }
        };
        // the ports of side A are the first two, those of side B the last two
        let (pa, pb) = ((ports[0], ports[1]), (ports[2], ports[3]));
        let (to_a_ports, to_b_ports) = if a_side == a { (pa, pb) } else { (pb, pa) };
        let to_a = P2PPacket::new(P2PPacketType::RelayGrant, me, false, grant_payload(&token, &b, to_a_ports));
        let to_b = P2PPacket::new(P2PPacketType::RelayGrant, me, false, grant_payload(&token, &a, to_b_ports));
        let _ = self.send_packet_dual_path(a, to_a).await;
        let _ = self.send_packet_dual_path(b, to_b).await;
    }

    /// Either side: the relay tells which of its ports lead to `peer`. Open our NAT towards them and start the ordinary hello.
    pub(super) async fn handle_relay_grant(&self, relay: HashId, payload: &[u8]) {
        let Some((token, peer, port_disc, port_data)) = parse_grant(payload) else { return };
        let me = self.identity.node_id();
        if peer == me || peer == relay {
            return;
        }
        let relay_ip = {
            let peers = self.peers.lock().await;
            match peers.get(&relay).and_then(|p| p.p2p_data_addr.as_deref()?.parse::<SocketAddr>().ok()) {
                Some(a) if crate::netlayer::nat::is_public_ip(a.ip()) => a.ip(),
                _ => return,
            }
        };
        let (disc, data) = (SocketAddr::new(relay_ip, port_disc), SocketAddr::new(relay_ip, port_data));
        {
            let mut routes = self.relay.routes.lock().unwrap();
            routes.retain(|_, r| r.until > Instant::now());
            if routes.len() >= MAX_ROUTES && !routes.contains_key(&peer) {
                return;
            }
            if routes.get(&peer).is_some_and(|r| r.disc == disc && r.data == data) {
                // the same grant again: nothing new to start
                return;
            }
            routes.insert(peer, Route { relay, disc, data, until: Instant::now() + ROUTE_LIFE });
        }
        // the hello that follows creates the peer with the relay's data port as its data address
        self.punch.note_data_addr(peer, data);
        println!("[relay] 🔗 reaching {} through {}: ports {} / {} (token {})", hex::encode(&peer.0[..8]), hex::encode(&relay.0[..8]), port_disc, port_data, hex::encode(&token[..4]));
        let Some(this) = self.self_ref.get().and_then(|w| w.upgrade()) else { return };
        let lower = me.0 < peer.0;
        tokio::spawn(async move {
            let probe = {
                let mut b = vec![0xE2u8];
                b.extend_from_slice(&[0u8; 16]);
                b.extend_from_slice(&this.identity.node_id().0);
                b
            };
            for i in 0..40u32 {
                if this.peers.lock().await.get(&peer).is_some_and(|p| p.p2p_data_addr.as_deref() == Some(&data.to_string())) && i > 4 {
                    // the peer is known at the relay's data port: the hello went through
                    break;
                }
                let _ = this.discovery_socket.send_to(&probe, disc).await;
                let _ = this.data_send_socket.send_to(&probe, data).await;
                // the lower id starts; the higher one joins in if nothing has happened for a while
                if (lower && i >= 3 && i % 10 == 3) || (!lower && i >= 15 && i % 10 == 5) {
                    let _ = this.send_hello_request(&disc.to_string()).await;
                }
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        });
    }

    /// Housekeeping, called every keepalive round: finished and over-age allocations go; relayed peers get a discovery keepalive.
    pub(super) async fn relay_housekeeping(&self) {
        {
            let mut allocs = self.relay.allocs.lock().unwrap();
            allocs.retain(|_, x| {
                let dead = x.task.is_finished() || x.created.elapsed() > HARD_LIFE + Duration::from_secs(5);
                if dead {
                    x.task.abort();
                }
                !dead
            });
        }
        let routes: Vec<Route> = {
            let mut r = self.relay.routes.lock().unwrap();
            r.retain(|_, x| x.until > Instant::now());
            r.values().cloned().collect()
        };
        if routes.is_empty() {
            return;
        }
        let mut probe = vec![0xE2u8];
        probe.extend_from_slice(&[0u8; 16]);
        probe.extend_from_slice(&self.identity.node_id().0);
        for r in routes {
            let _ = self.discovery_socket.send_to(&probe, r.disc).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn sock(ip: &str) -> Arc<UdpSocket> {
        Arc::new(UdpSocket::bind((ip, 0)).await.unwrap())
    }

    /// four relay sockets on loopback, two "sides" at 127.0.0.1 and 127.0.0.2
    async fn rig() -> ([Arc<UdpSocket>; 4], Arc<UdpSocket>, Arc<UdpSocket>, tokio::task::JoinHandle<()>) {
        let relay = [sock("127.0.0.1").await, sock("127.0.0.1").await, sock("127.0.0.1").await, sock("127.0.0.1").await];
        let a = sock("127.0.0.1").await;
        let b = sock("127.0.0.2").await;
        let ips = Arc::new(StdMutex::new(["127.0.0.1".parse().unwrap(), "127.0.0.2".parse().unwrap()]));
        let h = tokio::task::spawn(forward_loop(relay.clone(), ips.clone(), Instant::now() + Duration::from_secs(30), Duration::from_secs(30)));
        (relay, a, b, h)
    }

    async fn recv(s: &UdpSocket) -> Option<(Vec<u8>, SocketAddr)> {
        let mut b = vec![0u8; 2048];
        tokio::time::timeout(Duration::from_millis(400), s.recv_from(&mut b)).await.ok()?.ok().map(|(n, f)| (b[..n].to_vec(), f))
    }

    #[tokio::test]
    async fn datagrams_cross_the_relay_both_ways_from_the_port_of_the_receiving_side() {
        let (relay, a, b, _h) = rig().await;
        let (ra, rb) = (relay[1].local_addr().unwrap(), relay[3].local_addr().unwrap()); // the data ports of A and of B
        // B shows itself first (a datagram to its port); nothing is forwarded yet because A is not known
        b.send_to(b"hi from b", rb).await.unwrap();
        assert!(recv(&a).await.is_none());
        a.send_to(b"hello b", ra).await.unwrap();
        let (got, from) = recv(&b).await.expect("forwarded to B");
        assert_eq!(got, b"hello b");
        assert_eq!(from, rb, "B sees the datagram coming from its own port on the relay");
        b.send_to(b"hello a", rb).await.unwrap();
        let (got, from) = recv(&a).await.expect("forwarded to A");
        assert_eq!(got, b"hello a");
        assert_eq!(from, ra);
    }

    #[tokio::test]
    async fn a_datagram_from_a_stranger_changes_nothing_and_gets_no_answer() {
        let (relay, a, b, _h) = rig().await;
        let (ra, rb) = (relay[1].local_addr().unwrap(), relay[3].local_addr().unwrap());
        b.send_to(b"x", rb).await.unwrap();
        a.send_to(b"y", ra).await.unwrap();
        let _ = recv(&b).await;
        let _ = recv(&a).await;
        let stranger = sock("127.0.0.3").await;
        stranger.send_to(b"evil", ra).await.unwrap();
        stranger.send_to(b"evil", rb).await.unwrap();
        assert!(recv(&stranger).await.is_none(), "no answer");
        assert!(recv(&a).await.is_none() && recv(&b).await.is_none(), "nothing forwarded");
        // and the sides still work
        a.send_to(b"still", ra).await.unwrap();
        assert_eq!(recv(&b).await.unwrap().0, b"still");
    }

    #[tokio::test]
    async fn the_discovery_and_the_data_ports_are_kept_apart() {
        let (relay, a, b, _h) = rig().await;
        let (ra_d, rb_d) = (relay[0].local_addr().unwrap(), relay[2].local_addr().unwrap());
        let (ra_x, rb_x) = (relay[1].local_addr().unwrap(), relay[3].local_addr().unwrap());
        b.send_to(b"p", rb_d).await.unwrap();
        a.send_to(b"q", ra_d).await.unwrap();
        let _ = recv(&b).await;
        // B has shown itself on its discovery port only: a datagram on A's DATA port has nowhere to go
        a.send_to(b"data", ra_x).await.unwrap();
        assert!(recv(&b).await.is_none());
        b.send_to(b"p2", rb_x).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await; // B's data port is known to the relay before A's datagram arrives
        a.send_to(b"data2", ra_x).await.unwrap();
        assert_eq!(recv(&b).await.unwrap().0, b"data2");
    }

    #[tokio::test]
    async fn the_byte_budget_cuts_a_flood_and_an_idle_relay_ends() {
        let relay = [sock("127.0.0.1").await, sock("127.0.0.1").await, sock("127.0.0.1").await, sock("127.0.0.1").await];
        let a = sock("127.0.0.1").await;
        let b = sock("127.0.0.2").await;
        let ips = Arc::new(StdMutex::new(["127.0.0.1".parse().unwrap(), "127.0.0.2".parse().unwrap()]));
        let h = tokio::task::spawn(forward_loop(relay.clone(), ips.clone(), Instant::now() + Duration::from_secs(30), Duration::from_millis(1500)));
        let (ra, rb) = (relay[1].local_addr().unwrap(), relay[3].local_addr().unwrap());
        b.send_to(b"x", rb).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await; // B is known to the relay before the flood starts
        let big = vec![7u8; 60_000];
        for _ in 0..80 {
            a.send_to(&big, ra).await.unwrap(); // 4.8 MB in a burst: the budget is 2 MiB per second
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut bytes = 0;
        while let Some((d, _)) = recv(&b).await {
            bytes += d.len();
            if bytes > 3 * 1024 * 1024 {
                break;
            }
        }
        assert!(bytes <= 2 * 1024 * 1024 + 60_000, "{bytes} bytes got through");
        assert!(bytes > 0);
        tokio::time::sleep(Duration::from_millis(4000)).await; // idle life 1.5 s, with a wide margin for a loaded machine
        assert!(h.is_finished(), "an idle allocation ends by itself");
    }

    #[tokio::test]
    async fn a_side_that_shows_up_at_a_new_address_is_served_once_the_record_is_updated() {
        let relay = [sock("127.0.0.1").await, sock("127.0.0.1").await, sock("127.0.0.1").await, sock("127.0.0.1").await];
        let (a, b) = (sock("127.0.0.1").await, sock("127.0.0.2").await);
        let ips = Arc::new(StdMutex::new(["127.0.0.1".parse().unwrap(), "127.0.0.2".parse().unwrap()]));
        let _h = tokio::task::spawn(forward_loop(relay.clone(), ips.clone(), Instant::now() + Duration::from_secs(30), Duration::from_secs(30)));
        let (ra, rb) = (relay[1].local_addr().unwrap(), relay[3].local_addr().unwrap());
        b.send_to(b"x", rb).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        a.send_to(b"1", ra).await.unwrap();
        assert_eq!(recv(&b).await.unwrap().0, b"1");
        // A's router now shows a new outside address: its datagrams are ignored until an authenticated request says so
        let a2 = sock("127.0.0.4").await;
        a2.send_to(b"2", ra).await.unwrap();
        assert!(recv(&b).await.is_none());
        ips.lock().unwrap()[0] = "127.0.0.4".parse().unwrap();
        a2.send_to(b"3", ra).await.unwrap();
        assert_eq!(recv(&b).await.unwrap().0, b"3");
        b.send_to(b"back", rb).await.unwrap();
        assert_eq!(recv(&a2).await.unwrap().0, b"back", "and the replies now go to the new address");
    }

    #[test]
    fn a_grant_round_trips_and_nothing_else_parses() {
        let id = HashId([5; 32]);
        let p = grant_payload(&[1; 16], &id, (26300, 26301));
        assert_eq!(parse_grant(&p), Some(([1; 16], id, 26300, 26301)));
        for n in 0..p.len() {
            assert!(parse_grant(&p[..n]).is_none());
        }
        assert!(parse_grant(&grant_payload(&[1; 16], &id, (0, 26301))).is_none(), "port 0 is no port");
        assert_eq!(pair_key(&HashId([1; 32]), &HashId([2; 32])), pair_key(&HashId([2; 32]), &HashId([1; 32])));
    }
}
