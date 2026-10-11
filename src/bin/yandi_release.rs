//! Выпуск ноды: `manifest`, `sign`, `pubkey`, `verify`. Фраза — две строки на stdin (скрытый ввод делает `tools/node_release.sh`).
use ed25519_dalek::VerifyingKey;
use std::io::BufRead;
use std::path::PathBuf;
use yandi::release::*;

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

fn read_phrase_key(
    mut input: impl BufRead,
    params: argon2::Params,
) -> Result<ed25519_dalek::SigningKey, String> {
    fn read_normalized(input: &mut impl BufRead) -> Result<zeroize::Zeroizing<String>, String> {
        let mut raw = zeroize::Zeroizing::new(String::new());
        input.read_line(&mut raw).map_err(|e| e.to_string())?;
        Ok(normalize(&raw))
    }
    let first = read_normalized(&mut input)?;
    let second = read_normalized(&mut input)?;
    if first != second {
        return Err("Фразы не совпали.".into());
    }
    if let Some(why) = weakness(&first) {
        return Err(format!("Фраза слишком слабая: {why}."));
    }
    Ok(derive_signing_key(&first, params))
}
fn phrase_key() -> ed25519_dalek::SigningKey {
    read_phrase_key(std::io::stdin().lock(), production_params()).unwrap_or_else(|e| die(&e))
}

#[cfg(test)]
mod secret_tests {
    use super::*;
    #[test]
    fn r6_phrase_errors_return_through_zeroizing_scope() {
        let params = argon2::Params::new(64, 1, 1, Some(32)).unwrap();
        assert!(read_phrase_key(&b"secret\nother\n"[..], params.clone())
            .unwrap_err()
            .contains("совпали"));
        assert!(read_phrase_key(&b"short\nshort\n"[..], params)
            .unwrap_err()
            .contains("слабая"));
    }
}

fn parse_pubkey(hex_s: &str) -> VerifyingKey {
    if !lower_hex(hex_s.trim(), 64) {
        die("--pubkey: нужны 64 lowercase hex-символа");
    }
    let bytes: [u8; 32] = hex::decode(hex_s.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .unwrap_or_else(|| die("--pubkey: нужно 64 hex-символа"));
    VerifyingKey::from_bytes(&bytes).unwrap_or_else(|_| die("--pubkey: непригодный ключ"))
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        #[cfg(target_os = "linux")]
        Some(cmd @ ("update" | "rollback" | "status")) => {
            let mut from = None;
            let mut pubkey = None;
            let mut install = dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("yandi-node");
            let mut seq = None;
            let mut it = args[1..].iter();
            while let Some(arg) = it.next() {
                let value = it.next().unwrap_or_else(|| die("нужно значение параметра"));
                match arg.as_str() {
                    "--from" if cmd == "update" => from = Some(value.clone()),
                    "--pubkey" if cmd == "update" => pubkey = Some(value.clone()),
                    "--install-dir" => install = PathBuf::from(value),
                    "--min-seq" | "--current-seq" if cmd == "update" => {
                        seq = Some(
                            value
                                .parse()
                                .unwrap_or_else(|_| die("--current-seq: число")),
                        )
                    }
                    _ => die(&format!("неизвестный параметр: {arg}")),
                }
            }
            let result = match cmd {
                "update" => {
                    let source = from.unwrap_or_else(|| die("нужен --from"));
                    let vk = parse_pubkey(&pubkey.unwrap_or_else(|| die("нужен --pubkey")));
                    yandi::updater::update(
                        &source,
                        &vk,
                        &install,
                        seq.unwrap_or_else(|| {
                            die("нужен --min-seq (0 только для первой установки)")
                        }),
                    )
                    .await
                    .map(Some)
                }
                "rollback" => yandi::updater::rollback(&install).map(Some),
                _ => yandi::updater::status(&install),
            }
            .unwrap_or_else(|e| die(&format!("ОШИБКА: {e:#}")));
            if let Some(s) = result {
                println!(
                    "{} (№{}), максимум seq {}, ключ {}",
                    s.version, s.seq, s.max_seq, s.key_id
                );
                if cmd != "status" {
                    println!("Установлено в {}. Перезапуск выполняет владелец: sudo systemctl restart yandi-node", install.display());
                }
            } else {
                println!("выпуск ещё не установлен");
            }
        }
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
            let seq: u64 = args[3]
                .parse()
                .unwrap_or_else(|_| die("seq должен быть числом"));
            let vk = parse_pubkey(&args[5]);
            let now = match std::env::var("SOURCE_DATE_EPOCH") {
                Ok(v) => v
                    .parse()
                    .unwrap_or_else(|_| die("SOURCE_DATE_EPOCH: число")),
                Err(_) => std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            };
            let m = build_manifest(&dir, &args[2], seq, now, &key_id(&vk), &args[6..])
                .unwrap_or_else(|e| die(&e));
            std::fs::write(dir.join(MANIFEST_NAME), manifest_bytes(&m))
                .unwrap_or_else(|e| die(&e.to_string()));
            println!(
                "манифест записан: {} файлов, выпуск №{}",
                m.files.len(),
                m.seq
            );
        }
        Some("sign") => {
            let dir = PathBuf::from(
                args.get(1)
                    .unwrap_or_else(|| die("использование: yandi-release sign <каталог>")),
            );
            let bytes = ReleaseDir::open(&dir)
                .and_then(|d| d.read(MANIFEST_NAME, MAX_MANIFEST as u64))
                .unwrap_or_else(|e| die(&format!("{MANIFEST_NAME}: {e}")));
            let m = parse_manifest(&bytes).unwrap_or_else(|e| die(&e.to_string()));
            check_files(&dir, &m).unwrap_or_else(|e| die(&e.to_string()));
            let k = phrase_key();
            std::fs::write(
                dir.join(SIGNATURE_NAME),
                sign(&bytes, &k).unwrap_or_else(|e| die(&e.to_string())),
            )
            .unwrap_or_else(|e| die(&e.to_string()));
            println!("подписано ключом {}", key_id(&k.verifying_key()));
        }
        Some("verify") => {
            // verify <каталог> --pubkey HEX --min-seq N
            let dir = PathBuf::from(args.get(1).unwrap_or_else(|| {
                die("использование: yandi-release verify <каталог> --pubkey HEX --min-seq N")
            }));
            let mut pubkey = None;
            let mut min_seq = None;
            let mut it = args[2..].iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--pubkey" => pubkey = it.next().cloned(),
                    "--min-seq" => {
                        min_seq = Some(
                            it.next()
                                .and_then(|v| v.parse().ok())
                                .unwrap_or_else(|| die("--min-seq: число")),
                        )
                    }
                    other => die(&format!("неизвестный ключ: {other}")),
                }
            }
            let vk = parse_pubkey(
                &pubkey.unwrap_or_else(|| die("нужен --pubkey (закреплённый открытый ключ)")),
            );
            let bytes = ReleaseDir::open(&dir)
                .and_then(|d| d.read(MANIFEST_NAME, MAX_MANIFEST as u64))
                .unwrap_or_else(|e| die(&format!("{MANIFEST_NAME}: {e}")));
            let sig = ReleaseDir::open(&dir)
                .and_then(|d| d.read(SIGNATURE_NAME, 256))
                .and_then(|b| String::from_utf8(b).map_err(std::io::Error::other))
                .unwrap_or_else(|e| die(&format!("{SIGNATURE_NAME}: {e}")));
            let m = verify_manifest(
                &bytes,
                &sig,
                &vk,
                min_seq.unwrap_or_else(|| die("нужен --min-seq")),
            )
            .unwrap_or_else(|e| die(&format!("ПРОВЕРКА НЕ ПРОЙДЕНА: {e}")));
            check_files(&dir, &m).unwrap_or_else(|e| die(&format!("ПРОВЕРКА НЕ ПРОЙДЕНА: {e}")));
            println!(
                "ВЫПУСК ВЕРЕН: {} {} (№{}), файлов {}, ключ {}",
                m.product,
                m.version,
                m.seq,
                m.files.len(),
                m.key_id
            );
        }
        _ => die("команды: pubkey | manifest | sign | verify | update | rollback | status"),
    }
}
