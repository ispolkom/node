//! The web account: the login password, the first-time creation of the keys from what the person typed, and the reset of the login
//! password by the recovery secret. ONE implementation, used by the node's web page, by `yandi-keys`, and — through `yandi-keys` — by the
//! second web page (the Python assistant), so that both open with the same password and the same recovery secret.
//!
//! Error messages are short, in Russian, and safe to show to the person: they never contain a password, a key or a path.
use crate::machine::MachineContext;
use crate::root::RootDocument;
use crate::wrap::{KdfParams, KdfPolicy};
use crate::{recovery_secret, DeviceKeyProvider, KeyDir, KeyRootError, RecoveryCode, Zeroizing};

/// Hash a login password (Argon2id, PHC string).
pub fn hash_login_password(password: &str) -> Result<String, String> {
    use argon2::password_hash::{PasswordHasher, SaltString};
    let salt = SaltString::generate(&mut rand::rngs::OsRng);
    argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("Password hash failed: {}", e))
}

/// Verify a login password against a stored PHC hash.
pub fn verify_login_password(password: &str, stored_hash: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    let Ok(hash) = PasswordHash::new(stored_hash) else {
        return false;
    };
    argon2::Argon2::default()
        .verify_password(password.as_bytes(), &hash)
        .is_ok()
}

/// The web login hash of either auth.json format (1: machine-wrapped key, 2: device + recovery wrappers).
pub fn login_hash_of(json: &str) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Failed to parse auth.json: {}", e))?;
    value
        .get("login_hash")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| "auth.json has no login hash".to_string())
}

/// Is `password` the login password of the account in `dir`? `Err` when there is no account or its file cannot be read.
pub fn verify_login_in(dir: &KeyDir, password: &str) -> Result<bool, String> {
    let bytes = crate::keydir::read_private_file(&dir.auth_file()).map_err(|e| match e {
        KeyRootError::NotFound => "Вход ещё не настроен".to_string(),
        _ => "Файл ключей недоступен".to_string(),
    })?;
    let json = String::from_utf8(bytes).map_err(|_| "Файл ключей повреждён".to_string())?;
    Ok(verify_login_password(password, &login_hash_of(&json)?))
}

/// Never overwrite existing key material: an auth.json that exists (even one this build cannot open) is somebody's master key.
pub fn ensure_no_existing_auth(path: &std::path::Path) -> Result<(), String> {
    if std::fs::symlink_metadata(path).is_ok() {
        return Err(
            "Auth is already set up; refusing to overwrite the existing key file".to_string(),
        );
    }
    Ok(())
}

/// The rules for what the person typed at first setup, in words that are safe to show.
pub fn check_setup_inputs(
    login: &str,
    login_repeat: &str,
    master: &str,
    master_repeat: &str,
) -> Result<(), String> {
    if login.chars().count() < 8 {
        return Err("Пароль входа: минимум 8 символов".to_string());
    }
    if login != login_repeat {
        return Err("Пароли входа не совпадают".to_string());
    }
    if master.chars().count() < 12 {
        return Err(
            "Мастер-пароль: минимум 12 символов (лучше фраза из нескольких слов)".to_string(),
        );
    }
    if master != master_repeat {
        return Err("Мастер-пароли не совпадают".to_string());
    }
    if master == login {
        return Err("Мастер-пароль должен отличаться от пароля входа".to_string());
    }
    Ok(())
}

/// First-time creation of the keys from what the person typed: a login password and a master password (the person's own recovery
/// secret — not generated). The actual root key is random; it is wrapped for THIS device (a device key file) and for the master password
/// (Argon2id). Nothing is stored that could open the root without the device key or the master password. Fails, changing nothing, if key
/// material already exists. Returns the root (the caller decides what to do with it: the node keeps it in memory, the tool drops it).
pub fn create_keys(
    dir: &KeyDir,
    machine: &dyn MachineContext,
    device: &dyn DeviceKeyProvider,
    login_password: &str,
    master_password: &str,
    params: KdfParams,
    policy: &KdfPolicy,
) -> Result<Zeroizing<[u8; 32]>, String> {
    let auth_path = dir.auth_file();
    ensure_no_existing_auth(&auth_path)?;
    if device.load().map(|k| k.is_some()).unwrap_or(true) {
        return Err("В каталоге ключей уже есть ключ устройства; настройка не выполнена, чтобы ничего не затереть".to_string());
    }
    let root = Zeroizing::new(crate::wrap::random_bytes::<32>());
    let login_hash = hash_login_password(login_password)?;
    let device_key = device
        .create()
        .map_err(|_| "Не удалось создать ключ устройства".to_string())?;
    let created = (|| -> Result<(), String> {
        let doc = RootDocument::create(
            &root,
            &login_hash,
            &device_key,
            device.name(),
            machine,
            master_password,
            params,
            policy,
        )
        .map_err(|e| format!("Не удалось создать ключи: {}", e))?;
        crate::atomic::create_new_file(&auth_path, &doc.to_json())
            .map_err(|_| "Не удалось записать файл ключей".to_string())?;
        // both ways in must open the very root that was just made, read back from disk
        let bytes = crate::keydir::read_private_file(&auth_path)
            .map_err(|_| "Файл ключей не прочитался".to_string())?;
        let doc = RootDocument::parse(&bytes)
            .map_err(|_| "Файл ключей не прошёл проверку".to_string())?;
        let by_device = doc
            .unlock_with_device(&device_key, machine)
            .map_err(|_| "Ключ устройства не открывает корень".to_string())?;
        let by_password = doc
            .unlock_with_password(master_password, policy)
            .map_err(|_| "Мастер-пароль не открывает корень".to_string())?;
        if *by_device != *root || *by_password != *root {
            return Err("Проверка ключей не прошла".to_string());
        }
        Ok(())
    })();
    if let Err(e) = created {
        // undo only what this call has just created (nothing existed before: that was checked above)
        let _ = std::fs::remove_file(&auth_path);
        let _ = std::fs::remove_file(dir.file("device.key"));
        return Err(e);
    }
    Ok(root)
}

/// Forgot the web password: the recovery secret proves ownership and sets a new one. The recovery wrapper is opened (Argon2id: about half
/// a second per attempt, and the caller throttles attempts); only then is `auth.json` rewritten, atomically, with the same wrappers and
/// the new login hash. Errors are short messages that are safe to show.
pub fn reset_login(
    dir: &KeyDir,
    code_input: &str,
    new_login_password: &str,
    policy: &KdfPolicy,
) -> Result<(), String> {
    if new_login_password.chars().count() < 8 {
        return Err("Новый пароль входа должен быть не короче 8 символов".to_string());
    }
    let bytes = crate::keydir::read_private_file(&dir.auth_file())
        .map_err(|_| "Файл ключей недоступен".to_string())?;
    match crate::legacy::auth_format(&bytes) {
        Ok(crate::legacy::AuthFormat::RootV2) => {}
        Ok(_) => {
            return Err(
                "Ключи в старом формате: сначала выполните `yandi-keys migrate`".to_string(),
            )
        }
        Err(_) => return Err("Файл ключей повреждён".to_string()),
    }
    // a code with a typo is a typo, not "wrong"
    if crate::recovery_code::looks_like_code(code_input) && RecoveryCode::parse(code_input).is_err()
    {
        return Err(
            "В коде опечатка: он не проходит проверку. Проверьте, что записано, и введите ещё раз"
                .to_string(),
        );
    }
    let doc = RootDocument::parse(&bytes).map_err(|_| "Файл ключей повреждён".to_string())?;
    let secret = recovery_secret(code_input);
    let root = match doc.unlock_with_password(&secret, policy) {
        Ok(r) => r,
        Err(KeyRootError::RecoveryFailed) => {
            return Err("Код восстановления не подошёл".to_string())
        }
        Err(_) => return Err("Файл ключей повреждён".to_string()),
    };
    let hash = hash_login_password(new_login_password)?;
    let next = doc.with_login_hash(&hash).to_json();
    let auth_path = dir.auth_file();
    crate::atomic::backup_copy(&auth_path, "before-login-reset")
        .map_err(|_| "Не удалось сохранить копию ключей; ничего не изменено".to_string())?;
    crate::atomic::write_atomic(&auth_path, &next, |written| {
        RootDocument::parse(written)
            .and_then(|d| {
                Ok(d.login_hash == hash && *d.unlock_with_password(&secret, policy)? == *root)
            })
            .unwrap_or(false)
    })
    .map_err(|_| "Не удалось записать новый пароль; ничего не изменено".to_string())?;
    Ok(())
}
