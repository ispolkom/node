//! `yandi bootstrap …` — инструменты владельца списка входных узлов (`bootstrap/bootstrap.json`).
//!
//! ```text
//! подпись списка: `yandi admin bootstrap sign <файл>` (ключ — только в закрытой среде узла, файла с ключом нет; открытый: `yandi admin bootstrap pubkey`)
//! yandi bootstrap show   [файл]                             прочитать и показать список
//! yandi bootstrap verify [файл]                             проверить подпись закреплённым ключом проекта
//! yandi bootstrap add    [файл] [--id H] --key H --addr A [--addr A] [--region NL] [--roles entry,relay,exit]
//! yandi bootstrap from-card [файл] <визитка>                добавить узел по его визитке (`~/.yandi/node_card.txt` или страница узла)
//! yandi bootstrap remove [файл] --id H                      убрать узел
//! (подписанный список после правки — подписать заново: любая правка сбрасывает подпись)
//! ```
//! Файл по умолчанию — `bootstrap/bootstrap.json`. После `add`/`remove`/`from-card` подпись недействительна, пока не выполнен `sign`.
use super::*;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn all_args(args: &[String], name: &str) -> Vec<String> {
    args.iter().enumerate().filter(|(_, a)| *a == name).filter_map(|(i, _)| args.get(i + 1).cloned()).collect()
}

/// Первый аргумент без «--», не являющийся значением флага.
fn positional(args: &[String]) -> Vec<String> {
    let mut out = vec![];
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
        } else if a.starts_with("--") {
            skip = true;
        } else {
            out.push(a.clone());
        }
    }
    out
}

const DEFAULT_FILE: &str = "bootstrap/bootstrap.json";

fn load(path: &str) -> Result<Doc, String> {
    match std::fs::read(path) {
        Ok(b) => parse(&b).map_err(|e| format!("{path}: {e}")),
        Err(_) => Ok(Doc { format: 1, network: "yandi".into(), issued: 0, expires: 0, sequence: 0, entries: vec![], min_node: None, min_app: None, signer: String::new(), signature: String::new() }),
    }
}

fn save(path: &str, d: &Doc) -> Result<(), String> {
    if let Some(dir) = Path::new(path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, serde_json::to_string_pretty(d).map_err(|e| e.to_string())? + "\n").map_err(|e| format!("{path}: {e}"))
}

/// Вернуть код выхода и текст для печати.
pub fn run(args: Vec<String>) -> (i32, String) {
    let Some(cmd) = args.first().map(|s| s.as_str()) else { return (2, usage()) };
    let rest = &args[1..];
    let pos = positional(rest);
    let file = |i: usize| pos.get(i).cloned().unwrap_or_else(|| DEFAULT_FILE.to_string());
    let result: Result<String, String> = (|| match cmd {
        // ключ подписи списка НЕ хранится в файле: он живёт только в закрытой среде узла (`yandi admin bootstrap pubkey|sign`)
        "keygen" | "sign" => Err("подпись списка делает узел из закрытой среды: yandi admin bootstrap sign ФАЙЛ (открытый ключ: yandi admin bootstrap pubkey); файла с ключом нет".to_string()),
        "show" => {
            let d = load(&file(0))?;
            Ok(serde_json::to_string_pretty(&d).map_err(|e| e.to_string())?)
        }
        "verify" => {
            let f = file(0);
            let d = load(&f)?;
            d.verify(SIGNER_KEYS, now(), None).map_err(|e| format!("{f}: {e}"))?;
            Ok(format!("{f}: подпись верна, выпуск {}, узлов {}, действует до {}", d.sequence, d.entries.len(), d.expires))
        }
        "add" => {
            let f = file(0);
            let mut d = load(&f)?;
            let key_hex = arg(rest, "--key").ok_or("нужен --key")?.to_ascii_lowercase();
            // the id is derived from the key; --id may be omitted (a given one must agree with the key, which check_shape verifies)
            let derived_id = hex::decode(&key_hex).ok().and_then(|k| <[u8; 32]>::try_from(k).ok()).map(|k| hex::encode(crate::util::types::derive_node_id(&k)));
            let e = Entry {
                id: arg(rest, "--id").map(|i| i.to_ascii_lowercase()).or(derived_id).ok_or("нужен --id или правильный --key")?,
                key: key_hex,
                addr: all_args(rest, "--addr"),
                region: arg(rest, "--region").map(|r| r.to_ascii_uppercase()),
                roles: arg(rest, "--roles").map(|r| r.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_else(|| vec!["entry".into()]),
                mobile: all_args(rest, "--mobile"),
            };
            d.entries.retain(|x| x.id != e.id);
            d.entries.push(e);
            d.check_shape().map_err(|e| e.to_string())?;
            d.signature.clear();
            save(&f, &d)?;
            Ok(format!("{f}: узлов {}. Подпись сброшена — выполните `yandi bootstrap sign`.", d.entries.len()))
        }
        "from-card" => {
            let f = file(0);
            let card_text = pos.get(1).cloned().ok_or("укажите визитку")?;
            let card = crate::web::peers::decode_card(&card_text)?;
            let mut d = load(&f)?;
            let region = arg(rest, "--region").map(|r| r.to_ascii_uppercase());
            let roles = arg(rest, "--roles").map(|r| r.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_else(|| vec!["entry".into(), "relay".into()]);
            d.entries.retain(|x| x.id != card.id);
            d.entries.push(Entry { id: card.id.clone(), key: card.key.clone(), addr: card.addr.clone(), region, roles, mobile: all_args(rest, "--mobile") });
            d.check_shape().map_err(|e| e.to_string())?;
            d.signature.clear();
            save(&f, &d)?;
            Ok(format!("{f}: добавлен узел {}…, адресов {}. Подпись сброшена — выполните `yandi bootstrap sign`.", &card.id[..8], card.addr.len()))
        }
        "remove" => {
            let f = file(0);
            let id = arg(rest, "--id").ok_or("нужен --id")?.to_ascii_lowercase();
            let mut d = load(&f)?;
            let before = d.entries.len();
            d.entries.retain(|x| !x.id.starts_with(&id));
            if d.entries.len() == before {
                return Err("такого узла в списке нет".into());
            }
            d.signature.clear();
            save(&f, &d)?;
            Ok(format!("{f}: узлов {}. Подпись сброшена — выполните `yandi bootstrap sign`.", d.entries.len()))
        }
        // отзыв сборок (docs/ROADMAP_R9.md §5): минимальные версии узла и приложения; пустое значение снимает требование
        "min" => {
            let f = file(0);
            let mut d = load(&f)?;
            let norm = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            if let Some(v) = arg(rest, "--node") {
                d.min_node = norm(Some(v));
            }
            if let Some(v) = arg(rest, "--app") {
                d.min_app = norm(Some(v));
            }
            d.signature.clear();
            save(&f, &d)?;
            Ok(format!("{f}: минимальная версия узла {}, приложения {}. Подпись сброшена — выполните `yandi admin bootstrap sign`.",
                d.min_node.as_deref().unwrap_or("—"), d.min_app.as_deref().unwrap_or("—")))
        }
        _ => Err(usage()),
    })();
    match result {
        Ok(t) => (0, t),
        Err(e) => (2, e),
    }
}

fn usage() -> String {
    "Использование:\n  yandi bootstrap show|verify [файл]\n  yandi bootstrap add [файл] [--id H] --key H --addr A [--addr A] [--mobile АДРЕС:ПОРТ] [--region XX] [--roles entry,relay,exit,guest]\n  yandi bootstrap from-card [файл] <визитка> [--mobile АДРЕС:ПОРТ] [--region XX] [--roles ...,guest]\n  yandi bootstrap remove [файл] --id H\n  yandi bootstrap min [файл] [--node ВЕРСИЯ] [--app ВЕРСИЯ]   отозвать старые сборки (пусто — снять)\n  подпись: yandi admin bootstrap sign <файл> (ключ — в закрытой среде узла, файла нет)".to_string()
}

#[cfg(test)]
mod tool_tests {
    use super::*;

    #[test]
    fn the_owner_edits_the_list_offline_but_signing_is_only_done_by_the_closed_environment() {
        let dir = tempfile::tempdir().unwrap();
        let list = dir.path().join("b.json").display().to_string();
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        // файловых ключей больше нет: ни keygen, ни подпись ключом из файла
        for cmd in ["keygen", "sign"] {
            let (code, out) = run(s(&[cmd, &list, "--key", "/nonexistent"]));
            assert_eq!(code, 2);
            assert!(out.contains("закрытой среды") && out.contains("файла с ключом нет"), "{out}");
        }
        let signer = SigningKey::from_bytes(&[3; 32]);
        let key = hex::encode(signer.verifying_key().to_bytes());
        let id = hex::encode(crate::util::types::derive_node_id(&signer.verifying_key().to_bytes()));
        // пустой список нужно завести: без подписи (её даст среда), форму проверяет add/remove
        let empty = Doc { format: 1, network: "yandi".into(), issued: 0, expires: 0, sequence: 0, entries: vec![], min_node: None, min_app: None, signer: String::new(), signature: String::new() };
        save(&list, &empty).unwrap();
        let (code, out) = run(s(&["add", &list, "--id", &id, "--key", &key, "--addr", "203.0.113.5:9000", "--mobile", "203.0.113.5:8444", "--region", "nl", "--roles", "entry,relay,guest"]));
        assert_eq!(code, 0, "{out}");
        assert_eq!(run(s(&["add", &list, "--id", "nothex", "--key", &key, "--addr", "203.0.113.5:9000"])).0, 2, "a broken entry is refused");
        assert_eq!(run(s(&["add", &list, "--key", &key, "--addr", "203.0.113.5:9000", "--roles", "guest"])).0, 2, "guest without a phone address is refused");
        let d = load(&list).unwrap();
        assert_eq!(d.entries[0].region.as_deref(), Some("NL"));
        assert_eq!(d.entries[0].mobile, vec!["203.0.113.5:8444".to_string()]);
        assert_eq!(d.signature, "", "any edit drops the signature");
        // подпись (в узле: signer::sign_doc) принимается проверкой, а правка после неё сбрасывает подпись
        let mut live = d.clone();
        live.expires = now() + 1000;
        live.sequence = 1;
        let signed = live.sign(&signer);
        let pk = hex::encode(signer.verifying_key().to_bytes());
        assert_eq!(signed.verify(&[pk.as_str()], now(), None), Ok(()));
        let (code, _) = run(s(&["remove", &list, "--id", &id[..8]]));
        assert_eq!(code, 0);
        assert_eq!(load(&list).unwrap().entries.len(), 0);
    }
}
