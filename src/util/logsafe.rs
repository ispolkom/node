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

    /// Защита от возврата: строки журнала с адресом собеседника или клиента, куда пользователь заходил, обязаны идти через `logsafe::a`.
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
        let macros = ["println!(", "eprintln!(", "info!(", "warn!(", "error!(", "debug!(", "trace!("];
        let words = ["peer_addr", "client_addr", "remote_addr", "remote_ip", "target_addr", "addr_str", "addr_to_use", "addr_clone", "peer_addr_for_pump"];
        let mut bad = Vec::new();
        for f in files {
            // команды оператора в терминале (cli.rs) и тесты выводят адреса осознанно
            let name = f.to_string_lossy().to_string();
            if name.ends_with("netlayer/cli.rs") || name.ends_with("util/logsafe.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&f).unwrap();
            for (i, line) in text.lines().enumerate() {
                if !macros.iter().any(|m| line.contains(m)) || line.contains("logsafe") {
                    continue;
                }
                let has_word = |w: &str| {
                    line.match_indices(w).any(|(s, _)| {
                        let before = line[..s].chars().next_back();
                        let after = line[s + w.len()..].chars().next();
                        !before.map_or(false, |c| c.is_alphanumeric() || c == '_') && !after.map_or(false, |c| c.is_alphanumeric() || c == '_')
                    })
                };
                if words.iter().any(|w| has_word(w)) {
                    bad.push(format!("{}:{}: {}", name, i + 1, line.trim()));
                }
            }
        }
        assert!(bad.is_empty(), "адрес печатается в журнал без logsafe::a:\n{}", bad.join("\n"));
    }
}
