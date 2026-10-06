//! Сторож инвариантов: документ `docs/INVARIANTS.md` не должен расходиться с тестами.
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Ссылки вида `путь::имя` из таблицы инвариантов.
fn references(row: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    for part in row.split('`').skip(1).step_by(2) {
        if let Some((file, name)) = part.split_once("::") {
            if file.ends_with(".rs") {
                out.push((file.to_string(), name.to_string()));
            }
        }
    }
    out
}

#[test]
fn every_invariant_points_at_a_real_test_or_says_it_is_pending() {
    let doc = std::fs::read_to_string(root().join("docs/INVARIANTS.md")).unwrap();
    let mut rows = 0;
    let mut problems: Vec<String> = vec![];
    for line in doc.lines().filter(|l| l.starts_with("| I")) {
        rows += 1;
        let id = line.split('|').nth(1).unwrap_or("").trim().to_string();
        let status = line.split('|').nth(3).unwrap_or("").trim().to_lowercase();
        let refs = references(line);
        if status.starts_with("ждёт") {
            if !(status.contains("фаз")) {
                problems.push(format!("{id}: «ждёт» без указания фазы плана"));
            }
            if !refs.is_empty() {
                // ссылки у ожидающих необязательны, но если есть — должны быть настоящими
            }
        } else if refs.is_empty() {
            problems.push(format!("{id}: нет ни одной ссылки на тест и не помечен «ждёт»"));
        }
        for (file, name) in refs {
            let path = root().join(&file);
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    if !text.contains(&format!("fn {name}")) {
                        problems.push(format!("{id}: в {file} нет теста «{name}»"));
                    }
                }
                Err(_) => problems.push(format!("{id}: файла {file} нет")),
            }
        }
    }
    assert!(rows >= 10, "таблица инвариантов не найдена или пуста ({rows} строк)");
    assert!(problems.is_empty(), "\nдокумент инвариантов разошёлся с тестами:\n  {}\n", problems.join("\n  "));
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

#[test]
fn the_reference_parser_finds_references() {
    let r = references("| I1 | текст | да | `tests/a.rs::one`, `src/b.rs::two` и просто `слово` |");
    assert_eq!(r, vec![("tests/a.rs".to_string(), "one".to_string()), ("src/b.rs".to_string(), "two".to_string())]);
}

/// `try_lock().unwrap()` паникует, если блокировку в этот момент держит другая задача (настоящая гонка, найдена враждебным собеседником).
/// Блокировку берут с ожиданием, а `try_lock` обрабатывают, а не разворачивают через `unwrap`.
#[test]
fn no_try_lock_unwrap_in_production_code() {
    let mut files = vec![];
    walk(&root().join("src"), &mut files);
    let mut bad = vec![];
    for f in files.into_iter().filter(|f| f.extension().and_then(|e| e.to_str()) == Some("rs")) {
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        // тестовая часть файла (после #[cfg(test)]) не проверяется
        let body = text.split("#[cfg(test)]").next().unwrap_or(&text);
        for (i, line) in body.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            for pat in ["try_lock().unwrap()", "try_read().unwrap()", "try_write().unwrap()", "try_lock().expect(", "try_read().expect(", "try_write().expect("] {
                if code.contains(pat) {
                    bad.push(format!("{}:{} — {}", f.strip_prefix(root()).unwrap().display(), i + 1, pat));
                }
            }
        }
    }
    assert!(bad.is_empty(), "\nпаника при занятой блокировке:\n  {}\n", bad.join("\n  "));
}

/// I11 (сторож): код выхода не соединяется с целью напрямую — только через `exit_policy::connect_public`
/// (иначе чужой узел достанет до этого компьютера и домашней сети) и спрашивает `may_exit`.
#[test]
fn exit_handlers_use_exit_policy_only() {
    let files = [
        "src/protocol/tcp_tunnel_exit.rs",
        "src/protocol/tcp_tunnel_exit_v2.rs",
        "src/socks5/exit_node.rs",
        "src/proxy/gateway.rs",
    ];
    for f in files {
        let s = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("{f}: {e}"));
        let code: String = s.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        assert!(!code.contains("TcpStream::connect"), "{f}: прямое соединение мимо политики выхода");
        assert!(code.contains("connect_public"), "{f}: нет соединения через политику выхода");
        assert!(code.contains("may_exit"), "{f}: не спрашивает, кому можно выходить");
    }
}
