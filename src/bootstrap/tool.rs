//! `yandi bootstrap …` — инструменты владельца списка входных узлов (`bootstrap/bootstrap.json`).
//!
//! ```text
//! yandi bootstrap keygen <файл-ключа>                       новый ключ подписи (секрет — в файл 0600, открытый ключ — на экран)
//! yandi bootstrap show   [файл]                             прочитать и показать список
//! yandi bootstrap verify [файл]                             проверить подпись закреплённым ключом проекта
//! yandi bootstrap add    [файл] [--id H] --key H --addr A [--addr A] [--region NL] [--roles entry,relay,exit]
//! yandi bootstrap from-card [файл] <визитка>                добавить узел по его визитке (`~/.yandi/node_card.txt` или страница узла)
//! yandi bootstrap remove [файл] --id H                      убрать узел
//! yandi bootstrap sign   [файл] --key <файл-ключа> [--days 90] [--network yandi]   подписать (выпуск +1, срок +N дней)
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
        Err(_) => Ok(Doc { format: 1, network: "yandi".into(), issued: 0, expires: 0, sequence: 0, entries: vec![], signer: String::new(), signature: String::new() }),
    }
}

fn save(path: &str, d: &Doc) -> Result<(), String> {
    if let Some(dir) = Path::new(path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, serde_json::to_string_pretty(d).map_err(|e| e.to_string())? + "\n").map_err(|e| format!("{path}: {e}"))
}

fn read_key(path: &str) -> Result<SigningKey, String> {
    let s = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let b = hex::decode(s.trim()).map_err(|_| "ключ подписи: не hex".to_string())?;
    let a: [u8; 32] = b.try_into().map_err(|_| "ключ подписи: нужно 32 байта".to_string())?;
    Ok(SigningKey::from_bytes(&a))
}

/// Вернуть код выхода и текст для печати.
pub fn run(args: Vec<String>) -> (i32, String) {
    let Some(cmd) = args.first().map(|s| s.as_str()) else { return (2, usage()) };
    let rest = &args[1..];
    let pos = positional(rest);
    let file = |i: usize| pos.get(i).cloned().unwrap_or_else(|| DEFAULT_FILE.to_string());
    let result: Result<String, String> = (|| match cmd {
        "keygen" => {
            let path = pos.first().ok_or("укажите файл для ключа")?;
            if Path::new(path).exists() {
                return Err(format!("{path} уже есть — не перезаписываю (потеря ключа = потеря возможности обновлять список)"));
            }
            let mut seed = [0u8; 32];
            rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut seed);
            crate::util::private_file::write_private(Path::new(path), hex::encode(seed).as_bytes()).map_err(|e| format!("{path}: {e}"))?;
            let pubkey = hex::encode(SigningKey::from_bytes(&seed).verifying_key().to_bytes());
            Ok(format!("Секретный ключ записан в {path} (права 0600). СОХРАНИТЕ КОПИЮ в надёжном месте и никому не показывайте.\nОткрытый ключ (его закрепляют в программе, файл bootstrap/signer.pub):\n{pubkey}"))
        }
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
            d.entries.push(Entry { id: card.id.clone(), key: card.key.clone(), addr: card.addr.clone(), region, roles });
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
        "sign" => {
            let f = file(0);
            let key = read_key(&arg(rest, "--key").ok_or("нужен --key <файл-ключа>")?)?;
            let days: u64 = arg(rest, "--days").and_then(|d| d.parse().ok()).unwrap_or(90);
            let mut d = load(&f)?;
            if let Some(n) = arg(rest, "--network") {
                d.network = n;
            }
            let t = now();
            d.issued = t;
            d.expires = t + days * 86_400;
            d.sequence += 1;
            let d = d.sign(&key);
            d.check_shape().map_err(|e| e.to_string())?;
            save(&f, &d)?;
            let pinned = SIGNER_KEYS.iter().any(|k| k.trim().eq_ignore_ascii_case(&d.signer));
            Ok(format!("{f}: подписано, выпуск {}, действует {days} дней.{}", d.sequence, if pinned { "" } else { "\nВНИМАНИЕ: этот ключ не закреплён в программе (bootstrap/signer.pub) — узлы такой список не примут." }))
        }
        _ => Err(usage()),
    })();
    match result {
        Ok(t) => (0, t),
        Err(e) => (2, e),
    }
}

fn usage() -> String {
    "Использование:\n  yandi bootstrap keygen <файл-ключа>\n  yandi bootstrap show|verify [файл]\n  yandi bootstrap add [файл] [--id H] --key H --addr A [--addr A] [--region XX] [--roles entry,relay,exit]\n  yandi bootstrap from-card [файл] <визитка> [--region XX] [--roles ...]\n  yandi bootstrap remove [файл] --id H\n  yandi bootstrap sign [файл] --key <файл-ключа> [--days 90] [--network yandi]".to_string()
}

#[cfg(test)]
mod tool_tests {
    use super::*;

    #[test]
    fn the_owner_can_build_sign_and_check_a_list_from_scratch() {
        let dir = tempfile::tempdir().unwrap();
        let (list, keyfile) = (dir.path().join("b.json").display().to_string(), dir.path().join("k").display().to_string());
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let (code, out) = run(s(&["keygen", &keyfile]));
        assert_eq!(code, 0, "{out}");
        assert_eq!(run(s(&["keygen", &keyfile])).0, 2, "an existing key is never overwritten");
        let key = hex::encode(SigningKey::from_bytes(&[3; 32]).verifying_key().to_bytes());
        let id = hex::encode(crate::util::types::derive_node_id(&SigningKey::from_bytes(&[3; 32]).verifying_key().to_bytes()));
        let (code, out) = run(s(&["add", &list, "--id", &id, "--key", &key, "--addr", "203.0.113.5:9000", "--region", "nl", "--roles", "entry,relay"]));
        assert_eq!(code, 0, "{out}");
        assert_eq!(run(s(&["add", &list, "--id", "nothex", "--key", &key, "--addr", "203.0.113.5:9000"])).0, 2, "a broken entry is refused");
        let (code, out) = run(s(&["sign", &list, "--key", &keyfile, "--days", "30"]));
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("выпуск 1") && out.contains("не закреплён"), "{out}");
        let d = load(&list).unwrap();
        let signer = hex::encode(read_key(&keyfile).unwrap().verifying_key().to_bytes());
        assert_eq!(d.verify(&[&signer], now(), None), Ok(()));
        assert_eq!(d.entries[0].region.as_deref(), Some("NL"));
        let (_, out) = run(s(&["sign", &list, "--key", &keyfile]));
        assert!(out.contains("выпуск 2"), "{out}");
        let (code, _) = run(s(&["remove", &list, "--id", &id[..8]]));
        assert_eq!(code, 0);
        assert_eq!(load(&list).unwrap().entries.len(), 0);
        assert_eq!(load(&list).unwrap().signature, "", "any edit drops the signature");
    }
}
