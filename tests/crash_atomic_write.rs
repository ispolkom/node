//! Устойчивость к падению: файл состояния, записываемый узлом (`write_private`, `sealed::write`), после убийства процесса `kill -9`
//! в любой момент либо старый целиком, либо новый целиком, но никогда не обрезанный и не смешанный.

#![cfg(unix)] // проверка падения опирается на kill и атомарное переименование POSIX

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

fn payload(n: u64) -> Vec<u8> {
    // длинное содержимое (несколько страниц): обрыв посреди записи был бы заметен
    let line = format!("{{\"n\":{n},\"pad\":\"{}\"}}", "x".repeat(60_000));
    line.into_bytes()
}

fn valid(bytes: &[u8]) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let n = v["n"].as_u64()?;
    if bytes == payload(n).as_slice() { Some(n) } else { None }
}

/// Дочерний режим: бесконечно перезаписывает файл по пути из окружения.
#[test]
fn child_writer() {
    let Some(path) = std::env::var_os("YANDI_CRASH_CHILD") else { return };
    let path = PathBuf::from(path);
    let mut n = 0u64;
    loop {
        n += 1;
        yandi::util::private_file::write_private(&path, &payload(n)).unwrap();
    }
}

#[test]
fn file_is_never_torn_when_the_writer_is_killed() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("state.json");
    // исходное состояние
    yandi::util::private_file::write_private(&file, &payload(0)).unwrap();
    let exe = std::env::current_exe().unwrap();
    let mut seen_new = false;
    for round in 0..40u64 {
        let mut child = Command::new(&exe)
            .args(["--exact", "child_writer", "--nocapture", "--test-threads=1"])
            .env("YANDI_CRASH_CHILD", &file)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // случайная задержка: убиваем в разные моменты записи
        std::thread::sleep(Duration::from_millis(30 + (round * 37) % 160));
        child.kill().unwrap(); // SIGKILL
        child.wait().unwrap();
        let bytes = std::fs::read(&file).expect("файл состояния обязан существовать");
        let n = valid(&bytes).unwrap_or_else(|| panic!("раунд {round}: файл повреждён ({} байт)", bytes.len()));
        if n > 0 {
            seen_new = true;
        }
        // не должно оставаться мусора рядом (временные файлы подбираются при следующей записи)
        let leftovers = std::fs::read_dir(dir.path()).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().contains(".tmp")).count();
        assert!(leftovers <= 1, "раунд {round}: осталось временных файлов: {leftovers}");
    }
    assert!(seen_new, "дочерний писатель должен был успеть записать хотя бы раз");
}

#[test]
fn sealed_file_survives_kill_and_still_opens() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sealed.enc");
    let key = [9u8; 32];
    yandi::util::sealed::write(&file, &key, "crash-test", &payload(0)).unwrap();
    for round in 0..15u64 {
        // запись в этом же процессе, но «обрыв» эмулируем частичным временным файлом рядом
        std::fs::write(dir.path().join(format!(".sealed.enc.tmp{round}-0")), b"partial").unwrap();
        yandi::util::sealed::write(&file, &key, "crash-test", &payload(round + 1)).unwrap();
        let blob = std::fs::read(&file).unwrap();
        let plain = yandi::util::sealed::open(&key, "crash-test", &blob).expect("после записи файл открывается");
        assert_eq!(valid(&plain), Some(round + 1));
    }
}
