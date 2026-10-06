//! Тест-сторож: секретные файлы не пишутся приёмом «записали обычным способом, потом закрыли права».
//!
//! Между этими двумя шагами файл виден другим пользователям, а ошибка `chmod`, проглоченная через `let _ =`, оставляет его открытым навсегда.
//! Секреты пишет только `util::private_file::write_private` (создаёт сразу с 0600).
use std::path::Path;

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
            out.push(p);
        }
    }
}

/// Номера строк, где права 0600 ставятся уже после обычной записи (в пределах 25 строк выше).
fn write_then_chmod(text: &str) -> Vec<usize> {
    let lines: Vec<&str> = text.lines().collect();
    let mut hits = vec![];
    for (i, l) in lines.iter().enumerate() {
        let sets_private = (l.contains("set_permissions(") || l.contains("set_mode(")) && (l.contains("0o600") || l.contains("0o400"));
        let sets_mode_var = l.contains("set_mode(0o600)") || l.contains("from_mode(0o600)");
        if !(sets_private || sets_mode_var) {
            continue;
        }
        let from = i.saturating_sub(25);
        let before = lines[from..i].join("\n");
        if before.contains("fs::write(") || before.contains("File::create(") {
            hits.push(i + 1);
        }
    }
    hits
}

#[test]
fn secrets_are_never_written_loose_and_chmodded_later() {
    let mut files = vec![];
    walk(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").as_path(), &mut files);
    let mut bad = vec![];
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        // тесты внутри файлов проверяют права и могут писать как угодно
        let body = text.split("#[cfg(test)]").next().unwrap_or(&text);
        for line in write_then_chmod(body) {
            bad.push(format!("{}:{}", f.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap().display(), line));
        }
    }
    assert!(bad.is_empty(), "\nсекрет пишется обычной записью, права ставятся потом — используйте util::private_file::write_private:\n  {}\n", bad.join("\n  "));
}

#[test]
fn the_guard_recognises_the_old_pattern() {
    let old = "fn save() {\n    std::fs::write(&p, data)?;\n    #[cfg(unix)]\n    {\n        std::fs::set_permissions(&p, Permissions::from_mode(0o600))?;\n    }\n}";
    assert_eq!(write_then_chmod(old), vec![5]);
    let fine = "fn save() {\n    crate::util::private_file::write_private(&p, data)?;\n}";
    assert!(write_then_chmod(fine).is_empty());
}
