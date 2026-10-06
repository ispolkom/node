//! Тренировочная сеть: несколько копий узла на одном компьютере, чтобы налаживать и проверять связь между узлами
//! (переписка, файлы, звонки, проксирование, выход через другой узел), пока других настоящих узлов нет.
//!
//! ```text
//!     yandi testnet up [число узлов, 2–5, по умолчанию 3]   — поднять (или поднять снова) и познакомить узлы друг с другом
//!     yandi testnet status                                   — кто работает, кто кого видит, где открыть страницу каждого узла
//!     yandi testnet down                                     — остановить все копии
//!     yandi testnet reset                                    — остановить и стереть тренировочную сеть целиком
//! ```
//!
//! Всё живёт в отдельной папке (`~/.local/share/yandi-testnet`): у каждой копии своя «домашняя» папка, свои ключи, пароль, база и
//! порты (узел k: 26000 + 100·k и дальше). Настоящий узел владельца и его данные не трогаются.
//!
//! Первая копия — «якорь» (может быть выходом в интернет для остальных копий).
//! Копии работают в «тренировочном режиме» (`YANDI_TESTNET=1`): не объявляют себя в домашней сети (mDNS) и не открывают порты на
//! роутере (NAT-PMP). Знакомятся визитками (`/api/peers/trusted`) — тем же путём, каким владелец добавит настоящий узел.
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

/// Переменная окружения тренировочного режима копии узла.
pub const ENV: &str = "YANDI_TESTNET";
const DEFAULT_NODES: usize = 3;
const MAX_NODES: usize = 5;

/// Эта копия узла — из тренировочной сети.
pub fn active() -> bool {
    std::env::var_os(ENV).is_some()
}

/// Порты k-й копии (k с 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ports {
    pub web: u16,
    pub discovery: u16,
    pub data: u16,
    pub p2p_discovery: u16,
    pub p2p_data: u16,
    pub ws: u16,
    pub http_proxy: u16,
    pub mobile_gateway: u16,
    pub mobile_p2p: u16,
}

pub fn ports(k: usize) -> Ports {
    // ниже 32768: выше система раздаёт порты под временные исходящие соединения, и порт копии мог бы оказаться занят
    let b = 26000 + 100 * k as u16;
    Ports { web: b, discovery: b + 1, data: b + 2, p2p_discovery: b + 3, p2p_data: b + 4, ws: b + 6, http_proxy: b + 7, mobile_gateway: b + 8, mobile_p2p: b + 9 }
}

fn stand_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("YANDI_TESTNET_DIR") {
        return PathBuf::from(d);
    }
    let data = crate::util::data_dir::data_dir();
    data.parent().map(|p| p.join("yandi-testnet")).unwrap_or_else(|| data.join("testnet"))
}

fn node_dir(stand: &Path, k: usize) -> PathBuf {
    stand.join(format!("node{k}"))
}

fn config_yaml(p: &Ports) -> String {
    format!(
        "# тренировочная копия узла (yandi testnet)\nserver: {{bind_address: 127.0.0.1, log_level: info}}\nports: {{discovery: {}, data: {}, mobile_gateway: {}, mobile_p2p: {}, http_proxy: {}, web_ui: {}}}\nnetwork: {{public_ip: 127.0.0.1}}\nws: {{bind: \"127.0.0.1:{}\"}}\n",
        p.discovery, p.data, p.mobile_gateway, p.mobile_p2p, p.http_proxy, p.web, p.ws
    )
}

/// Окружение копии: своя домашняя папка и данные, свои порты, тренировочный режим.
fn node_env(dir: &Path, p: &Ports) -> Vec<(String, String)> {
    let d = dir.display().to_string();
    let no_proxy = ["127.0.0.1", "localhost", "::1"].join(",");
    vec![
        ("HOME".into(), format!("{d}/home")),
        ("XDG_DATA_HOME".into(), format!("{d}/home/.local/share")),
        ("XDG_CONFIG_HOME".into(), format!("{d}/home/.config")),
        ("YANDI_CONFIG".into(), format!("{d}/config.yaml")),
        ("YANDI_P2P_DISCOVERY_PORT".into(), p.p2p_discovery.to_string()),
        ("YANDI_P2P_DATA_PORT".into(), p.p2p_data.to_string()),
        ("YANDI_CORE_BIN".into(), "self".into()),
        (ENV.into(), "1".into()),
        ("NO_PROXY".into(), no_proxy.clone()),
        ("no_proxy".into(), no_proxy),
    ]
}

fn pid_alive_here(dir: &Path) -> Option<i32> {
    let pid: i32 = std::fs::read_to_string(dir.join("pid")).ok()?.trim().parse().ok()?;
    // та же программа и та же папка — иначе номер процесса уже занят кем-то другим
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    (cwd == dir).then_some(pid)
}

fn random_password() -> String {
    use rand::Rng;
    const A: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut r = rand::thread_rng();
    (0..20).map(|_| A[r.gen_range(0..A.len())] as char).collect()
}

/// Пароли копий (одинаковые у всех копий этой сети): создаются один раз, лежат только у владельца (права 0600).
fn passwords(stand: &Path) -> std::io::Result<(String, String)> {
    let f = stand.join("passwords.txt");
    if let Ok(s) = std::fs::read_to_string(&f) {
        let get = |label: &str| s.lines().find_map(|l| l.strip_prefix(label)).map(|v| v.trim().to_string());
        if let (Some(a), Some(b)) = (get("пароль входа:"), get("мастер-пароль:")) {
            return Ok((a, b));
        }
    }
    let (a, b) = (random_password(), random_password());
    crate::util::private_file::write_private(&f, format!("# пароли тренировочных копий узла (yandi testnet), одинаковые у всех копий\nпароль входа: {a}\nмастер-пароль: {b}\n").as_bytes())?;
    Ok((a, b))
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(30)).build().expect("http client")
}

async fn wait_web(c: &reqwest::Client, p: &Ports, secs: u64) -> bool {
    for _ in 0..secs * 2 {
        if c.get(format!("http://127.0.0.1:{}/login", p.web)).send().await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

fn session_cookie(r: &reqwest::Response) -> Option<String> {
    r.headers().get_all("set-cookie").iter().filter_map(|v| v.to_str().ok()).find(|v| v.starts_with("yandi_session=")).map(|v| v.split(';').next().unwrap_or("").to_string())
}

/// Войти в копию: первый запуск — создать пароли (как человек на странице первой настройки), иначе — обычный вход.
async fn sign_in(c: &reqwest::Client, dir: &Path, p: &Ports, login: &str, master: &str) -> Result<String, String> {
    let url = format!("http://127.0.0.1:{}", p.web);
    let first = !dir.join("home/.yandi_keys/auth.json").exists();
    for _ in 0..240 {
        let r = if first {
            c.post(format!("{url}/api/auth/setup")).json(&json!({"login_password": login, "login_password_repeat": login, "master_password": master, "master_password_repeat": master})).send().await
        } else {
            c.post(format!("{url}/api/auth/login")).json(&json!({"login_password": login})).send().await
        };
        if let Ok(r) = r {
            if r.status().is_success() {
                if let Some(cookie) = session_cookie(&r) {
                    return Ok(cookie);
                }
            } else if r.status().as_u16() != 404 {
                return Err(format!("вход не удался ({}): {}", r.status(), r.text().await.unwrap_or_default()));
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err("узел не ответил на вход".into())
}

/// После первой настройки узел продолжает запуск; ждём, пока заработает полная страница (с проверкой входа).
async fn wait_ready(c: &reqwest::Client, p: &Ports, cookie: &str, secs: u64) -> Option<Value> {
    for _ in 0..secs * 2 {
        if let Ok(r) = c.get(format!("http://127.0.0.1:{}/api/peers/trusted", p.web)).header("cookie", cookie).send().await {
            if r.status().is_success() {
                if let Ok(v) = r.json::<Value>().await {
                    if v["card"].is_string() {
                        return Some(v);
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    None
}

fn spawn_node(dir: &Path, p: &Ports) -> std::io::Result<u32> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe()?;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("node.log"))?;
    let mut cmd = std::process::Command::new(exe);
    // первая копия — «якорь»: может быть выходом в интернет для остальных (кто может — решает её владелец, по умолчанию доверенные)
    if dir.file_name().map(|n| n == "node1").unwrap_or(false) {
        cmd.arg("--anchor");
    }
    // `YANDI_TESTNET_CLIENT_NODES=1,3` — эти копии играют узлы за NAT (клиенты через ретрансляторы)
    let k = dir.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix("node")).unwrap_or("").to_string();
    if std::env::var("YANDI_TESTNET_CLIENT_NODES").map(|v| v.split(',').any(|x| x.trim() == k)).unwrap_or(false) {
        cmd.env("YANDI_CLIENT_ONLY", "1");
    }
    let child = cmd
        .current_dir(dir)
        .envs(node_env(dir, p))
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()?;
    std::fs::write(dir.join("pid"), child.id().to_string())?;
    Ok(child.id())
}

fn port_free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

async fn up(n: usize) -> Result<(), String> {
    let stand = stand_dir();
    std::fs::create_dir_all(&stand).map_err(|e| format!("папка {}: {e}", stand.display()))?;
    let (login, master) = passwords(&stand).map_err(|e| format!("пароли: {e}"))?;
    let c = http();
    println!("Тренировочная сеть: {} узла(ов), папка {}", n, stand.display());
    let mut cookies = Vec::new();
    for k in 1..=n {
        let dir = node_dir(&stand, k);
        let p = ports(k);
        std::fs::create_dir_all(dir.join("home")).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("config.yaml"), config_yaml(&p)).map_err(|e| e.to_string())?;
        if pid_alive_here(&dir).is_none() {
            if !port_free(p.web) {
                return Err(format!("порт {} занят другой программой — узел {k} не запущен", p.web));
            }
            let _ = std::fs::remove_file(dir.join("home/.yandi/node_card.txt"));
            spawn_node(&dir, &p).map_err(|e| format!("узел {k} не запустился: {e}"))?;
            println!("  узел {k}: запущен");
        } else {
            println!("  узел {k}: уже работает");
        }
        if !wait_web(&c, &p, 120).await {
            return Err(format!("узел {k} не открыл страницу за 2 минуты — см. {}", dir.join("node.log").display()));
        }
        let cookie = sign_in(&c, &dir, &p, &login, &master).await.map_err(|e| format!("узел {k}: {e}"))?;
        cookies.push(cookie);
    }
    let mut cards = Vec::new();
    for k in 1..=n {
        let v = wait_ready(&c, &ports(k), &cookies[k - 1], 180).await.ok_or(format!("узел {k} не закончил запуск за 3 минуты — см. {}", node_dir(&stand, k).join("node.log").display()))?;
        cards.push(v["card"].as_str().unwrap_or("").to_string());
    }
    for k in 1..=n {
        for j in 1..=n {
            if j == k {
                continue;
            }
            let r = c.post(format!("http://127.0.0.1:{}/api/peers/trusted", ports(k).web)).header("cookie", &cookies[k - 1])
                .json(&json!({"card": cards[j - 1], "name": format!("тренировочный узел {j}")})).send().await.map_err(|e| e.to_string())?;
            if !r.status().is_success() {
                return Err(format!("узел {k} не принял визитку узла {j}: {}", r.text().await.unwrap_or_default()));
            }
        }
    }
    // знакомство по сети занимает несколько секунд
    for _ in 0..60 {
        if all_online(&c, n, &cookies).await {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    print_status(&c, &stand, n, &cookies).await;
    Ok(())
}

async fn peers_of(c: &reqwest::Client, k: usize, cookie: &str) -> Option<Value> {
    c.get(format!("http://127.0.0.1:{}/api/peers/trusted", ports(k).web)).header("cookie", cookie).send().await.ok()?.json().await.ok()
}

async fn all_online(c: &reqwest::Client, n: usize, cookies: &[String]) -> bool {
    for k in 1..=n {
        let Some(v) = peers_of(c, k, &cookies[k - 1]).await else { return false };
        let list = v["peers"].as_array().cloned().unwrap_or_default();
        if list.len() < n - 1 || !list.iter().all(|p| p["online"] == true) {
            return false;
        }
    }
    true
}

async fn print_status(c: &reqwest::Client, stand: &Path, n: usize, cookies: &[String]) {
    println!();
    for k in 1..=n {
        let dir = node_dir(stand, k);
        let running = pid_alive_here(&dir).is_some();
        let seen = match (running, cookies.get(k - 1)) {
            (true, Some(ck)) => peers_of(c, k, ck).await.map(|v| {
                let list = v["peers"].as_array().cloned().unwrap_or_default();
                let on = list.iter().filter(|p| p["online"] == true).count();
                format!("видит в сети {on} из {} доверенных", list.len())
            }),
            _ => None,
        };
        println!("  узел {k}: {} — http://127.0.0.1:{}{}", if running { "работает" } else { "остановлен" }, ports(k).web, seen.map(|s| format!(" — {s}")).unwrap_or_default());
    }
    println!("\nВход на страницу любого узла — пароль из {}", stand.join("passwords.txt").display());
}

fn existing_nodes(stand: &Path) -> usize {
    (1..=MAX_NODES).take_while(|k| node_dir(stand, *k).exists()).count()
}

async fn status() -> Result<(), String> {
    let stand = stand_dir();
    let n = existing_nodes(&stand);
    if n == 0 {
        println!("Тренировочной сети нет. Поднять: yandi testnet up");
        return Ok(());
    }
    let c = http();
    let (login, master) = passwords(&stand).map_err(|e| e.to_string())?;
    let mut cookies = Vec::new();
    for k in 1..=n {
        let dir = node_dir(&stand, k);
        let ck = if pid_alive_here(&dir).is_some() { sign_in(&c, &dir, &ports(k), &login, &master).await.unwrap_or_default() } else { String::new() };
        cookies.push(ck);
    }
    print_status(&c, &stand, n, &cookies).await;
    Ok(())
}

async fn down() -> Result<(), String> {
    let stand = stand_dir();
    let n = existing_nodes(&stand);
    let mut stopped = 0;
    for k in 1..=n {
        let dir = node_dir(&stand, k);
        let Some(pid) = pid_alive_here(&dir) else { continue };
        // узел — глава своей группы процессов; сигнал останавливает его вместе с дочерними процессами
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        for _ in 0..40 {
            if pid_alive_here(&dir).is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        if pid_alive_here(&dir).is_some() {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        let _ = std::fs::remove_file(dir.join("pid"));
        stopped += 1;
        println!("  узел {k}: остановлен");
    }
    println!("Остановлено копий: {stopped}");
    Ok(())
}

async fn reset() -> Result<(), String> {
    down().await?;
    let stand = stand_dir();
    if stand.exists() {
        std::fs::remove_dir_all(&stand).map_err(|e| format!("не удалось стереть {}: {e}", stand.display()))?;
    }
    println!("Тренировочная сеть стёрта.");
    Ok(())
}

const HELP: &str = "Тренировочная сеть — несколько копий узла на этом компьютере:
  yandi testnet up [2–5]   поднять и познакомить узлы (по умолчанию 3)
  yandi testnet status     кто работает и кого видит
  yandi testnet down       остановить
  yandi testnet reset      остановить и стереть";

/// `yandi testnet …` — код выхода процесса.
pub async fn run_cli(args: Vec<String>) -> i32 {
    let r = match args.first().map(String::as_str) {
        Some("up") => match args.get(1).map(|s| s.parse::<usize>()) {
            None => up(DEFAULT_NODES).await,
            Some(Ok(n)) if (2..=MAX_NODES).contains(&n) => up(n).await,
            _ => Err(format!("число узлов — от 2 до {MAX_NODES}")),
        },
        Some("status") => status().await,
        Some("down") => down().await,
        Some("reset") => reset().await,
        _ => {
            println!("{HELP}");
            return 2;
        }
    };
    match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Ошибка: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_get_distinct_ports_that_never_touch_the_real_node() {
        let real = [9999u16, 8080, 9000, 10000, 9111, 9112, 9001, 9998, 18082, 18083, 8443, 9010];
        let mut all = std::collections::HashSet::new();
        for k in 1..=MAX_NODES {
            let p = ports(k);
            for x in [p.web, p.discovery, p.data, p.p2p_discovery, p.p2p_data, p.ws, p.http_proxy, p.mobile_gateway, p.mobile_p2p] {
                assert!(all.insert(x), "port {x} used twice");
                assert!(!real.contains(&x));
            }
        }
    }

    #[test]
    fn a_copy_lives_only_in_its_own_folder_and_on_loopback() {
        let dir = Path::new("/s/node2");
        let env: std::collections::HashMap<_, _> = node_env(dir, &ports(2)).into_iter().collect();
        assert_eq!(env["HOME"], "/s/node2/home");
        assert!(env["XDG_DATA_HOME"].starts_with("/s/node2/"));
        assert_eq!(env["YANDI_CONFIG"], "/s/node2/config.yaml");
        assert_eq!(env[ENV], "1");
        let y = config_yaml(&ports(2));
        assert!(y.contains("bind_address: 127.0.0.1") && y.contains("public_ip: 127.0.0.1") && y.contains("127.0.0.1:26206") && y.contains("web_ui: 26200"));
    }
}
