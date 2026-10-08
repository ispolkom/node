//! Hole punching for the chat channel (measured on the lab: two port-preserving NATs behind different routers connect in ~50 ms, see
//! chaos/punchtest.py; a punched path dies after 30 s of silence, hence the 15 s keepalive).
//!
//! The scheme, with a node every side already has a session with as the introducer:
//!   A --PunchReq(B)--> I          (encrypted, A's session with I)
//!   I --PunchIntro(token, B's outside addresses)--> A     and     I --PunchIntro(token, A's outside addresses)--> B
//!   A and B both send small probes to the other's addresses for ~3 s; a probe that comes back with the token proves the way is open
//!   and starts the ordinary hello (key exchange, signatures, pinned keys). A probe proves NOTHING about who sent it.
//!
//! What stops this from being used against us: a probe is acted on only if its token was handed out by an introducer we have a session
//! with, only from the address of the peer it was meant for, and only once per token; introductions are accepted only for public addresses,
//! at a limited rate, and a limited number at a time. A probe is 49 bytes and never answered with anything bigger.
use super::*;
use std::net::IpAddr;
use std::sync::atomic::AtomicUsize;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

pub(super) const PROBE_MAGIC: u8 = 0xE2;
pub(super) const PROBE_LEN: usize = 1 + 16 + 32;
const ATTEMPT_LIFE: Duration = Duration::from_secs(15);
const PROBE_EVERY: Duration = Duration::from_millis(100);
const PROBES: u32 = 30;
const MAX_ATTEMPTS: usize = 16;
const ASK_EVERY: Duration = Duration::from_secs(8);
const INTRO_PER_PAIR: Duration = Duration::from_secs(5);
const INTRO_PER_INTRODUCER: Duration = Duration::from_secs(2);
const MAX_INTRODUCERS_ASKED: usize = 3;
const MAX_HOME_NODES: usize = 3;
/// how long the address a probe came from is trusted as the peer's data address
const SEEN_LIFE: Duration = Duration::from_secs(60);
/// an address learned from a genuine packet is fresh for this long (ms) when an introducer vouches for it
const FRESH_MS: u128 = 60_000;
/// how long a challenge to a new address waits for its answer, and how soon the same address is challenged again
const CHALLENGE_LIFE: Duration = Duration::from_secs(10);
const CHALLENGE_AGAIN: Duration = Duration::from_secs(3);

struct Attempt {
    peer: HashId,
    ips: Vec<IpAddr>,
    expires: Instant,
    hello_sent: bool,
    introducer: HashId,
}

#[derive(Default)]
pub(super) struct PunchState {
    expected: StdMutex<HashMap<[u8; 16], Attempt>>,
    /// client side: when we last asked for an introduction to this peer
    asked: StdMutex<HashMap<HashId, Instant>>,
    /// introducer side: when this pair was last introduced
    introduced: StdMutex<HashMap<(HashId, HashId), Instant>>,
    /// client side: when an introduction from this introducer was last accepted
    intro_from: StdMutex<HashMap<HashId, Instant>>,
    /// the outside data address a probe of this peer came from (or, for a relayed peer, the relay's port)
    data_seen: StdMutex<HashMap<HashId, (SocketAddr, Instant)>>,
    running: AtomicUsize,
    /// path validation: a peer's packets came from this new address and it was challenged with this number
    candidates: StdMutex<HashMap<HashId, (SocketAddr, [u8; 8], Instant)>>,
    /// when the quick follow-up request after greeting a home node was last made for this target (at most once in a while)
    followup: StdMutex<HashMap<HashId, Instant>>,
}

impl PunchState {
    /// Remember the data address to use for `peer` when its hello completes (a relay's port, for instance).
    pub(super) fn note_data_addr(&self, peer: HashId, addr: SocketAddr) {
        self.data_seen.lock().unwrap().insert(peer, (addr, Instant::now()));
    }

    /// The data address to use for a peer that has just completed a hello: the one its probe came from, if that is recent; otherwise the guess
    /// from its hello.
    pub(super) fn data_addr_for(&self, peer: &HashId, declared: &str, seen_ip: IpAddr) -> String {
        if let Some((addr, at)) = self.data_seen.lock().unwrap().get(peer) {
            if at.elapsed() < SEEN_LIFE {
                return addr.to_string();
            }
        }
        observed_data_addr(declared, seen_ip)
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

fn public(a: &SocketAddr) -> bool {
    crate::netlayer::nat::is_public_ip(a.ip()) && a.port() != 0
}

/// the two outside addresses (data, discovery) of a peer, if both are known, public and were confirmed by a genuine packet a short while ago
pub(super) fn endpoints(p: &P2PPeer) -> Option<(SocketAddr, SocketAddr)> {
    let data: SocketAddr = p.p2p_data_addr.as_deref()?.parse().ok()?;
    let disc: SocketAddr = p.addr.parse().ok()?;
    if !public(&data) || !public(&disc) || now_ms().saturating_sub(p.last_seen) > FRESH_MS {
        return None;
    }
    Some((data, disc))
}

fn intro_payload(token: &[u8; 16], peer: &HashId, data: &SocketAddr, disc: &SocketAddr) -> Vec<u8> {
    let (d, c) = (data.to_string(), disc.to_string());
    let mut out = token.to_vec();
    out.extend_from_slice(&peer.0);
    out.push(d.len() as u8);
    out.extend_from_slice(d.as_bytes());
    out.push(c.len() as u8);
    out.extend_from_slice(c.as_bytes());
    out
}

fn parse_intro(p: &[u8]) -> Option<([u8; 16], HashId, SocketAddr, SocketAddr)> {
    if p.len() < 16 + 32 + 2 {
        return None;
    }
    let token: [u8; 16] = p[..16].try_into().ok()?;
    let peer = HashId(p[16..48].try_into().ok()?);
    let dl = *p.get(48)? as usize;
    let d = std::str::from_utf8(p.get(49..49 + dl)?).ok()?.parse().ok()?;
    let cl = *p.get(49 + dl)? as usize;
    let c = std::str::from_utf8(p.get(50 + dl..50 + dl + cl)?).ok()?.parse().ok()?;
    if p.len() != 50 + dl + cl {
        return None;
    }
    Some((token, peer, d, c))
}

fn probe(token: &[u8; 16], me: &HashId) -> Vec<u8> {
    let mut b = vec![PROBE_MAGIC];
    b.extend_from_slice(token);
    b.extend_from_slice(&me.0);
    b
}

impl P2PTransport {
    /// Ask the nodes we have a session with to introduce us to `target`. Called when the chat cannot reach it. Cheap to call repeatedly: one request
    /// per target every few seconds.
    pub async fn request_punch(&self, target: HashId) {
        let me = self.identity.node_id();
        if target == me {
            return;
        }
        {
            let mut asked = self.punch.asked.lock().unwrap();
            if asked.get(&target).is_some_and(|t| t.elapsed() < ASK_EVERY) {
                return;
            }
            asked.insert(target, Instant::now());
            if asked.len() > 1024 {
                asked.retain(|_, t| t.elapsed() < Duration::from_secs(60));
            }
        }
        self.ask_introducers(target).await;
        // and make sure we are connected to the nodes the target is connected to: they know it
        self.reach_home_nodes(target).await;
    }

    /// Ask the nodes we have a session with (up to three) to introduce us to `target`.
    async fn ask_introducers(&self, target: HashId) {
        let me = self.identity.node_id();
        let known: Vec<HashId> = self.peers.lock().await.keys().filter(|id| **id != target).copied().collect();
        let mut introducers = Vec::new();
        {
            let enc = self.p2p_encryption.lock().await;
            for id in known {
                if enc.has_session(&id) {
                    introducers.push(id);
                    if introducers.len() >= MAX_INTRODUCERS_ASKED {
                        break;
                    }
                }
            }
        }
        if introducers.is_empty() {
            println!("[punch] ⚠️  nobody to ask for an introduction to {} (no session with any other node)", hex::encode(&target.0[..8]));
        }
        for intro in introducers {
            println!("[punch] 🙋 asking {} to introduce me to {}", hex::encode(&intro.0[..8]), hex::encode(&target.0[..8]));
            let pkt = P2PPacket::new(P2PPacketType::PunchReq, me, false, target.0.to_vec());
            let _ = self.send_packet_dual_path(intro, pkt).await;
        }
    }

    /// A contact behind a NAT is reachable through the nodes it keeps links to (its home nodes, announced in its signed availability record). If we
    /// have no session with such a node yet, greet it: the next request for an introduction can then go through it, because it knows the contact.
    async fn reach_home_nodes(&self, target: HashId) {
        let target_hex = hex::encode(target.0);
        let Some(key_hex) = crate::web::peers::key_hex_of(&target_hex) else { return };
        let homes = crate::relay_net::lookup_relays(&target_hex, &key_hex);
        let me = self.identity.node_id();
        let mut greeted = false;
        for h in homes.iter().take(MAX_HOME_NODES) {
            let Some(id) = hex::decode(h).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()).map(HashId) else { continue };
            if id == me || id == target || self.peers.lock().await.contains_key(&id) {
                continue;
            }
            let Some(addr) = crate::network_offers::p2p_discovery_addr(h) else { continue };
            println!("[punch] 🏠 {} is reached through its home node {}: greeting {}", hex::encode(&target.0[..8]), &h[..16], addr);
            let _ = self.send_hello_request(&addr.to_string()).await;
            greeted = true;
        }
        let quick = greeted && {
            let mut f = self.punch.followup.lock().unwrap();
            let ok = f.get(&target).map_or(true, |t| t.elapsed() > Duration::from_secs(20));
            if ok {
                f.insert(target, Instant::now());
                if f.len() > 1024 {
                    f.retain(|_, t| t.elapsed() < Duration::from_secs(60));
                }
            }
            ok
        };
        if quick {
            // the new session is there in a moment: ask again at once instead of waiting for the next round of the chat's resends
            if let Some(this) = self.self_ref.get().and_then(|w| w.upgrade()) {
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    this.ask_introducers(target).await;
                });
            }
        }
    }

    /// Introducer: `a` wants to reach the node in the payload. If both are known here (recently heard, public addresses), each gets the other's addresses.
    pub(super) async fn handle_punch_req(&self, a: HashId, payload: &[u8]) {
        let Ok(t) = <[u8; 32]>::try_from(payload) else { return };
        let b = HashId(t);
        let me = self.identity.node_id();
        if b == a || b == me || a == me {
            return;
        }
        if self.punch.introduced.lock().unwrap().get(&(a, b)).is_some_and(|t| t.elapsed() < INTRO_PER_PAIR) {
            return;
        }
        let (ea, eb) = {
            let peers = self.peers.lock().await;
            match (peers.get(&a).and_then(endpoints), peers.get(&b).and_then(endpoints)) {
                (Some(x), Some(y)) => (x, y),
                (xa, xb) => {
                    // one of them is not known well enough here: no answer to the asker (it asks others), but the reason goes to the log
                    println!("[punch] introducer: cannot introduce {} to {}: {} {}", hex::encode(&a.0[..8]), hex::encode(&b.0[..8]),
                        if xa.is_none() { "the asker is not known here (recently, at a public address)" } else { "" },
                        if xb.is_none() { "the target is not known here (recently, at a public address)" } else { "" });
                    return;
                }
            }
        };
        {
            // the pair is "used up" only by an introduction that was actually made
            let mut gate = self.punch.introduced.lock().unwrap();
            gate.insert((a, b), Instant::now());
            if gate.len() > 4096 {
                gate.retain(|_, t| t.elapsed() < Duration::from_secs(60));
            }
        }
        let token: [u8; 16] = rand::random();
        let to_a = P2PPacket::new(P2PPacketType::PunchIntro, me, false, intro_payload(&token, &b, &eb.0, &eb.1));
        let to_b = P2PPacket::new(P2PPacketType::PunchIntro, me, false, intro_payload(&token, &a, &ea.0, &ea.1));
        let _ = self.send_packet_dual_path(a, to_a).await;
        let _ = self.send_packet_dual_path(b, to_b).await;
    }

    /// Either side: an introducer told us where to find a peer. Start the probes.
    pub(super) async fn handle_punch_intro(&self, introducer: HashId, payload: &[u8]) {
        let Some((token, peer, data, disc)) = parse_intro(payload) else { return };
        let me = self.identity.node_id();
        if peer == me || peer == introducer || !public(&data) || !public(&disc) {
            return;
        }
        {
            let mut from = self.punch.intro_from.lock().unwrap();
            if from.get(&introducer).is_some_and(|t| t.elapsed() < INTRO_PER_INTRODUCER) {
                return;
            }
            from.insert(introducer, Instant::now());
            if from.len() > 1024 {
                from.retain(|_, t| t.elapsed() < Duration::from_secs(60));
            }
        }
        {
            let mut ex = self.punch.expected.lock().unwrap();
            ex.retain(|_, a| a.expires > Instant::now());
            if ex.len() >= MAX_ATTEMPTS || ex.contains_key(&token) {
                return; // too many at once, or the same introduction arriving a second time (the two paths of a dual send)
            }
            ex.insert(token, Attempt { peer, ips: vec![data.ip(), disc.ip()], expires: Instant::now() + ATTEMPT_LIFE, hello_sent: false, introducer });
        }
        if self.punch.running.fetch_add(1, Ordering::Relaxed) >= MAX_ATTEMPTS {
            self.punch.running.fetch_sub(1, Ordering::Relaxed);
            return;
        }
        println!("[punch] 🤝 introduced to {} by {}: probing {} and {}", hex::encode(&peer.0[..8]), hex::encode(&introducer.0[..8]), data, disc);
        let (data_sock, disc_sock, state) = (self.data_send_socket.clone(), self.discovery_socket.clone(), self.punch.clone());
        let this = self.self_ref.get().and_then(|w| w.upgrade());
        let msg = probe(&token, &me);
        tokio::spawn(async move {
            for _ in 0..PROBES {
                if !state.expected.lock().unwrap().contains_key(&token) {
                    break;
                }
                let _ = data_sock.send_to(&msg, data).await;
                let _ = disc_sock.send_to(&msg, disc).await;
                tokio::time::sleep(PROBE_EVERY).await;
            }
            state.running.fetch_sub(1, Ordering::Relaxed);
            // three seconds of probes and the other side never answered: there is no direct way. Ask the introducer to relay.
            // (when several introducers introduced the same pair, only the one with the lowest id is asked: both sides then pick the same relay)
            let failed = {
                let ex = state.expected.lock().unwrap();
                ex.get(&token).is_some_and(|a| !a.hello_sent)
                    && !ex.values().any(|o| o.peer == peer && o.expires > Instant::now() && o.introducer.0 < introducer.0)
            };
            if failed {
                if let Some(this) = this {
                    println!("[punch] ❌ no direct way to {}", hex::encode(&peer.0[..8]));
                    // the side with the lower id asks; the other waits for the grant that follows and asks only if none comes (one allocation per pair)
                    if me.0 > peer.0 {
                        tokio::time::sleep(Duration::from_secs(4)).await;
                        if this.relay_route_to(&peer) {
                            return;
                        }
                    }
                    this.request_relay(peer, introducer).await;
                }
            }
        });
    }

    /// A probe arrived (on the data socket or on the discovery socket). It is acted on only if it matches an introduction.
    pub(super) async fn on_punch_probe(&self, data: &[u8], from: SocketAddr, on_discovery: bool) {
        if data.len() != PROBE_LEN || data[0] != PROBE_MAGIC {
            return;
        }
        let token: [u8; 16] = data[1..17].try_into().unwrap();
        let sender = HashId(data[17..49].try_into().unwrap());
        let send_hello = {
            let mut ex = self.punch.expected.lock().unwrap();
            let Some(a) = ex.get_mut(&token) else { return };
            if a.expires <= Instant::now() || a.peer != sender || !a.ips.contains(&from.ip()) {
                return;
            }
            if on_discovery && !a.hello_sent {
                a.hello_sent = true;
                true
            } else {
                false
            }
        };
        let me = self.identity.node_id();
        if on_discovery {
            // answer so that our own NAT opens towards them too, then do the ordinary introduction
            let _ = self.discovery_socket.send_to(&probe(&token, &me), from).await;
            if send_hello {
                println!("[punch] ✅ the way is open to {} ({}): starting the hello", hex::encode(&sender.0[..8]), from);
                let _ = self.send_hello_request(&from.to_string()).await;
            }
        } else {
            self.punch.data_seen.lock().unwrap().insert(sender, (from, Instant::now()));
            let _ = self.data_send_socket.send_to(&probe(&token, &me), from).await;
        }
    }
}

/// What to do with a genuine packet that came from `seen` when the peer is known at `current` (path validation).
#[derive(Debug, PartialEq, Eq)]
pub(super) enum AddrAction {
    /// the packet came from the known address
    Same,
    /// nothing is known yet: take it
    Take,
    /// a different address: ask it to prove itself, with this number
    Challenge([u8; 8]),
    /// that address was challenged a moment ago: wait for the answer
    Waiting,
}

pub(super) fn decide_address(current: Option<&str>, seen: &SocketAddr, candidate: Option<&(SocketAddr, [u8; 8], Instant)>, fresh_token: [u8; 8]) -> AddrAction {
    match current {
        None => AddrAction::Take,
        Some(c) if c == seen.to_string() => AddrAction::Same,
        Some(_) => match candidate {
            Some((addr, _, at)) if addr == seen && at.elapsed() < CHALLENGE_AGAIN => AddrAction::Waiting,
            _ => AddrAction::Challenge(fresh_token),
        },
    }
}

impl P2PTransport {
    /// A genuine packet of `peer` arrived from `from`. The known address is changed only after the peer has answered a challenge sent to the new
    /// one: whoever merely forwards a captured packet cannot answer (the answer is sealed with the session key), so replies cannot be steered.
    pub(super) async fn learn_address(&self, peer: HashId, from: SocketAddr) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let action = {
            let mut peers = self.peers.lock().await;
            let Some(p) = peers.get_mut(&peer) else { return };
            let cand = self.punch.candidates.lock().unwrap().get(&peer).cloned();
            let act = decide_address(p.p2p_data_addr.as_deref(), &from, cand.as_ref(), rand::random());
            match &act {
                AddrAction::Same => {
                    p.last_seen = now;
                    self.punch.candidates.lock().unwrap().remove(&peer);
                }
                AddrAction::Take => {
                    p.p2p_data_addr = Some(from.to_string());
                    p.last_seen = now;
                }
                _ => {}
            }
            act
        };
        if let AddrAction::Challenge(token) = action {
            {
                let mut c = self.punch.candidates.lock().unwrap();
                c.insert(peer, (from, token, Instant::now()));
                if c.len() > 1024 {
                    c.retain(|_, (_, _, t)| t.elapsed() < CHALLENGE_LIFE);
                }
            }
            println!("[path] packets of {} now come from {}: challenging that address", hex::encode(&peer.0[..8]), from);
            self.send_sealed_to(peer, P2PPacketType::PathChallenge, token.to_vec(), from).await;
        }
    }

    /// Seal a small packet with the session of `peer` and send it to one given address.
    async fn send_sealed_to(&self, peer: HashId, kind: P2PPacketType, payload: Vec<u8>, to: SocketAddr) {
        let sealed = {
            let enc = self.p2p_encryption.lock().await;
            if !enc.has_session(&peer) {
                return;
            }
            enc.encrypt(&peer, &seal_prefix(kind.to_byte(), &payload))
        };
        let Ok(ct) = sealed else { return };
        let mut pkt = P2PPacket::new(kind, self.identity.node_id(), true, ct);
        pkt.line_id = 0;
        let _ = self.data_send_socket.send_to(&pkt.to_bytes(), to).await;
    }

    /// The peer asks us to prove that we are at the address its challenge reached: answer to the address it came from.
    pub(super) async fn on_path_challenge(&self, peer: HashId, from: SocketAddr, payload: &[u8]) {
        if payload.len() != 8 {
            return;
        }
        self.send_sealed_to(peer, P2PPacketType::PathResponse, payload.to_vec(), from).await;
    }

    /// The answer came back: if it came from the address that was challenged and carries its number, that address becomes the peer's.
    pub(super) async fn on_path_response(&self, peer: HashId, from: SocketAddr, payload: &[u8]) {
        let ok = {
            let mut c = self.punch.candidates.lock().unwrap();
            match c.get(&peer) {
                Some((addr, token, at)) if *addr == from && token[..] == *payload && at.elapsed() < CHALLENGE_LIFE => {
                    c.remove(&peer);
                    true
                }
                _ => false,
            }
        };
        if !ok {
            return;
        }
        let mut peers = self.peers.lock().await;
        if let Some(p) = peers.get_mut(&peer) {
            println!("[path] {} confirmed at {}", hex::encode(&peer.0[..8]), from);
            p.p2p_data_addr = Some(from.to_string());
            p.last_seen = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(b: u8) -> HashId {
        HashId([b; 32])
    }

    #[test]
    fn an_introduction_survives_the_trip_and_nothing_else_parses() {
        let (d, c): (SocketAddr, SocketAddr) = ("203.0.113.5:26104".parse().unwrap(), "203.0.113.5:26103".parse().unwrap());
        let p = intro_payload(&[7; 16], &id(9), &d, &c);
        let (token, peer, d2, c2) = parse_intro(&p).expect("round trip");
        assert_eq!((token, peer, d2, c2), ([7; 16], id(9), d, c));
        // every cut and every extension of a good message is refused
        for n in 0..p.len() {
            assert!(parse_intro(&p[..n]).is_none(), "cut at {n}");
        }
        let mut longer = p.clone();
        longer.push(0);
        assert!(parse_intro(&longer).is_none());
        // lengths that point outside the message
        let mut bad = p.clone();
        bad[48] = 200;
        assert!(parse_intro(&bad).is_none());
        // not an address
        let mut junk = p.clone();
        junk[49] = b'x';
        assert!(parse_intro(&junk).is_none());
    }

    #[test]
    fn a_probe_has_exactly_the_agreed_shape_and_is_not_a_p2p_packet() {
        let b = probe(&[1; 16], &id(2));
        assert_eq!(b.len(), PROBE_LEN);
        assert_eq!(b[0], PROBE_MAGIC);
        assert!(P2PPacket::from_bytes(&b).is_none(), "shorter than any packet: it can never be mistaken for one");
        assert!(PROBE_LEN < P2P_PACKET_HEADER_LEN + 1);
    }

    #[test]
    fn only_public_addresses_are_ever_probed() {
        for bad in ["10.0.0.1:1000", "192.168.1.5:1000", "127.0.0.1:1000", "169.254.1.1:1000", "0.0.0.0:1000", "203.0.113.5:0"] {
            assert!(!public(&bad.parse().unwrap()), "{bad}");
        }
        assert!(public(&"203.0.113.5:1000".parse().unwrap()));
    }

    #[test]
    fn the_address_a_probe_came_from_beats_the_guess_from_the_hello_but_only_while_fresh() {
        let st = PunchState::default();
        let ip: IpAddr = "203.0.113.5".parse().unwrap();
        assert_eq!(st.data_addr_for(&id(1), "192.168.1.5:26104", ip), "203.0.113.5:26104", "the guess");
        st.data_seen.lock().unwrap().insert(id(1), ("203.0.113.5:40000".parse().unwrap(), Instant::now()));
        assert_eq!(st.data_addr_for(&id(1), "192.168.1.5:26104", ip), "203.0.113.5:40000", "the fresh observation");
        assert_eq!(st.data_addr_for(&id(2), "192.168.1.5:26104", ip), "203.0.113.5:26104", "another peer is not affected");
    }

    #[test]
    fn a_changed_address_is_challenged_not_believed() {
        let a: SocketAddr = "203.0.113.5:26104".parse().unwrap();
        let b: SocketAddr = "198.51.100.9:40000".parse().unwrap();
        let t = [7u8; 8];
        assert_eq!(decide_address(None, &a, None, t), AddrAction::Take, "nothing known: take it");
        assert_eq!(decide_address(Some("203.0.113.5:26104"), &a, None, t), AddrAction::Same);
        assert_eq!(decide_address(Some("203.0.113.5:26104"), &b, None, t), AddrAction::Challenge(t), "a new address must prove itself");
        let pending = (b, [1u8; 8], Instant::now());
        assert_eq!(decide_address(Some("203.0.113.5:26104"), &b, Some(&pending), t), AddrAction::Waiting, "and is not challenged again at once");
        let other = ("192.0.2.1:1".parse().unwrap(), [1u8; 8], Instant::now());
        assert_eq!(decide_address(Some("203.0.113.5:26104"), &b, Some(&other), t), AddrAction::Challenge(t), "a challenge to another address does not cover this one");
    }
}
