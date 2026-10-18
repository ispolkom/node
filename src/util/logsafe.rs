//! Адреса в журналах. По умолчанию адрес собеседника или клиента в журнал не попадает (приватность: узел не копит, кто откуда подключался);
//! для отладки владелец узла может включить показ переменной окружения `YANDI_LOG_ADDRS=1`.
use std::sync::OnceLock;

fn verbose() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var("YANDI_LOG_ADDRS").map(|v| v == "1").unwrap_or(false))
}

/// Что печатать вместо адреса.
pub fn a<T: std::fmt::Display + ?Sized>(x: &T) -> String {
    if verbose() {
        x.to_string()
    } else {
        "адрес скрыт".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_hidden_unless_the_owner_asked_for_it() {
        // переменная окружения в этом процессе не задана: адрес не печатается
        assert_eq!(a(&"203.0.113.7:9000"), "адрес скрыт");
        assert_eq!(a(&std::net::SocketAddr::from(([203, 0, 113, 7], 9000))), "адрес скрыт");
    }

    /// Текст вызова макроса журнала целиком (в том числе на нескольких строках): от имени макроса до парной скобки.
    fn macro_calls(text: &str) -> Vec<(usize, String)> {
        let macros = ["println!(", "eprintln!(", "info!(", "warn!(", "error!(", "debug!(", "trace!("];
        let bytes: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            let rest: String = bytes[i..bytes.len().min(i + 12)].iter().collect();
            if let Some(m) = macros.iter().find(|m| rest.starts_with(*m)) {
                let before = if i > 0 { bytes[i - 1] } else { ' ' };
                if !(before.is_alphanumeric() || before == '_') {
                    let start = i + m.len();
                    let (mut depth, mut j, mut in_str) = (1, start, false);
                    while j < bytes.len() && depth > 0 {
                        let c = bytes[j];
                        if in_str {
                            if c == '\\' { j += 1; } else if c == '"' { in_str = false; }
                        } else if c == '"' {
                            in_str = true;
                        } else if c == '(' {
                            depth += 1;
                        } else if c == ')' {
                            depth -= 1;
                        }
                        j += 1;
                    }
                    let line = bytes[..i].iter().filter(|c| **c == '\n').count() + 1;
                    out.push((line, bytes[start..j.saturating_sub(1).max(start)].iter().collect()));
                    i = j;
                    continue;
                }
            }
            i += 1;
        }
        out
    }

    /// Защита от возврата: вызовы журнала, где печатается адрес собеседника или клиента либо адрес сайта, куда ходили через узел, обязаны
    /// заворачивать его в `logsafe::a`. Проверяется ВЕСЬ вызов (в том числе перенесённые на другие строки аргументы и `{имя}` в строке формата);
    /// у файлов с короткими именами переменных (`to`, `peer`, `a`) — свой список слов.
    #[test]
    fn log_lines_do_not_print_peer_client_or_destination_addresses_raw() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().map_or(false, |x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        let global = ["peer_addr", "client_addr", "remote_addr", "remote_ip", "target_addr", "addr_str", "addr_to_use", "addr_clone", "peer_addr_for_pump", "target_url", "request_line", "target_host", "src_addr", "from_addr"];
        let per_file: [(&str, &[&str]); 4] = [
            ("netlayer/ws_transport.rs", &["peer", "addr"]),
            ("netlayer/tcp_carrier.rs", &["to", "peer", "remote"]),
            ("web/peers.rs", &["a", "addr"]),
            ("proxy/gateway.rs", &["url"]),
        ];
        let whole = |hay: &str, w: &str| {
            hay.match_indices(w).any(|(s, _)| {
                let before = hay[..s].chars().next_back();
                let after = hay[s + w.len()..].chars().next();
                !before.map_or(false, |c| c.is_alphanumeric() || c == '_') && !after.map_or(false, |c| c.is_alphanumeric() || c == '_')
            })
        };
        let mut bad = Vec::new();
        for f in files {
            // на Windows путь с «\\»: сравнивать нужно по единому виду
            let name = f.to_string_lossy().replace('\\', "/");
            // команды оператора в терминале (cli.rs) выводят адреса осознанно
            if name.ends_with("netlayer/cli.rs") || name.ends_with("util/logsafe.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&f).unwrap();
            let mut words: Vec<&str> = global.to_vec();
            for (suffix, extra) in per_file {
                if name.ends_with(suffix) {
                    words.extend_from_slice(extra);
                }
            }
            for (line, call) in macro_calls(&text) {
                // убираем безопасные места: logsafe::a(&слово) и logsafe::a(слово)
                let mut cleaned = call.clone();
                while let Some(p) = cleaned.find("logsafe::a(") {
                    let end = cleaned[p..].find(')').map(|e| p + e + 1).unwrap_or(cleaned.len());
                    cleaned.replace_range(p..end, "");
                }
                // слова внутри строковых литералов (не в `{слово}`) — обычный текст сообщения, не переменная
                let mut code = String::new();
                let mut in_str = false;
                let mut it = cleaned.chars().peekable();
                while let Some(c) = it.next() {
                    if in_str {
                        if c == '\\' { it.next(); } else if c == '"' { in_str = false; code.push(' '); } else if c == '{' {
                            let mut name = String::new();
                            while let Some(&n) = it.peek() { if n == '}' || n == ':' { break; } name.push(n); it.next(); }
                            code.push(' ');
                            code.push_str(&name);
                            code.push(' ');
                        }
                    } else if c == '"' { in_str = true; } else { code.push(c); }
                }
                if words.iter().any(|w| whole(&code, w)) {
                    bad.push(format!("{}:{}: {}", name, line, call.trim().replace('\n', " ")));
                }
            }
        }
        assert!(bad.is_empty(), "адрес печатается в журнал без logsafe::a:\n{}", bad.join("\n"));
    }
}
