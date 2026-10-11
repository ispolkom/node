//! Выпуск ноды: `manifest`, `sign`, `pubkey`, `verify`. Фраза — две строки на stdin (скрытый ввод делает `tools/node_release.sh`).
use ed25519_dalek::VerifyingKey;
use std::io::BufRead;
use std::path::PathBuf;
use yandi::release::*;

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

fn phrase_key() -> ed25519_dalek::SigningKey {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let first = normalize(&lines.next().and_then(|l| l.ok()).unwrap_or_default());
    let second = normalize(&lines.next().and_then(|l| l.ok()).unwrap_or_default());
    if first != second {
        die("Фразы не совпали.");
    }
    if let Some(why) = weakness(&first) {
        die(&format!("Фраза слишком слабая: {why}."));
    }
    derive_signing_key(&first, production_params())
}

fn parse_pubkey(hex_s: &str) -> VerifyingKey {
    let bytes: [u8; 32] = hex::decode(hex_s.trim()).ok().and_then(|b| b.try_into().ok()).unwrap_or_else(|| die("--pubkey: нужно 64 hex-символа"));
    VerifyingKey::from_bytes(&bytes).unwrap_or_else(|_| die("--pubkey: непригодный ключ"))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("pubkey") => {
            let k = phrase_key();
            println!("{}", hex::encode(k.verifying_key().to_bytes()));
        }
        Some("manifest") => {
            // manifest <каталог> <версия> <seq> <файл>...   (key_id берётся из открытого ключа: --pubkey HEX обязателен ПЕРЕД файлами)
            if args.len() < 7 || args[4] != "--pubkey" {
                die("использование: yandi-release manifest <каталог> <версия> <seq> --pubkey HEX <файл>...");
            }
            let dir = PathBuf::from(&args[1]);
            let seq: u64 = args[3].parse().unwrap_or_else(|_| die("seq должен быть числом"));
            let vk = parse_pubkey(&args[5]);
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let m = build_manifest(&dir, &args[2], seq, now, &key_id(&vk), &args[6..]).unwrap_or_else(|e| die(&e));
            std::fs::write(dir.join(MANIFEST_NAME), manifest_bytes(&m)).unwrap_or_else(|e| die(&e.to_string()));
            println!("манифест записан: {} файлов, выпуск №{}", m.files.len(), m.seq);
        }
        Some("sign") => {
            let dir = PathBuf::from(args.get(1).unwrap_or_else(|| die("использование: yandi-release sign <каталог>")));
            let bytes = std::fs::read(dir.join(MANIFEST_NAME)).unwrap_or_else(|e| die(&format!("{MANIFEST_NAME}: {e}")));
            let k = phrase_key();
            std::fs::write(dir.join(SIGNATURE_NAME), sign(&bytes, &k)).unwrap_or_else(|e| die(&e.to_string()));
            println!("подписано ключом {}", key_id(&k.verifying_key()));
        }
        Some("verify") => {
            // verify <каталог> --pubkey HEX [--min-seq N]
            let dir = PathBuf::from(args.get(1).unwrap_or_else(|| die("использование: yandi-release verify <каталог> --pubkey HEX [--min-seq N]")));
            let mut pubkey = None;
            let mut min_seq = 0u64;
            let mut it = args[2..].iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--pubkey" => pubkey = it.next().cloned(),
                    "--min-seq" => min_seq = it.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| die("--min-seq: число")),
                    other => die(&format!("неизвестный ключ: {other}")),
                }
            }
            let vk = parse_pubkey(&pubkey.unwrap_or_else(|| die("нужен --pubkey (закреплённый открытый ключ)")));
            let bytes = std::fs::read(dir.join(MANIFEST_NAME)).unwrap_or_else(|e| die(&format!("{MANIFEST_NAME}: {e}")));
            let sig = std::fs::read_to_string(dir.join(SIGNATURE_NAME)).unwrap_or_else(|e| die(&format!("{SIGNATURE_NAME}: {e}")));
            let m = verify_manifest(&bytes, &sig, &vk, min_seq).unwrap_or_else(|e| die(&format!("ПРОВЕРКА НЕ ПРОЙДЕНА: {e}")));
            check_files(&dir, &m).unwrap_or_else(|e| die(&format!("ПРОВЕРКА НЕ ПРОЙДЕНА: {e}")));
            println!("ВЫПУСК ВЕРЕН: {} {} (№{}), файлов {}, ключ {}", m.product, m.version, m.seq, m.files.len(), m.key_id);
        }
        _ => die("команды: pubkey | manifest | sign | verify"),
    }
}
