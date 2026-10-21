//! Консольная настройка и управление (`yandi setup`, `yandi admin`) на настоящем процессе узла с чистой домашней папкой.
//!   * свежий узел принимает первую настройку из консоли (пароль со стандартного ввода, фраза в новый файл 0600);
//!   * повторная настройка отказывается; без терминала и без --seed-file фраза не печатается;
//!   * админские команды просят пароль администратора; неверный пароль отказывает;
//!   * стирание закрытой среды требует слова подтверждения и действует только с ним.
//! `#[ignore]` (около минуты):  cargo test --offline --test admin_cli_test -- --ignored
#![cfg(target_os = "linux")]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Узлы тестов делят порты: по одному за раз.
static ONE_NODE: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Node {
    child: Child,
    dir: PathBuf,
}
impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn start(dir: &Path) -> Node {
    let home = dir.join("home");
    std::fs::create_dir_all(home.join(".local/share")).unwrap();
    std::fs::create_dir_all(home.join(".config")).unwrap();
    std::fs::set_permissions(&home, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    std::fs::write(
        dir.join("config.yaml"),
        "server: {bind_address: 127.0.0.1, log_level: info}\nports: {discovery: 31101, data: 31102, mobile_gateway: 31108, mobile_p2p: 31109, http_proxy: 31107, web_ui: 31100}\nnetwork: {public_ip: 127.0.0.1}\nws: {bind: \"127.0.0.1:31106\"}\n",
    )
    .unwrap();
    let log = std::fs::File::create(dir.join("node.log")).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_yandi"))
        .current_dir(dir)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", &home)
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("YANDI_CONFIG", dir.join("config.yaml"))
        .env("YANDI_P2P_DISCOVERY_PORT", "31103")
        .env("YANDI_P2P_DATA_PORT", "31104")
        .env("YANDI_CORE_BIN", "self")
        .env("YANDI_TESTNET", "1")
        .env("NO_PROXY", "127.0.0.1")
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    Node { child, dir: dir.to_path_buf() }
}

fn run(args: &[&str], stdin: &str, sock: &Path) -> (i32, String) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_yandi"))
        .args(args)
        .arg("--socket")
        .arg(sock)
        // сообщения проверяются по-русски; язык не зависит от окружения (на CI LANG не задан — был бы английский)
        .env("YANDI_LANG", "ru")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = c.wait_with_output().unwrap();
    (out.status.code().unwrap_or(-1), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[test]
#[ignore = "starts a real node process"]
fn first_setup_and_admin_work_from_the_console() {
    let _one = ONE_NODE.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let node = start(tmp.path());
    let sock = node.dir.join("home/.local/share/yandi/run/yandi-ui.sock");
    let t = Instant::now();
    while !sock.exists() {
        assert!(t.elapsed() < Duration::from_secs(60), "узел не поднял сокет");
        std::thread::sleep(Duration::from_millis(300));
    }
    // без терминала и без файла фраза не показывается
    let (rc, out) = run(&["setup", "--password-stdin"], "testpass123\n", &sock);
    assert_ne!(rc, 0);
    assert!(out.contains("нет терминала"), "{out}");
    // тот же отказ по-английски: язык задаётся ключом и сильнее переменной окружения
    let (rc, out) = run(&["setup", "--lang", "en", "--password-stdin"], "testpass123\n", &sock);
    assert_ne!(rc, 0);
    assert!(out.contains("no terminal") && !out.contains("нет терминала"), "{out}");
    // настройка с файлом фразы
    let seed = node.dir.join("seed.txt");
    let seed_s = seed.to_str().unwrap();
    let (rc, out) = run(&["setup", "--password-stdin", "--seed-file", seed_s], "testpass123\n", &sock);
    assert_eq!(rc, 0, "{out}");
    let phrase = std::fs::read_to_string(&seed).unwrap();
    assert_eq!(phrase.split_whitespace().count(), 24, "по умолчанию 24 слова");
    assert!(!out.contains(phrase.trim()), "фраза попала в вывод");
    assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&seed).unwrap().permissions()) & 0o777, 0o600);
    // повторная настройка отказывает
    let (rc, out) = run(&["setup", "--password-stdin", "--seed-file", node.dir.join("seed2.txt").to_str().unwrap()], "x\n", &sock);
    assert_ne!(rc, 0);
    assert!(out.contains("уже настроен"), "{out}");
    // чтение без админ-сеанса
    let (rc, out) = run(&["admin", "vault", "status"], "", &sock);
    assert_eq!(rc, 0, "{out}");
    assert!(out.contains("\"configured\": true"), "{out}");
    // команды, которые меняют состояние или читают разные части узла: ни одна не должна отвечать пустым успехом (строгий разбор ответа)
    for (args, stdin) in [
        (vec!["admin", "alerts"], ""),
        (vec!["admin", "update"], ""),
        (vec!["admin", "status"], ""),
        (vec!["admin", "proxies", "list", "--admin-password-stdin"], "testpass123\n"),
        (vec!["admin", "proxies", "settings", "--enabled", "on", "--strict", "off", "--max-clients", "7", "--admin-password-stdin"], "testpass123\n"),
        (vec!["admin", "vault", "lock"], ""),
        (vec!["admin", "vault", "unlock", "--admin-password-stdin"], "testpass123\n"),
        (vec!["admin", "vault", "status"], ""),
    ] {
        let (rc, out) = run(&args, stdin, &sock);
        assert_eq!(rc, 0, "{args:?}: {out}");
    }
    // админская команда: неверный пароль отказывает, верный пускает
    let (rc, out) = run(&["admin", "devices", "--admin-password-stdin"], "wrongwrong\n", &sock);
    assert_ne!(rc, 0);
    assert!(out.contains("неверный пароль"), "{out}");
    let (rc, out) = run(&["admin", "devices", "--admin-password-stdin"], "testpass123\n", &sock);
    assert_eq!(rc, 0, "{out}");
    // стирание: без слова подтверждения ничего не стёрто, со словом — стёрто
    let (rc, _) = run(&["admin", "vault", "panic-erase", "--admin-password-stdin"], "testpass123\nнет\n", &sock);
    assert_ne!(rc, 0);
    let (_, out) = run(&["admin", "vault", "status"], "", &sock);
    assert!(out.contains("\"configured\": true"), "{out}");
    let (rc, out) = run(&["admin", "vault", "panic-erase", "--admin-password-stdin"], "testpass123\nСТЕРЕТЬ\n", &sock);
    assert_eq!(rc, 0, "{out}");
    let (_, out) = run(&["admin", "vault", "status"], "", &sock);
    assert!(out.contains("\"configured\": false"), "{out}");
}

fn wait_sock(node: &Node) -> PathBuf {
    let sock = node.dir.join("home/.local/share/yandi/run/yandi-ui.sock");
    let t = Instant::now();
    while !sock.exists() {
        assert!(t.elapsed() < Duration::from_secs(60), "узел не поднял сокет");
        std::thread::sleep(Duration::from_millis(300));
    }
    sock
}

/// Номер узла из ответа состояния (`yandi admin status`).
fn node_id_of(sock: &Path) -> String {
    let (rc, out) = run(&["admin", "raw", "GET", "/api/status"], "", sock);
    assert_eq!(rc, 0, "{out}");
    let json = out.split_once('\n').map(|x| x.1).unwrap_or(&out);
    let v: serde_json::Value = serde_json::from_str(json.trim()).unwrap_or_else(|_| panic!("не JSON: {out}"));
    v["node_id"].as_str().or_else(|| v["id"].as_str()).unwrap_or_else(|| panic!("нет номера узла: {out}")).to_string()
}

fn signer_of(sock: &Path) -> String {
    let (rc, out) = run(&["admin", "bootstrap", "pubkey", "--admin-password-stdin"], "testpass123\n", sock);
    assert_eq!(rc, 0, "{out}");
    out.split_whitespace().find(|w| w.len() == 64 && w.chars().all(|c| c.is_ascii_hexdigit())).unwrap_or_else(|| panic!("нет ключа: {out}")).to_string()
}

#[test]
#[ignore = "starts a real node process"]
fn the_seed_phrase_alone_brings_back_the_same_node_and_the_same_list_signing_key() {
    let _one = ONE_NODE.lock().unwrap_or_else(|e| e.into_inner());
    let word = "Воробей-2026";
    // узел A: новая настройка с контрольным словом
    let tmp_a = tempfile::tempdir().unwrap();
    let a = start(tmp_a.path());
    let sock = wait_sock(&a);
    let seed = a.dir.join("seed.txt");
    let (rc, out) = run(&["setup", "--password-stdin", "--control-word-stdin", "--no-backup-dir", "--words", "18", "--seed-file", seed.to_str().unwrap()], &format!("testpass123\n{word}\n"), &sock);
    assert_eq!(rc, 0, "{out}");
    let phrase = std::fs::read_to_string(&seed).unwrap().trim().to_string();
    assert_eq!(phrase.split_whitespace().count(), 18);
    assert!(!out.contains(&phrase) && !out.contains(word), "ни фраза, ни слово не попали в вывод");
    let id_a = node_id_of(&sock);
    let signer_a = signer_of(&sock);
    // номер узла из --check совпадает с настоящим (узел для этого не нужен)
    let (rc, check) = run(&["setup", "--check", "--password-stdin", "--control-word-stdin"], &format!("{phrase}\n{word}\n"), &sock);
    assert_eq!(rc, 0, "{check}");
    let spaced = check.lines().find(|l| l.contains("Номер узла")).unwrap().split(':').nth(1).unwrap().trim().replace(' ', "");
    assert!(id_a.starts_with(&spaced), "{id_a} vs {spaced}");
    drop(a);

    // узел B: чистая машина, только сид и слово
    let tmp_b = tempfile::tempdir().unwrap();
    let b = start(tmp_b.path());
    let sock_b = wait_sock(&b);
    // без слова получается ДРУГОЙ узел: --check это показывает, ничего не записывая
    let (rc, no_word) = run(&["setup", "--check", "--password-stdin"], &format!("{phrase}\n"), &sock_b);
    assert_eq!(rc, 0, "{no_word}");
    assert!(!no_word.replace(' ', "").contains(&id_a[..16]), "без слова тот же узел получиться не должен");
    let (rc, out) = run(&["setup", "--restore", "--yes", "--no-backup-dir", "--password-stdin", "--control-word-stdin"], &format!("{phrase}\n{word}\nnewpass456\n"), &sock_b);
    assert_eq!(rc, 0, "{out}");
    assert!(out.contains("Личность узла"), "{out}");
    assert_eq!(node_id_of(&sock_b), id_a, "тот же номер узла");
    let (rc, out) = run(&["admin", "bootstrap", "pubkey", "--admin-password-stdin"], "newpass456\n", &sock_b);
    assert_eq!(rc, 0, "{out}");
    assert!(out.contains(&signer_a), "тот же ключ подписи списка узлов: {out} / {signer_a}");
}
