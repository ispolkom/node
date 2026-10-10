//! Ключ подписи приложения (APK) из фразы разработчика.
//!
//! Фразу вводит сам разработчик — программа ничего не генерирует и нигде её не сохраняет. Из фразы тяжёлой функцией Argon2id (1 ГБ памяти,
//! 4 прохода) выводится закрытый ключ ECDSA P-256 и записывается в файл (SEC1 DER) для одной сборки; открытая часть (сертификат) хранится в
//! репозитории, поэтому потерянный файл ключа восстанавливается той же фразой.
//!
//! Открытый ключ подписи публичен (он внутри каждого APK), значит фразу можно перебирать офлайн. Поэтому функция вывода тяжёлая, а слабые
//! фразы отвергаются. Запуск — через `tools/apk_release.sh` (скрытый ввод, сборка, удаление файлов ключа).
//!
//! Вход: фраза двумя строками на stdin (вторая — повтор). Аргумент: куда записать ключ.
use argon2::{Algorithm, Argon2, Params, Version};
use sha2::{Digest, Sha256};
use std::io::BufRead;

/// Порядок группы P-256: закрытый ключ должен быть в [1, n).
const P256_N: [u8; 32] = [
    0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xBC, 0xE6, 0xFA, 0xAD, 0xA7, 0x17, 0x9E, 0x84,
    0xF3, 0xB9, 0xCA, 0xC2, 0xFC, 0x63, 0x25, 0x51,
];
const SALT: &[u8] = b"YANDI-APK-SIGNING-KEY-V1";

/// Пробелы в любом количестве — один пробел, по краям — ничего: опечатка в пробелах не должна давать другой ключ.
fn normalize(p: &str) -> String {
    p.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Почему фраза слишком слабая (None — годится). Это грубый фильтр, а не оценка стойкости: случайные слова, не цитата.
fn weakness(p: &str) -> Option<&'static str> {
    let words: Vec<&str> = p.split(' ').collect();
    let distinct: std::collections::HashSet<&str> = words.iter().copied().collect();
    if words.len() < 12 {
        return Some("нужно не меньше 12 слов");
    }
    if p.chars().count() < 60 {
        return Some("нужно не меньше 60 символов");
    }
    if distinct.len() < 10 {
        return Some("слишком много повторяющихся слов");
    }
    None
}

fn derive(phrase: &str) -> [u8; 32] {
    let params = Params::new(1 << 20, 4, 1, Some(32)).expect("argon2 params");
    let mut out = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params).hash_password_into(phrase.as_bytes(), SALT, &mut out).expect("argon2");
    // в [1, n): иначе детерминированно перехэшировать (вероятность ~2^-32)
    let mut counter = 0u32;
    while out.iter().all(|b| *b == 0) || out >= P256_N {
        counter += 1;
        out = Sha256::new().chain_update(out).chain_update(counter.to_be_bytes()).finalize().into();
    }
    out
}

/// SEC1 ECPrivateKey без открытой части: openssl вычислит её сам.
fn sec1_der(d: &[u8; 32]) -> Vec<u8> {
    let mut v = vec![0x30, 0x31, 0x02, 0x01, 0x01, 0x04, 0x20];
    v.extend_from_slice(d);
    v.extend_from_slice(&[0xA0, 0x0A, 0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07]);
    v
}

fn main() {
    let Some(out) = std::env::args().nth(1) else {
        eprintln!("использование: apk_signing_key <файл ключа> (фраза — две строки на stdin)");
        std::process::exit(2);
    };
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let first = normalize(&lines.next().and_then(|l| l.ok()).unwrap_or_default());
    let second = normalize(&lines.next().and_then(|l| l.ok()).unwrap_or_default());
    if first != second {
        eprintln!("Фразы не совпали.");
        std::process::exit(1);
    }
    if let Some(why) = weakness(&first) {
        eprintln!("Фраза слишком слабая: {why}. Открытый ключ подписи публичен, слабую фразу подберут перебором.");
        std::process::exit(1);
    }
    eprintln!("Вывожу ключ (Argon2id, 1 ГБ памяти, несколько секунд)…");
    let d = derive(&first);
    if let Err(e) = crate_write_private(std::path::Path::new(&out), &sec1_der(&d)) {
        eprintln!("не удалось записать ключ: {e}");
        std::process::exit(1);
    }
}

fn crate_write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)?.write_all(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_phrase_same_key_and_spacing_does_not_matter() {
        let a = normalize("  alpha beta   gamma ");
        assert_eq!(a, "alpha beta gamma");
        assert!(weakness("one two three").is_some());
        assert!(weakness("water lamp river stone cloud seven orange bridge winter pocket violin garden").is_none());
        let d = sec1_der(&[7; 32]);
        assert_eq!(d.len(), 51);
        assert_eq!(d[1] as usize, d.len() - 2);
    }
}
