//! Тренировочная сеть (`yandi testnet`) целиком, на настоящих процессах узла: две копии поднимаются, сами проходят первую настройку,
//! знакомятся визитками и видят друг друга по настоящей зашифрованной связи; после остановки и повторного подъёма (вход, а не
//! первая настройка) — снова видят; остановка не оставляет процессов; `reset` стирает папку.
//! Домашняя папка владельца подменена временной — его данные не читаются и не трогаются.
//! `#[ignore]` (поднимает настоящие узлы, около минуты):
//!     cargo test --offline --test testnet_test -- --ignored
#![cfg(target_os = "linux")]
use std::path::Path;
use std::process::Command;

fn yandi(stand: &Path, home: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_yandi"))
        .arg("testnet")
        .args(args)
        .env("YANDI_TESTNET_DIR", stand)
        .env("HOME", home)
        .env_remove("XDG_DATA_HOME")
        .output()
        .unwrap();
    (out.status.code().unwrap_or(-1), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// Процессы, работающие в папке тренировочной сети (узлы).
fn processes_in(stand: &Path) -> Vec<String> {
    let mut found = vec![];
    for e in std::fs::read_dir("/proc").unwrap().flatten() {
        if let Ok(cwd) = std::fs::read_link(e.path().join("cwd")) {
            if cwd.starts_with(stand) {
                found.push(e.file_name().to_string_lossy().to_string());
            }
        }
    }
    found
}

#[test]
#[ignore = "starts real node processes"]
fn two_copies_meet_see_each_other_survive_a_restart_and_leave_nothing_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let stand = tmp.path().join("stand");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    let (code, out) = yandi(&stand, &home, &["up", "2"]);
    let guard = scopeguard(&stand, &home);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out.matches("видит в сети 1 из 1 доверенных").count(), 2, "{out}");
    let pw = std::fs::read_to_string(stand.join("passwords.txt")).unwrap();
    assert!(pw.contains("пароль входа:") && pw.contains("мастер-пароль:"));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(stand.join("passwords.txt")).unwrap().permissions().mode() & 0o777, 0o600);
    // каждая копия записала другую в доверенные — с адресом из визитки
    let trusted = std::fs::read_to_string(stand.join("node1/home/.yandi/trusted_peers.json")).unwrap();
    assert!(trusted.contains("127.0.0.1:26201"), "{trusted}");

    let (code, out) = yandi(&stand, &home, &["down"]);
    assert_eq!(code, 0, "{out}");
    assert!(processes_in(&stand).is_empty(), "nothing is left running: {:?}", processes_in(&stand));

    // второй подъём: вход по сохранённому паролю, доверие сохранилось
    let (code, out) = yandi(&stand, &home, &["up", "2"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out.matches("видит в сети 1 из 1 доверенных").count(), 2, "{out}");
    let (code, out) = yandi(&stand, &home, &["status"]);
    assert_eq!((code, out.matches("работает").count()), (0, 2), "{out}");

    drop(guard);
    assert!(processes_in(&stand).is_empty());
    let (code, out) = yandi(&stand, &home, &["reset"]);
    assert_eq!(code, 0, "{out}");
    assert!(!stand.exists());
}

/// Остановить копии, даже если проверка упала посередине.
struct Down(std::path::PathBuf, std::path::PathBuf);
impl Drop for Down {
    fn drop(&mut self) {
        let _ = yandi(&self.0, &self.1, &["down"]);
    }
}
fn scopeguard(stand: &Path, home: &Path) -> Down {
    Down(stand.to_path_buf(), home.to_path_buf())
}
