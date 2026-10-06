//! Тест-сторож: в журнал не попадает ключевой материал.
//!
//! Читает исходники, находит каждый вызов печати/журналирования (`println!`, `eprintln!`, `print!`, `eprint!`, `info!`, `debug!`, `warn!`,
//! `error!`, `trace!`) и смотрит на его АРГУМЕНТЫ (не на строку формата). Если в аргументах есть имя из списка секретов — тест падает.
//! Печать булевых признаков («есть ли ключ») разрешена: `x.is_some()` не раскрывает значение.
use std::path::Path;

const MACROS: &[&str] = &["println!", "eprintln!", "print!", "eprint!", "info!", "debug!", "warn!", "error!", "trace!"];

/// Имена, которых не должно быть в аргументах журнала.
const SECRET_NAMES: &[&str] = &[
    "shared_secret", "session_key", "key_hash", "resume_secret", "secret_bytes", "private_key", "signing_private_key", "master_key",
    "master_password", "password", "passphrase", "seed", "key_bytes", "hop_key", "layer_key", "ikm", "shared.as_bytes", "secret.as_bytes",
    // печать значения (hex) от того, что называется ключом или секретом
    "encode(&key", "encode(key", "encode(&self.key", "encode(&self.secret", "encode(&shared", "encode(&secret", "encode(&seed", "encode(&hashed",
    "encode(&session_", "encode(&resume", "encode(&priv", ".key.key",
];

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

/// Аргументы вызова (после строки формата) от позиции открывающей скобки до парной закрывающей; строки пропускаются.
fn call_args(s: &str, open: usize) -> Option<(String, usize)> {
    let b = s.as_bytes();
    let (mut depth, mut k, mut in_str, mut esc) = (0i32, open, false, false);
    let mut args = String::new();
    let mut first_string_done = false;
    while k < b.len() {
        let c = b[k] as char;
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
                first_string_done = true;
            }
        } else {
            match c {
                '"' => in_str = true,
                '(' => {
                    depth += 1;
                    if first_string_done && depth > 1 {
                        args.push(c);
                    }
                }
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some((args, k));
                    }
                    if first_string_done {
                        args.push(c);
                    }
                }
                _ => {
                    if first_string_done {
                        args.push(c);
                    }
                }
            }
        }
        k += 1;
    }
    None
}

/// Подвыражения, которые безопасны: булевы признаки и длины.
fn sanitize(args: &str) -> String {
    let mut a = args.to_string();
    for safe in [".is_some()", ".is_none()", ".len()", ".is_empty()", ".is_ok()", ".is_err()"] {
        a = a.replace(safe, "");
    }
    a
}

#[test]
fn logs_never_contain_key_material() {
    let mut files = vec![];
    walk(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").as_path(), &mut files);
    let mut bad: Vec<String> = vec![];
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        for m in MACROS {
            let mut from = 0;
            while let Some(pos) = text[from..].find(m) {
                let at = from + pos;
                from = at + m.len();
                // это вызов макроса, а не часть другого слова и не комментарий
                let line_start = text[..at].rfind('\n').map(|x| x + 1).unwrap_or(0);
                let before = &text[line_start..at];
                if before.contains("//") || before.chars().last().map(|c| c.is_alphanumeric() || c == '_').unwrap_or(false) {
                    continue;
                }
                if text.as_bytes().get(at + m.len()) != Some(&b'(') {
                    continue;
                }
                if let Some((args, _)) = call_args(&text, at + m.len()) {
                    let a = sanitize(&args);
                    for name in SECRET_NAMES {
                        if a.contains(name) {
                            let line = text[..at].matches('\n').count() + 1;
                            bad.push(format!("{}:{} — в аргументах журнала «{}»", f.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap().display(), line, name));
                            break;
                        }
                    }
                }
            }
        }
    }
    assert!(bad.is_empty(), "\nключевой материал в журнале ({}):\n  {}\n", bad.len(), bad.join("\n  "));
}

/// Проверка самого сторожа: он обязан ловить заведомо плохие примеры.
#[test]
fn the_guard_catches_a_leak() {
    let bad = "fn f() { println!(\"key: {}\", hex::encode(&shared_secret.as_bytes()[..8])); }";
    let at = bad.find("println!").unwrap();
    let (args, _) = call_args(bad, at + "println!".len()).unwrap();
    assert!(SECRET_NAMES.iter().any(|n| sanitize(&args).contains(n)));
    let ok = "fn f() { println!(\"есть ключ: {}\", sk.is_some()); }";
    let at = ok.find("println!").unwrap();
    let (args, _) = call_args(ok, at + "println!".len()).unwrap();
    assert!(!SECRET_NAMES.iter().any(|n| sanitize(&args).contains(n)));
}
