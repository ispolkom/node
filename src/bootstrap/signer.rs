//! Ключ подписи списка входных узлов: живёт ТОЛЬКО в закрытой среде узла.
//!
//! Файла с ключом нет нигде. Ключ — подключ закрытой среды по отдельному домену (`DOMAIN_BOOTSTRAP_SIGNER`): он выводится из ключа среды,
//! появляется в памяти, когда владелец открыл среду паролем, и стирается при запирании. Сид-фраза восстанавливает среду, а с ней и тот же
//! ключ: отдельной резервной копии ключа не нужно. Секрет наружу не выходит: узел сам подписывает список по запросу администратора
//! (`/api/admin/bootstrap/sign`) и отдаёт только открытый ключ и подписанный список.
use super::{Doc, BootstrapError, SIGNER_KEYS};
use ed25519_dalek::SigningKey;
use std::sync::Mutex;
use zeroize::Zeroizing;

static KEY: Mutex<Option<Zeroizing<[u8; 32]>>> = Mutex::new(None);

/// Среда открыта: ключ подписи списка доступен.
pub fn unlock(key: [u8; 32]) {
    *KEY.lock().unwrap_or_else(|e| e.into_inner()) = Some(Zeroizing::new(key));
}

/// Среда заперта: ключ стирается из памяти.
pub fn lock() {
    *KEY.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub fn is_unlocked() -> bool {
    KEY.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

fn key() -> Option<SigningKey> {
    KEY.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|k| SigningKey::from_bytes(k))
}

/// Открытый ключ подписи списка (hex): его записывают в `bootstrap/signer.pub`, он вшивается в узлы и приложения. `None` — среда заперта.
pub fn public_hex() -> Option<String> {
    key().map(|k| hex::encode(k.verifying_key().to_bytes()))
}

/// Закреплён ли этот ключ в программе (`SIGNER_KEYS`): иначе узлы и приложения такой список не примут.
pub fn is_pinned(public_hex: &str) -> bool {
    SIGNER_KEYS.iter().any(|k| k.trim().eq_ignore_ascii_case(public_hex))
}

/// Подписать список: номер выпуска +1, выпущен сейчас, срок — `days` дней. Среда заперта или список неверен — ошибка, ничего не меняется.
pub fn sign_doc(mut doc: Doc, days: u64, now_secs: u64, network: Option<String>) -> Result<Doc, String> {
    let key = key().ok_or("закрытая среда заперта: откройте её паролем")?;
    if !(1..=366).contains(&days) {
        return Err("срок действия списка: от 1 до 366 дней".into());
    }
    if let Some(n) = network {
        doc.network = n;
    }
    doc.issued = now_secs;
    doc.expires = now_secs + days * 86_400;
    doc.sequence = doc.sequence.checked_add(1).ok_or("номер выпуска переполнен")?;
    doc.check_shape().map_err(|e: BootstrapError| e.to_string())?;
    let signed = doc.sign(&key);
    signed.check_shape().map_err(|e| e.to_string())?;
    Ok(signed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::{Entry, Doc};

    fn doc() -> Doc {
        let k = SigningKey::from_bytes(&[5u8; 32]);
        let key_hex = hex::encode(k.verifying_key().to_bytes());
        let id = hex::encode(crate::util::types::derive_node_id(&k.verifying_key().to_bytes()));
        Doc {
            format: 1,
            network: "yandi".into(),
            issued: 1,
            expires: 2,
            sequence: 3,
            entries: vec![Entry { id, key: key_hex, addr: vec!["203.0.113.5:9000".into()], region: Some("NL".into()), roles: vec!["entry".into(), "guest".into()], mobile: vec!["203.0.113.5:8444".into()] }],
            signer: String::new(),
            signature: String::new(),
        }
    }

    // ключ общий на процесс: проверки идут одним тестом, чтобы параллельные тесты не гоняли его друг у друга
    #[test]
    fn the_key_exists_only_while_the_environment_is_open_and_signs_without_ever_leaving() {
        lock();
        assert!(!is_unlocked());
        assert_eq!(public_hex(), None);
        assert!(sign_doc(doc(), 90, 1_000_000, None).unwrap_err().contains("заперта"), "заперто — подписи нет");

        unlock([9u8; 32]);
        let pubkey = public_hex().expect("открыто — открытый ключ есть");
        assert_eq!(pubkey, public_hex().unwrap(), "один и тот же ключ среды — один и тот же открытый ключ (сид-фраза возвращает его)");
        let signed = sign_doc(doc(), 90, 1_000_000, None).expect("подписано");
        assert_eq!((signed.sequence, signed.issued, signed.expires), (4, 1_000_000, 1_000_000 + 90 * 86_400));
        assert_eq!(signed.signer, pubkey);
        // подпись настоящая: проверяется тем же правилом, что у узлов и приложения, с закреплением этого ключа
        assert!(signed.verify(&[pubkey.as_str()], 1_000_001, Some(4)).is_ok());
        assert!(signed.verify(&[pubkey.as_str()], 1_000_001, Some(5)).is_err(), "старый выпуск не принимается");
        assert!(signed.verify(&["00".repeat(32).as_str()], 1_000_001, None).is_err(), "чужой закреплённый ключ");
        let mut bad = doc();
        bad.entries[0].roles = vec!["boss".into()];
        assert!(sign_doc(bad, 90, 1, None).is_err(), "неверный список не подписывается");
        assert!(sign_doc(doc(), 0, 1, None).is_err() && sign_doc(doc(), 400, 1, None).is_err());
        // другой ключ среды — другой открытый ключ
        unlock([10u8; 32]);
        assert_ne!(public_hex().unwrap(), pubkey);
        lock();
        assert_eq!(public_hex(), None, "запирание стирает ключ из памяти");
    }
}
