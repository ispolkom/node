//! Файл, запечатанный ключом закрытой среды: `nonce(12) || AES-256-GCM(plain)`; `aad` привязывает содержимое к назначению.
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use std::path::Path;

pub fn seal(key: &[u8; 32], aad: &str, plain: &[u8]) -> Result<Vec<u8>, String> {
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let ct = Aes256Gcm::new_from_slice(key)
        .map_err(|e| e.to_string())?
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad: aad.as_bytes() })
        .map_err(|_| "шифрование".to_string())?;
    Ok([nonce.as_slice(), &ct].concat())
}

pub fn open(key: &[u8; 32], aad: &str, blob: &[u8]) -> Option<Vec<u8>> {
    if blob.len() < 12 + 16 {
        return None;
    }
    Aes256Gcm::new_from_slice(key)
        .ok()?
        .decrypt(Nonce::from_slice(&blob[..12]), Payload { msg: &blob[12..], aad: aad.as_bytes() })
        .ok()
}

/// Записать и проверить прочитанным обратно, только потом считать записанным.
pub fn write(path: &Path, key: &[u8; 32], aad: &str, plain: &[u8]) -> Result<(), String> {
    let blob = seal(key, aad, plain)?;
    crate::util::private_file::write_private(path, &blob).map_err(|e| e.to_string())?;
    let back = std::fs::read(path).map_err(|e| e.to_string())?;
    if open(key, aad, &back).as_deref() != Some(plain) {
        return Err("проверка записанного файла не прошла".into());
    }
    Ok(())
}

/// Затереть нулями и удалить (открытая копия после переноса).
pub fn wipe_remove(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::write(path, vec![0u8; meta.len() as usize]);
    }
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_file_is_bound_to_key_and_purpose() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f.enc");
        write(&p, &[1; 32], "proxy", b"secret").unwrap();
        let blob = std::fs::read(&p).unwrap();
        assert!(!blob.windows(6).any(|w| w == b"secret"));
        assert_eq!(open(&[1; 32], "proxy", &blob).unwrap(), b"secret");
        assert!(open(&[2; 32], "proxy", &blob).is_none());
        assert!(open(&[1; 32], "other", &blob).is_none());
        let mut bad = blob.clone();
        bad[20] ^= 1;
        assert!(open(&[1; 32], "proxy", &bad).is_none());
    }
}
