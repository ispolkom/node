//! Чужие байты не должны ронять узел.
//!
//! Каждый разборщик входящих данных получает пустые, предельные, случайные и искажённые входы. Любая паника, зависание или попытка занять
//! гигабайты по чужому «полю длины» — ошибка. Каждый разборщик проверяется в отдельном процессе с ограничением памяти (2 ГБ) и времени:
//! так видно и аварийное завершение, которое внутри одного процесса поймать нельзя.
//!
//! Родитель запускает по одному ребёнку на разборщик (`child_decoder` с переменной `YANDI_PROBE=<имя>`).
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

type Probe = Box<dyn Fn(&[u8])>;

fn probe<F: Fn(&[u8]) + 'static>(f: F) -> Probe {
    Box::new(f)
}

fn decoders() -> Vec<(&'static str, Probe)> {
    use yandi::communication::protocol::{CommPacket, GroupPacket};
    use yandi::dataplane::stream::{StreamFrame, StreamHeader};
    use yandi::dht::messages::{DhtQuery, DhtResponse};
    use yandi::dht::record::NodeRecord;
    use yandi::netlayer::packet::{HelloPacket, NetPacket};
    use yandi::netlayer::pairing;
    use yandi::netlayer::transport::PortUpdatePacket;
    use yandi::p2p::hello::P2PHelloPacket;
    use yandi::p2p::packet::P2PPacket;
    use yandi::p2p_tunnel::protocol::TunnelPacket;
    use yandi::protocol::station::BatchedWagon;
    use yandi::protocol::tcp_station::TcpWagon;
    use yandi::protocol::tcp_transport::TcpPacketHeader;
    use yandi::protocol::wagon::Wagon;
    use yandi::socks5::proxy_protocol as pp;
    use yandi::socks5::protocol as sp;

    macro_rules! d {
        ($name:expr, $f:expr) => {
            ($name, probe(move |b: &[u8]| $f(b)))
        };
    }
    let mut v = vec![
        // цепочки через несколько узлов, записи доступности, список входных узлов, карточки: чужие байты не роняют разбор
        d!("hops::normalize", |b| drop(yandi::hops::normalize(b))),
        d!("hops::Router::on_packet", |b| {
            let mut r = yandi::hops::Router::new([1; 32], ed25519_dalek::SigningKey::from_bytes(&[1; 32]));
            drop(r.on_packet([2; 32], b, 0))
        }),
        d!("relay_net::receive_packet", |b| drop(yandi::relay_net::receive_packet(&mut yandi::relay_net::ViaStore::default(), b, 0))),
        d!("bootstrap::parse", |b| drop(yandi::bootstrap::parse(b))),
        d!("network_offers::receive_packet", |b| drop(yandi::network_offers::receive_packet(&mut yandi::network_offers::Directory::default(), b, 0))),
        d!("NetPacket", |b| drop(NetPacket::from_bytes(b))),
        d!("HelloPacket", |b| drop(HelloPacket::from_bytes(b))),
        d!("PortUpdatePacket", |b| drop(PortUpdatePacket::from_bytes(b))),
        d!("P2PPacket", |b| drop(P2PPacket::from_bytes(b))),
        d!("P2PHelloPacket", |b| drop(P2PHelloPacket::from_bytes(b))),
        d!("CommPacket", |b| drop(CommPacket::from_bytes(b))),
        d!("GroupPacket", |b| drop(GroupPacket::from_bytes(b))),
        d!("TunnelPacket", |b| drop(TunnelPacket::from_bytes(b))),
        d!("Wagon", |b| drop(Wagon::from_bytes(b))),
        d!("BatchedWagon", |b| drop(BatchedWagon::from_bytes(b))),
        d!("TcpWagon", |b| drop(TcpWagon::from_bytes(b))),
        d!("TcpPacketHeader", |b| drop(TcpPacketHeader::from_bytes(b))),
        d!("StreamHeader", |b| drop(StreamHeader::from_bytes(b))),
        d!("StreamFrame", |b| drop(StreamFrame::from_bytes(b))),
        d!("DhtQuery", |b| drop(DhtQuery::from_bytes(b))),
        d!("DhtResponse", |b| drop(DhtResponse::from_bytes(b))),
        d!("NodeRecord", |b| drop(NodeRecord::from_bytes(b))),
        d!("decode_resume", |b| drop(pairing::decode_resume(b))),
        d!("decode_session_issue", |b| drop(pairing::decode_session_issue(b))),
        d!("decode_resume_ack", |b| drop(pairing::decode_resume_ack(b))),
        d!("Socks5Request", |b| drop(sp::Socks5Request::from_bytes(b))),
        d!("Socks5AuthSelect", |b| drop(sp::Socks5AuthSelect::from_bytes(b))),
        d!("ConnectRequest", |b| drop(pp::ConnectRequest::from_bytes(b))),
        d!("ConnectResponse", |b| drop(pp::ConnectResponse::from_bytes(b))),
        d!("DataMessage", |b| drop(pp::DataMessage::from_bytes(b))),
        d!("CloseMessage", |b| drop(pp::CloseMessage::from_bytes(b))),
    ];
    // заведомо плохие «разборщики» — только для проверки самой проверки (см. harness_detects_failures)
    if std::env::var("YANDI_SELFTEST").is_ok() {
        v.push(d!("selftest_panic", |b: &[u8]| {
            if b.len() == 4 {
                panic!("специально");
            }
        }));
        v.push(d!("selftest_alloc", |b: &[u8]| {
            if b.len() == 4 {
                let v: Vec<u8> = Vec::with_capacity(8 << 30);
                drop(v);
            }
        }));
        v.push(d!("selftest_hang", |b: &[u8]| {
            if b == [0xaa, 0, 0, 0] {
                std::thread::sleep(Duration::from_secs(1));
            }
        }));
    }
    v
}

/// Детерминированный генератор (одинаковые входы при каждом запуске).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

fn corpus() -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = vec![vec![], vec![0], vec![0xff], vec![0; 4], vec![0xff; 4], vec![0; 64], vec![0xff; 64], vec![0x7f; 1024], vec![0xff; 70_000], vec![0; 70_000]];
    let mut r = Rng(0x9e3779b97f4a7c15);
    // первый байт — тип: перебор всех значений с разными хвостами (чтобы пройти диспетчеризацию по типу)
    for first in 0..=255u8 {
        out.push(vec![first]);
        for tail in [0usize, 1, 2, 3, 4, 8, 16, 33, 100, 300] {
            let mut v = vec![first];
            v.extend(r.bytes(tail));
            out.push(v);
            let mut z = vec![first];
            z.extend(std::iter::repeat(0xff).take(tail));
            out.push(z);
            let mut zero = vec![first];
            zero.extend(std::iter::repeat(0).take(tail));
            out.push(zero);
        }
    }
    // «поля длины»: после типа — огромные значения в разных местах
    for first in [0u8, 1, 2, 3, 4, 5, 0xa0, 0xc0, 0xd0] {
        for pos in 1..24usize {
            let mut v = vec![first];
            v.extend(r.bytes(40));
            for k in 0..4 {
                if pos + k < v.len() {
                    v[pos + k] = 0xff;
                }
            }
            out.push(v);
        }
    }
    for _ in 0..4000 {
        let len = match r.next() % 10 {
            0..=4 => (r.next() % 64) as usize,
            5..=8 => (r.next() % 2048) as usize,
            _ => (r.next() % 70_000) as usize,
        };
        out.push(r.bytes(len));
    }
    out
}

#[test]
#[ignore = "запускается из decoders_survive_hostile_bytes в отдельном процессе"]
fn child_decoder() {
    let name = std::env::var("YANDI_PROBE").expect("YANDI_PROBE");
    // потолок памяти: попытка занять больше — это ошибка разборщика
    unsafe {
        let lim = libc::rlimit { rlim_cur: 2 << 30, rlim_max: 2 << 30 };
        libc::setrlimit(libc::RLIMIT_AS, &lim);
    }
    let all = decoders();
    let (_, f) = all.iter().find(|(n, _)| *n == name).expect("unknown decoder");
    std::panic::set_hook(Box::new(|_| {}));
    let mut bad: Vec<String> = vec![];
    for input in corpus() {
        let t = Instant::now();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&input)));
        let dt = t.elapsed();
        if r.is_err() && bad.len() < 3 {
            bad.push(format!("паника на {} байтах: {}", input.len(), hex(&input[..input.len().min(24)])));
        }
        if dt > Duration::from_millis(500) && bad.len() < 3 {
            bad.push(format!("медленно ({dt:?}) на {} байтах: {}", input.len(), hex(&input[..input.len().min(24)])));
        }
    }
    if !bad.is_empty() {
        eprintln!("PROBE-FAIL {name}: {}", bad.join(" | "));
        std::process::exit(3);
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn run_probes(names: Vec<&'static str>, selftest: bool) -> Vec<String> {
    let exe = std::env::current_exe().unwrap();
    let mut failures: Vec<String> = vec![];
    // по несколько процессов сразу, чтобы не ждать долго
    let mut running: Vec<(&str, std::process::Child, Instant)> = vec![];
    let mut queue = names.clone();
    let mut finish = |name: &str, out: std::process::Output, failures: &mut Vec<String>| {
        let text = String::from_utf8_lossy(&out.stderr).to_string();
        if !out.status.success() {
            let why = text.lines().find(|l| l.starts_with("PROBE-FAIL")).map(String::from).unwrap_or_else(|| format!("процесс завершился аварийно ({:?}); {}", out.status, text.lines().last().unwrap_or("")));
            failures.push(format!("{name}: {why}"));
        }
    };
    while !queue.is_empty() || !running.is_empty() {
        while running.len() < 6 && !queue.is_empty() {
            let name = queue.remove(0);
            let child = Command::new(&exe).args(["child_decoder", "--exact", "--ignored", "--nocapture", "--test-threads=1"]).env("YANDI_PROBE", name).envs(if selftest { Some(("YANDI_SELFTEST", "1")) } else { None }).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
            running.push((name, child, Instant::now()));
        }
        let mut i = 0;
        while i < running.len() {
            let timed_out = running[i].2.elapsed() > Duration::from_secs(120);
            if timed_out {
                let (name, mut c, _) = running.remove(i);
                let _ = c.kill();
                let _ = c.wait();
                failures.push(format!("{name}: завис (больше 120 с)"));
                continue;
            }
            match running[i].1.try_wait().unwrap() {
                Some(_) => {
                    let (name, c, _) = running.remove(i);
                    let out = c.wait_with_output().unwrap();
                    finish(name, out, &mut failures);
                }
                None => i += 1,
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    failures.sort();
    failures
}

#[test]
fn decoders_survive_hostile_bytes() {
    let names: Vec<&'static str> = decoders().iter().map(|(n, _)| *n).collect();
    let failures = run_probes(names, false);
    assert!(failures.is_empty(), "\nразборщики, которые не выдержали чужие байты ({}):\n  {}\n", failures.len(), failures.join("\n  "));
}

/// Проверка самой проверки: паника, попытка занять 8 ГБ и зависание обязаны быть пойманы.
#[test]
fn harness_detects_failures() {
    let failures = run_probes(vec!["selftest_panic", "selftest_alloc", "selftest_hang"], true);
    let joined = failures.join("\n");
    assert!(joined.contains("selftest_panic: PROBE-FAIL"), "паника не поймана:\n{joined}");
    assert!(joined.contains("selftest_alloc"), "огромное выделение памяти не поймано:\n{joined}");
    assert!(joined.contains("selftest_hang: PROBE-FAIL") && joined.contains("медленно"), "зависание не поймано:\n{joined}");
    assert_eq!(failures.len(), 3, "{joined}");
}
