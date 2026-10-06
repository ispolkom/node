// src/web/auth.rs
//! Authentication and session management for YANDI web UI.
//!
//! Two-password model:
//!   login_password  — protects web UI access; browser may remember it
//!   master_password — derives master_key for all at-rest encryption; never stored in browser

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aes_gcm::{aead::{Aead, KeyInit}, Aes256Gcm, Nonce};
use argon2::{Argon2, Params, Algorithm, Version};
use rand::RngCore;
use serde::{Deserialize, Serialize};

// ── Constants ──────────────────────────────────────────────────────────────

const ARGON2_MEMORY_KB: u32 = 32768; // 32 MB
const ARGON2_ITERATIONS: u32 = 2;
const SESSION_COOKIE: &str = "yandi_session";
const SESSION_REMEMBER_SECS: u64 = 30 * 24 * 3600; // 30 days

// ── On-disk auth state ─────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct EncryptedMasterKey {
    /// Argon2id salt for deriving machine key, hex-encoded (32 bytes)
    pub machine_salt: String,
    /// AES-GCM nonce, hex-encoded (12 bytes)
    pub nonce: String,
    /// AES-GCM ciphertext of master_key, hex-encoded (32 + 16 bytes)
    pub ciphertext: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StoredAuth {
    pub version: u32,
    /// Argon2id encoded hash of login_password (from argon2 crate PHC string format)
    pub login_hash: String,
    /// master_key encrypted with machine-derived key
    pub master_key_encrypted: EncryptedMasterKey,
}

// ── Session ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Session {
    pub token: String,
    /// Unix timestamp when session expires
    pub expires_at: u64,
}

impl Session {
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now >= self.expires_at
    }
}

// ── Login rate limiting ────────────────────────────────────────────────────
//
// api_auth_login/api_auth_rebind (web/server.rs) had no throttling at all —
// combined with the web UI defaulting to bind 0.0.0.0 (real, reachable from
// WAN/LAN, confirmed live on this machine) and a login password allowed to
// be as short as 4 characters, this was a genuine, unlimited-speed online
// brute-force surface. A global (not per-IP — this is a single-owner
// dashboard; per-IP would need ConnectInfo plumbing through the whole axum
// server for a marginal gain, and doesn't protect against a distributed
// attempt anyway) exponential backoff after a few free attempts closes it
// without ever permanently locking the real owner out — it only ever slows
// attempts down, never blocks them outright.
const LOGIN_FREE_ATTEMPTS: u32 = 3;
const LOGIN_BACKOFF_BASE_SECS: u64 = 2;
const LOGIN_BACKOFF_MAX_SECS: u64 = 300;

#[derive(Debug, Clone, Copy)]
struct LoginAttempts {
    consecutive_failures: u32,
    last_failure: Instant,
}

impl LoginAttempts {
    fn backoff_after(failures: u32) -> Duration {
        if failures <= LOGIN_FREE_ATTEMPTS {
            return Duration::ZERO;
        }
        let exp = (failures - LOGIN_FREE_ATTEMPTS - 1).min(31);
        let secs = LOGIN_BACKOFF_BASE_SECS.saturating_mul(1u64 << exp);
        Duration::from_secs(secs.min(LOGIN_BACKOFF_MAX_SECS))
    }
}

// ── Auth state (in-memory) ─────────────────────────────────────────────────

#[derive(Clone)]
pub struct AuthState {
    /// In-memory session store
    pub sessions: Arc<Mutex<HashMap<String, Session>>>,
    /// master_key available after successful setup/login/rebind (32 bytes)
    pub master_key: Arc<Mutex<Option<[u8; 32]>>>,
    /// Whether auth has been set up (auth.json exists and loaded)
    pub is_setup: Arc<std::sync::atomic::AtomicBool>,
    /// Whether the machine_id matches (false = need rebind)
    pub needs_rebind: Arc<std::sync::atomic::AtomicBool>,
    /// Login brute-force throttling — see `LoginAttempts`.
    login_attempts: Arc<Mutex<Option<LoginAttempts>>>,
}

impl Default for AuthState {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            master_key: Arc::new(Mutex::new(None)),
            is_setup: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            needs_rebind: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            login_attempts: Arc::new(Mutex::new(None)),
        }
    }
}

impl AuthState {
    /// Returns true if auth.json was loaded and master_key decrypted successfully.
    pub fn is_ready(&self) -> bool {
        self.is_setup.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn needs_rebind(&self) -> bool {
        self.needs_rebind.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn get_master_key(&self) -> Option<[u8; 32]> {
        self.master_key.lock().ok()?.clone()
    }

    /// Create a new session token. Returns the token string.
    pub fn create_session(&self, remember_me: bool) -> String {
        let mut token_bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut token_bytes);
        let token = hex::encode(token_bytes);

        let ttl = if remember_me { SESSION_REMEMBER_SECS } else { 3600 * 24 };
        let expires_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() + ttl;

        let session = Session { token: token.clone(), expires_at };
        if let Ok(mut store) = self.sessions.lock() {
            // Evict expired sessions
            store.retain(|_, s| !s.is_expired());
            store.insert(token.clone(), session);
        }
        token
    }

    /// Verify a session token. Returns true if valid and not expired.
    pub fn verify_session(&self, token: &str) -> bool {
        if let Ok(mut store) = self.sessions.lock() {
            if let Some(session) = store.get(token) {
                if session.is_expired() {
                    store.remove(token);
                    return false;
                }
                return true;
            }
        }
        false
    }

    pub fn invalidate_session(&self, token: &str) {
        if let Ok(mut store) = self.sessions.lock() {
            store.remove(token);
        }
    }

    /// If a login attempt should be rejected right now without even
    /// checking the password (still cooling down from prior failures),
    /// returns how much longer to wait.
    pub fn login_backoff_remaining(&self) -> Option<Duration> {
        let attempts = self.login_attempts.lock().ok()?;
        let a = (*attempts)?;
        let required = LoginAttempts::backoff_after(a.consecutive_failures);
        if required.is_zero() {
            return None;
        }
        let elapsed = a.last_failure.elapsed();
        if elapsed >= required {
            None
        } else {
            Some(required - elapsed)
        }
    }

    pub fn record_login_failure(&self) {
        if let Ok(mut attempts) = self.login_attempts.lock() {
            let failures = attempts.map(|a| a.consecutive_failures).unwrap_or(0) + 1;
            *attempts = Some(LoginAttempts { consecutive_failures: failures, last_failure: Instant::now() });
        }
    }

    pub fn record_login_success(&self) {
        if let Ok(mut attempts) = self.login_attempts.lock() {
            *attempts = None;
        }
    }
}

// ── Key derivation helpers ─────────────────────────────────────────────────

/// Derive a 32-byte key from a passphrase + salt using Argon2id.
pub fn derive_key(passphrase: &[u8], salt: &[u8; 32]) -> Result<[u8; 32], String> {
    let params = Params::new(ARGON2_MEMORY_KB, ARGON2_ITERATIONS, 1, Some(32))
        .map_err(|e| format!("Argon2 params: {}", e))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon2.hash_password_into(passphrase, salt, &mut key)
        .map_err(|e| format!("Argon2 KDF failed: {}", e))?;
    Ok(key)
}

/// Return machine-specific passphrase used to protect the stored master_key.
fn machine_passphrase() -> Vec<u8> {
    let machine_id = std::fs::read_to_string("/etc/machine-id")
        .unwrap_or_else(|_| "YANDI_FALLBACK_MACHINE_ID".to_string());
    format!("YANDI_MACHINE:{}", machine_id.trim()).into_bytes()
}

/// Hash a login password using Argon2id. Returns a PHC-format string. (On Unix this is the shared implementation in `key_root::login`, the
/// one `yandi-keys` and the second web page use, so that every door checks the same password the same way.)
#[cfg(unix)]
pub fn hash_login_password(password: &str) -> Result<String, String> {
    key_root::login::hash_login_password(password)
}

#[cfg(not(unix))]
pub fn hash_login_password(password: &str) -> Result<String, String> {
    use argon2::password_hash::{PasswordHasher, SaltString};
    let salt = SaltString::generate(&mut rand::rngs::OsRng);
    let argon2 = Argon2::default();
    argon2.hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("Password hash failed: {}", e))
}

/// Verify a login password against stored PHC hash.
#[cfg(unix)]
pub fn verify_login_password(password: &str, stored_hash: &str) -> bool {
    key_root::login::verify_login_password(password, stored_hash)
}

#[cfg(not(unix))]
pub fn verify_login_password(password: &str, stored_hash: &str) -> bool {
    use argon2::password_hash::{PasswordVerifier, PasswordHash};
    let Ok(hash) = PasswordHash::new(stored_hash) else { return false };
    Argon2::default().verify_password(password.as_bytes(), &hash).is_ok()
}

/// Encrypt master_key with a machine-derived key.
fn encrypt_master_key(master_key: &[u8; 32]) -> Result<EncryptedMasterKey, String> {
    let mut salt = [0u8; 32];
    let mut nonce_bytes = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);

    let machine_key = derive_key(&machine_passphrase(), &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&machine_key)
        .map_err(|e| format!("AES init: {}", e))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher.encrypt(nonce, master_key.as_slice())
        .map_err(|e| format!("AES encrypt: {}", e))?;

    Ok(EncryptedMasterKey {
        machine_salt: hex::encode(salt),
        nonce: hex::encode(nonce_bytes),
        ciphertext: hex::encode(ciphertext),
    })
}

/// Decrypt master_key using machine-derived key. Returns None if machine-id mismatch.
fn decrypt_master_key(enc: &EncryptedMasterKey) -> Option<[u8; 32]> {
    let salt_bytes = hex::decode(&enc.machine_salt).ok()?;
    let nonce_bytes = hex::decode(&enc.nonce).ok()?;
    let ciphertext = hex::decode(&enc.ciphertext).ok()?;

    let mut salt = [0u8; 32];
    let mut nonce_arr = [0u8; 12];
    if salt_bytes.len() != 32 || nonce_bytes.len() != 12 { return None; }
    salt.copy_from_slice(&salt_bytes);
    nonce_arr.copy_from_slice(&nonce_bytes);

    let machine_key = derive_key(&machine_passphrase(), &salt).ok()?;
    let cipher = Aes256Gcm::new_from_slice(&machine_key).ok()?;
    let nonce = Nonce::from_slice(&nonce_arr);
    let plaintext = cipher.decrypt(nonce, ciphertext.as_slice()).ok()?;
    if plaintext.len() != 32 { return None; }
    let mut key = [0u8; 32];
    key.copy_from_slice(&plaintext);
    Some(key)
}

// ── Auth file path ─────────────────────────────────────────────────────────

pub fn auth_file_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".yandi_keys")
        .join("auth.json")
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Try to load auth from disk and auto-decrypt master_key.
/// Returns the loaded AuthState (may be in setup/rebind mode).
#[cfg(unix)]
pub fn load_auth_state() -> AuthState {
    use key_root::{unlock_root, FileDeviceKey, KeyDir, KeyRootError, RootSource, SystemMachine};
    let state = AuthState::default();
    let path = auth_file_path();

    if !path.exists() {
        // First run — needs setup
        return state;
    }
    // An auth.json exists: from here on, "cannot open it" is never treated as "first run" (that would let setup overwrite it).
    let locked = |state: &AuthState| {
        state.is_setup.store(true, std::sync::atomic::Ordering::Relaxed);
        state.needs_rebind.store(true, std::sync::atomic::Ordering::Relaxed);
    };
    let dir = match path.parent().map(KeyDir::open) {
        Some(Ok(d)) => d,
        Some(Err(e)) => {
            println!("[auth] ❌ key directory is not safe to use ({}); nothing was changed", e.category());
            locked(&state);
            return state;
        }
        None => {
            locked(&state);
            return state;
        }
    };
    let device = FileDeviceKey::new(dir.file("device.key"));
    match unlock_root(&dir, &SystemMachine, &device) {
        Ok((root, source)) => {
            match source {
                RootSource::LegacyMachineId => {
                    println!("[auth] ✅ Master key loaded (machine verified)");
                    println!("[auth] ⚠️  legacy key format: the master key is protected only by the public machine id. Run `yandi-keys migrate` (see docs/KEY_RECOVERY.md)");
                }
                RootSource::Device => println!("[auth] ✅ Master key loaded (device key)"),
            }
            state.is_setup.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Ok(mut mk) = state.master_key.lock() {
                *mk = Some(*root);
            }
        }
        Err(KeyRootError::RecoveryRequired) => {
            println!("[auth] ⚠️ this device cannot open the key by itself — recovery is required: `yandi-keys recover` (nothing was changed)");
            locked(&state);
        }
        Err(e) => {
            println!("[auth] ❌ the key file cannot be used ({}); nothing was changed", e.category());
            locked(&state);
        }
    }
    state
}

#[cfg(not(unix))]
pub fn load_auth_state() -> AuthState {
    let state = AuthState::default();
    let path = auth_file_path();

    if !path.exists() {
        // First run — needs setup
        return state;
    }

    let json = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[auth] Failed to read auth.json: {}", e);
            return state;
        }
    };

    let stored: StoredAuth = match serde_json::from_str(&json) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[auth] Failed to parse auth.json: {}", e);
            return state;
        }
    };

    match decrypt_master_key(&stored.master_key_encrypted) {
        Some(master_key) => {
            println!("[auth] ✅ Master key loaded (machine verified)");
            state.is_setup.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Ok(mut mk) = state.master_key.lock() {
                *mk = Some(master_key);
            }
        }
        None => {
            println!("[auth] ⚠️ Machine-id mismatch — rebind required");
            state.is_setup.store(true, std::sync::atomic::Ordering::Relaxed);
            state.needs_rebind.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    state
}

/// Never overwrite existing key material: an auth.json that exists (even one this build cannot open) is somebody's master key.
#[cfg_attr(unix, allow(dead_code))] // on Unix the shared `key_root::login` does this; the tests and the Windows path still use it
fn ensure_no_existing_auth(path: &std::path::Path) -> Result<(), String> {
    if std::fs::symlink_metadata(path).is_ok() {
        return Err("Auth is already set up; refusing to overwrite the existing key file".to_string());
    }
    Ok(())
}

/// The web login hash of either auth.json format (1: machine-wrapped key, 2: device + recovery wrappers).
fn login_hash_of(json: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("Failed to parse auth.json: {}", e))?;
    value.get("login_hash").and_then(|v| v.as_str()).map(str::to_owned).ok_or_else(|| "auth.json has no login hash".to_string())
}

/// First-time setup, exactly as the person entered it in the web page: a login password and a master password, each typed twice.
/// The master password is the person's own recovery secret (it is not generated); the actual master key is a random root that this
/// creates, wrapped for THIS device (a device key file) and for the master password (Argon2id). Nothing is stored that could
/// open the root without the device key or the master password. Fails, changing nothing, if key material already exists.
pub fn setup_auth(
    state: &AuthState,
    login_password: &str,
    login_password_repeat: &str,
    master_password: &str,
    master_password_repeat: &str,
) -> Result<[u8; 32], String> {
    check_setup_inputs(login_password, login_password_repeat, master_password, master_password_repeat)?;
    #[cfg(unix)]
    {
        let path = auth_file_path();
        let dir = key_root::KeyDir::open(path.parent().ok_or("Каталог ключей недоступен")?.to_path_buf()).map_err(|_| "Каталог ключей недоступен".to_string())?;
        let device = key_root::FileDeviceKey::new(dir.file("device.key"));
        setup_auth_in(state, &dir, &key_root::SystemMachine, &device, login_password, master_password, key_root::KdfParams::RECOMMENDED, &key_root::KdfPolicy::production())
    }
    #[cfg(not(unix))]
    {
        setup_auth_legacy(state, login_password, master_password)
    }
}

/// The rules for what the person typed, in words that are safe to show (the shared rules of `key_root::login` on Unix).
#[cfg(unix)]
pub(crate) fn check_setup_inputs(login: &str, login_repeat: &str, master: &str, master_repeat: &str) -> Result<(), String> {
    key_root::login::check_setup_inputs(login, login_repeat, master, master_repeat)
}

#[cfg(not(unix))]
pub(crate) fn check_setup_inputs(login: &str, login_repeat: &str, master: &str, master_repeat: &str) -> Result<(), String> {
    if login.chars().count() < 8 {
        return Err("Пароль входа: минимум 8 символов".to_string());
    }
    if login != login_repeat {
        return Err("Пароли входа не совпадают".to_string());
    }
    if master.chars().count() < 12 {
        return Err("Мастер-пароль: минимум 12 символов (лучше фраза из нескольких слов)".to_string());
    }
    if master != master_repeat {
        return Err("Мастер-пароли не совпадают".to_string());
    }
    if master == login {
        return Err("Мастер-пароль должен отличаться от пароля входа".to_string());
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn setup_auth_in(
    state: &AuthState,
    dir: &key_root::KeyDir,
    machine: &dyn key_root::MachineContext,
    device: &dyn key_root::DeviceKeyProvider,
    login_password: &str,
    master_password: &str,
    params: key_root::KdfParams,
    policy: &key_root::KdfPolicy,
) -> Result<[u8; 32], String> {
    let root = key_root::login::create_keys(dir, machine, device, login_password, master_password, params, policy)?;
    state.is_setup.store(true, std::sync::atomic::Ordering::Relaxed);
    state.needs_rebind.store(false, std::sync::atomic::Ordering::Relaxed);
    if let Ok(mut mk) = state.master_key.lock() {
        *mk = Some(*root);
    }
    println!("[auth] ✅ Keys created (device key + master password); the master password is your recovery secret");
    Ok(*root)
}

#[cfg(not(unix))]
fn setup_auth_legacy(
    state: &AuthState,
    login_password: &str,
    master_password: &str,
) -> Result<[u8; 32], String> {
    if login_password.len() < 4 {
        return Err("Login password must be at least 4 characters".to_string());
    }
    if master_password.len() < 8 {
        return Err("Master password must be at least 8 characters".to_string());
    }

    ensure_no_existing_auth(&auth_file_path())?;

    // Derive master_key from master_password
    let mut master_salt = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut master_salt);
    let master_key = derive_key(master_password.as_bytes(), &master_salt)?;

    // Hash login password
    let login_hash = hash_login_password(login_password)?;

    // Encrypt master_key with machine-id
    let master_key_encrypted = encrypt_master_key(&master_key)?;

    let stored = StoredAuth {
        version: 1,
        login_hash,
        master_key_encrypted,
    };

    // Write to disk
    let path = auth_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create keys dir: {}", e))?;
    }
    let json = serde_json::to_string_pretty(&stored)
        .map_err(|e| format!("Serialization failed: {}", e))?;
    crate::util::private_file::write_private(&path, json.as_bytes())
        .map_err(|e| format!("Failed to write auth.json: {}", e))?;

    // Update in-memory state
    state.is_setup.store(true, std::sync::atomic::Ordering::Relaxed);
    state.needs_rebind.store(false, std::sync::atomic::Ordering::Relaxed);
    if let Ok(mut mk) = state.master_key.lock() {
        *mk = Some(master_key);
    }

    println!("[auth] ✅ Auth setup complete");
    Ok(master_key)
}

/// Verify login password against stored hash.
pub fn verify_login(login_password: &str) -> Result<bool, String> {
    let path = auth_file_path();
    if !path.exists() {
        return Err("Auth not set up".to_string());
    }
    let json = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read auth.json: {}", e))?;
    let login_hash = login_hash_of(&json)?;
    Ok(verify_login_password(login_password, &login_hash))
}

/// Re-bind master_key to a new machine (hardware migration).
/// User must provide master_password to prove ownership.
pub fn rebind_to_machine(
    _state: &AuthState,
    _master_password: &str,
) -> Result<(), String> {
    // The former implementation derived a NEW random master key from the password (its salt was never stored), which is not the old
    // key: everything encrypted under the old one became unreadable, and the password proved nothing. Recovery is now done by
    // `yandi-keys recover` with the recovery password of a migrated key directory (docs/KEY_RECOVERY.md); nothing is changed here.
    Err("Recovery is done on the command line: run `yandi-keys recover` (see docs/KEY_RECOVERY.md). Nothing was changed.".to_string())
}

/// Forgot the web password: the recovery code proves ownership and sets a new one. The recovery wrapper is opened (Argon2id: about half a
/// second per attempt, and the caller throttles attempts); only then is `auth.json` rewritten, atomically, with the same wrappers and the
/// new login hash. Every existing session is ended. Errors are short messages that are safe to show.
#[cfg(unix)]
pub fn recover_login(state: &AuthState, code_input: &str, new_login_password: &str) -> Result<(), String> {
    let path = auth_file_path();
    let dir = key_root::KeyDir::open(path.parent().ok_or("Файл ключей недоступен")?.to_path_buf()).map_err(|_| "Каталог ключей недоступен".to_string())?;
    recover_login_in(state, &dir, code_input, new_login_password, &key_root::KdfPolicy::production())
}

#[cfg(unix)]
pub(crate) fn recover_login_in(
    state: &AuthState,
    dir: &key_root::KeyDir,
    code_input: &str,
    new_login_password: &str,
    policy: &key_root::KdfPolicy,
) -> Result<(), String> {
    key_root::login::reset_login(dir, code_input, new_login_password, policy)?;
    if let Ok(mut sessions) = state.sessions.lock() {
        sessions.clear(); // a password reset ends every session that existed before it
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn recover_login(_state: &AuthState, _code_input: &str, _new_login_password: &str) -> Result<(), String> {
    Err("Восстановление по коду пока поддерживается только на Linux и macOS".to_string())
}

/// Extract session token from Cookie header value.
pub fn extract_session_token(cookie_header: &str) -> Option<String> {
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(val) = part.strip_prefix(&format!("{}=", SESSION_COOKIE)) {
            return Some(val.to_string());
        }
    }
    None
}

/// Build a Set-Cookie header value for the session token.
pub fn make_session_cookie(token: &str, remember_me: bool) -> String {
    if remember_me {
        format!(
            "{}={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
            SESSION_COOKIE, token, SESSION_REMEMBER_SECS
        )
    } else {
        format!(
            "{}={}; HttpOnly; SameSite=Strict; Path=/",
            SESSION_COOKIE, token
        )
    }
}

/// Build a Set-Cookie header that clears the session.
pub fn clear_session_cookie() -> String {
    format!(
        "{}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
        SESSION_COOKIE
    )
}

#[cfg(test)]
mod login_throttle_tests {
    use super::*;

    /// Live exploit + regression test: the web login endpoint had no rate
    /// limiting at all — combined with a real 0.0.0.0 default bind and a
    /// login password allowed to be just 4 characters, this was an
    /// unlimited-speed online brute-force surface. Proves the schedule is
    /// free for a few genuine typos, then actually slows things down, and
    /// that a real success clears it — the legitimate owner is never
    /// permanently locked out.
    #[test]
    fn backoff_schedule_is_free_then_grows_then_caps() {
        assert_eq!(LoginAttempts::backoff_after(0), Duration::ZERO);
        assert_eq!(LoginAttempts::backoff_after(LOGIN_FREE_ATTEMPTS), Duration::ZERO, "free attempts must not be throttled");
        assert_eq!(LoginAttempts::backoff_after(LOGIN_FREE_ATTEMPTS + 1), Duration::from_secs(2));
        assert_eq!(LoginAttempts::backoff_after(LOGIN_FREE_ATTEMPTS + 2), Duration::from_secs(4));
        assert_eq!(LoginAttempts::backoff_after(LOGIN_FREE_ATTEMPTS + 3), Duration::from_secs(8));
        // Must cap, not overflow or grow unbounded, under a very long attack.
        assert_eq!(LoginAttempts::backoff_after(LOGIN_FREE_ATTEMPTS + 30), Duration::from_secs(LOGIN_BACKOFF_MAX_SECS));
    }

    #[test]
    fn live_brute_force_is_throttled_but_real_success_recovers_immediately() {
        let state = AuthState::default();

        // A few genuine mistakes (e.g. real typos) must never be throttled.
        assert!(state.login_backoff_remaining().is_none());
        for _ in 0..LOGIN_FREE_ATTEMPTS {
            state.record_login_failure();
            assert!(state.login_backoff_remaining().is_none(), "free attempts must not be throttled");
        }

        // The next failure (an attacker guessing fast) must now be throttled.
        state.record_login_failure();
        let remaining = state.login_backoff_remaining();
        assert!(remaining.is_some(), "REPLAY/BRUTE-FORCE SUCCEEDED: no throttle after repeated failures");
        assert!(remaining.unwrap() <= Duration::from_secs(2));

        // A real, correct password must be accepted immediately by the
        // password check itself (the caller is expected to check
        // login_backoff_remaining() BEFORE calling verify_login — this
        // only proves record_login_success() fully clears the throttle
        // once that real success happens).
        state.record_login_success();
        assert!(state.login_backoff_remaining().is_none(), "a real success must fully clear the throttle, not just pause it");

        // Attacking again afterward must still work from a clean slate —
        // success must not have disabled throttling for future abuse.
        for _ in 0..LOGIN_FREE_ATTEMPTS {
            state.record_login_failure();
        }
        state.record_login_failure();
        assert!(state.login_backoff_remaining().is_some(), "throttle must re-arm after a fresh run of failures post-success");
    }

    #[test]
    fn backoff_actually_expires_after_waiting() {
        let state = AuthState::default();
        for _ in 0..=LOGIN_FREE_ATTEMPTS {
            state.record_login_failure();
        }
        assert!(state.login_backoff_remaining().is_some());
        std::thread::sleep(Duration::from_secs(2) + Duration::from_millis(100));
        assert!(state.login_backoff_remaining().is_none(), "backoff must actually expire, not block forever");
    }
}

#[cfg(test)]
mod key_root_integration_tests {
    use super::*;

    #[test]
    fn setup_never_overwrites_an_existing_auth_file() {
        let tmp = std::env::temp_dir().join(format!("yandi-auth-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let existing = tmp.join("auth.json");
        std::fs::write(&existing, "anything, even something unreadable").unwrap();
        assert!(ensure_no_existing_auth(&existing).is_err());
        assert!(ensure_no_existing_auth(&tmp.join("absent.json")).is_ok());
        let link = tmp.join("link.json");
        let _ = std::fs::remove_file(&link);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(tmp.join("nowhere"), &link).unwrap();
            assert!(ensure_no_existing_auth(&link).is_err(), "a dangling link is not 'absent'");
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn rebind_no_longer_makes_a_different_master_key() {
        let state = AuthState::default();
        assert!(rebind_to_machine(&state, "any password at all").is_err());
        assert!(state.get_master_key().is_none(), "rebind must not install a key");
        assert!(!state.is_setup.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn the_login_hash_is_read_from_both_auth_formats() {
        assert_eq!(login_hash_of(r#"{"version":1,"login_hash":"h1","master_key_encrypted":{}}"#).unwrap(), "h1");
        assert_eq!(login_hash_of(r#"{"version":2,"login_hash":"h2","root_id":"x"}"#).unwrap(), "h2");
        assert!(login_hash_of(r#"{"version":2}"#).is_err());
        assert!(login_hash_of("not json").is_err());
    }

    #[cfg(unix)]
    mod recover {
        use super::*;
        use key_root::{FixedMachine, KdfParams, KdfPolicy, KeyDir, RecoveryCode, RootDocument};

        const FAST: KdfParams = KdfParams { memory_kib: 64, iterations: 1, parallelism: 1 };

        fn store(code: &RecoveryCode) -> (std::path::PathBuf, KeyDir) {
            let base = std::env::temp_dir().join(format!("yandi-recover-test-{}-{}", std::process::id(), rand::random::<u32>()));
            let dir = KeyDir::open(base.join("keys")).unwrap();
            let old_hash = hash_login_password("the old login password").unwrap();
            let doc = RootDocument::create(&[7u8; 32], &old_hash, &[9u8; 32], "file-v1", &FixedMachine("m".into()), code.secret(), FAST, &KdfPolicy::for_tests()).unwrap();
            key_root::atomic::create_new_file(&dir.auth_file(), &doc.to_json()).unwrap();
            (base, dir)
        }

        fn state_with_a_session() -> (AuthState, String) {
            let st = AuthState::default();
            let token = st.create_session(false);
            (st, token)
        }

        #[test]
        fn the_recovery_code_resets_the_login_password_and_ends_old_sessions() {
            let code = RecoveryCode::generate();
            let (base, dir) = store(&code);
            let (st, token) = state_with_a_session();
            assert!(st.verify_session(&token));
            recover_login_in(&st, &dir, &code.display().to_lowercase().replace('-', " "), "a brand new password", &KdfPolicy::for_tests()).unwrap();
            let doc = RootDocument::parse(&std::fs::read(dir.auth_file()).unwrap()).unwrap();
            assert!(verify_login_password("a brand new password", &doc.login_hash));
            assert!(!verify_login_password("the old login password", &doc.login_hash));
            assert!(!st.verify_session(&token), "an old session survived a password reset");
            // the keys themselves are untouched: the same code still opens the same root, the device still opens it too
            assert_eq!(*doc.unlock_with_password(code.secret(), &KdfPolicy::for_tests()).unwrap(), [7u8; 32]);
            assert_eq!(*doc.unlock_with_device(&[9u8; 32], &FixedMachine("m".into())).unwrap(), [7u8; 32]);
            let _ = std::fs::remove_dir_all(base);
        }

        #[test]
        fn a_wrong_code_a_typo_a_weak_password_and_a_legacy_directory_change_nothing() {
            let code = RecoveryCode::generate();
            let (base, dir) = store(&code);
            let before = std::fs::read(dir.auth_file()).unwrap();
            let (st, token) = state_with_a_session();
            let pol = KdfPolicy::for_tests();
            let other = RecoveryCode::generate().display();
            assert_eq!(recover_login_in(&st, &dir, &other, "a brand new password", &pol).unwrap_err(), "Код восстановления не подошёл");
            let mut typo = code.display();
            typo.replace_range(0..1, if typo.starts_with('A') { "B" } else { "A" });
            assert!(recover_login_in(&st, &dir, &typo, "a brand new password", &pol).unwrap_err().contains("опечатка"));
            assert!(recover_login_in(&st, &dir, &code.display(), "short", &pol).is_err());
            assert!(recover_login_in(&st, &dir, "", "a brand new password", &pol).is_err());
            assert_eq!(std::fs::read(dir.auth_file()).unwrap(), before, "a refused reset changed auth.json");
            assert!(st.verify_session(&token), "a refused reset ended a session");
            // a legacy (v1) directory has no recovery wrapper
            let legacy = std::env::temp_dir().join(format!("yandi-recover-legacy-{}-{}", std::process::id(), rand::random::<u32>()));
            let ldir = KeyDir::open(legacy.join("keys")).unwrap();
            let v1 = key_root::legacy::seal_legacy_auth(&[7u8; 32], "$h", &FixedMachine("m".into())).unwrap();
            key_root::atomic::create_new_file(&ldir.auth_file(), &v1).unwrap();
            assert!(recover_login_in(&st, &ldir, &code.display(), "a brand new password", &pol).unwrap_err().contains("migrate"));
            assert_eq!(std::fs::read(ldir.auth_file()).unwrap(), v1);
            let _ = std::fs::remove_dir_all(base);
            let _ = std::fs::remove_dir_all(legacy);
        }
    }

    #[cfg(unix)]
    mod first_setup_flow {
        use super::*;
        use crate::core::identity::NodeIdentity;
        use key_root::{
            unlock_root, FileDeviceKey, FixedMachine, IdentityFormat, KdfParams, KdfPolicy, KeyDir, KeyRootError, OpenContext, Recovery, RootDocument, RootSource,
        };
        use std::os::unix::fs::PermissionsExt;

        const FAST: KdfParams = KdfParams { memory_kib: 64, iterations: 1, parallelism: 1 };
        const LOGIN: &str = "my login password";
        const MASTER: &str = "my own master phrase, written on paper";

        fn fresh() -> (std::path::PathBuf, KeyDir) {
            let base = std::env::temp_dir().join(format!("yandi-setup-test-{}-{}", std::process::id(), rand::random::<u32>()));
            let dir = KeyDir::open(base.join("keys")).unwrap();
            (base, dir)
        }

        fn setup(dir: &KeyDir, machine: &FixedMachine, state: &AuthState) -> Result<[u8; 32], String> {
            let device = FileDeviceKey::new(dir.file("device.key"));
            setup_auth_in(state, dir, machine, &device, LOGIN, MASTER, FAST, &KdfPolicy::for_tests())
        }

        #[test]
        fn what_the_person_typed_is_the_recovery_secret_and_the_device_opens_the_key_by_itself() {
            let (base, dir) = fresh();
            let m = FixedMachine("machine-A".into());
            let state = AuthState::default();
            let root = setup(&dir, &m, &state).unwrap();
            assert_eq!(state.get_master_key(), Some(root));
            let bytes = std::fs::read(dir.auth_file()).unwrap();
            let doc = RootDocument::parse(&bytes).unwrap();
            let device = FileDeviceKey::new(dir.file("device.key"));
            // normal start: no password
            let (by_device, source) = unlock_root(&dir, &m, &device).unwrap();
            assert_eq!((*by_device, source), (root, RootSource::Device));
            // the master password exactly as typed opens it; a near miss does not
            assert_eq!(*doc.unlock_with_password(MASTER, &KdfPolicy::for_tests()).unwrap(), root);
            assert_eq!(doc.unlock_with_password("my own master phrase, written on paper.", &KdfPolicy::for_tests()).unwrap_err(), KeyRootError::RecoveryFailed);
            assert!(verify_login_password(LOGIN, &doc.login_hash) && !verify_login_password(MASTER, &doc.login_hash));
            // nothing readable: neither password, nor the root, nor the device key is in any file
            let device_key = std::fs::read(dir.file("device.key")).unwrap();
            for name in ["auth.json", "device.key"] {
                let content = std::fs::read(dir.file(name)).unwrap();
                for secret in [MASTER.as_bytes().to_vec(), LOGIN.as_bytes().to_vec(), root.to_vec(), hex::encode(root).into_bytes()] {
                    assert!(!content.windows(secret.len()).any(|w| w == secret.as_slice()), "{name} contains a secret");
                }
            }
            assert!(!std::fs::read(dir.auth_file()).unwrap().windows(32).any(|w| w == device_key.as_slice()));
            for name in ["auth.json", "device.key"] {
                assert_eq!(std::fs::metadata(dir.file(name)).unwrap().permissions().mode() & 0o777, 0o600);
            }
            let _ = std::fs::remove_dir_all(base);
        }

        #[test]
        fn setup_never_overwrites_and_refuses_what_the_person_typed_wrongly() {
            let (base, dir) = fresh();
            let m = FixedMachine("machine-A".into());
            let state = AuthState::default();
            setup(&dir, &m, &state).unwrap();
            let before = (std::fs::read(dir.auth_file()).unwrap(), std::fs::read(dir.file("device.key")).unwrap());
            assert!(setup(&dir, &m, &AuthState::default()).is_err(), "a second setup must not overwrite the keys");
            assert_eq!((std::fs::read(dir.auth_file()).unwrap(), std::fs::read(dir.file("device.key")).unwrap()), before);
            // the rules on the typed text
            assert!(check_setup_inputs("short", "short", MASTER, MASTER).unwrap_err().contains("минимум 8"));
            assert!(check_setup_inputs(LOGIN, "another", MASTER, MASTER).unwrap_err().contains("Пароли входа не совпадают"));
            assert!(check_setup_inputs(LOGIN, LOGIN, "eleven char", "eleven char").unwrap_err().contains("минимум 12"));
            assert!(check_setup_inputs(LOGIN, LOGIN, MASTER, "another master phrase").unwrap_err().contains("Мастер-пароли не совпадают"));
            assert!(check_setup_inputs(LOGIN, LOGIN, LOGIN, LOGIN).unwrap_err().contains("отличаться"));
            assert!(check_setup_inputs(LOGIN, LOGIN, MASTER, MASTER).is_ok());
            // a fresh directory where a stray device key already lies: refused, nothing is written
            let (base2, dir2) = fresh();
            std::fs::write(dir2.file("device.key"), [1u8; 32]).unwrap();
            std::fs::set_permissions(dir2.file("device.key"), std::fs::Permissions::from_mode(0o600)).unwrap();
            assert!(setup(&dir2, &m, &AuthState::default()).is_err());
            assert!(!dir2.auth_file().exists());
            let _ = std::fs::remove_dir_all(base);
            let _ = std::fs::remove_dir_all(base2);
        }

        #[test]
        fn a_fresh_install_gets_a_recoverable_identity_and_the_typed_master_password_brings_it_back() {
            let (base, dir) = fresh();
            let m = FixedMachine("machine-A".into());
            let state = AuthState::default();
            let root = setup(&dir, &m, &state).unwrap();
            // the identity is created UNDER the new keys (format v3), at first start
            let id = NodeIdentity::load_or_create_in(&dir, &m, None, 9000, Some(&root)).unwrap();
            let file = std::fs::read(dir.identity_file(9000)).unwrap();
            assert_eq!(key_root::identity_store::identity_format(&file).unwrap(), IdentityFormat::RootV3);
            // every later start: the device opens the key, the same identity loads, no password
            let device = FileDeviceKey::new(dir.file("device.key"));
            let (root2, _) = unlock_root(&dir, &m, &device).unwrap();
            let again = NodeIdentity::load_or_create_in(&dir, &m, None, 9000, Some(&root2)).unwrap();
            assert_eq!(again.node_id().0, id.node_id().0);
            // ANOTHER MACHINE, the device key gone: only the two files and the master password the person typed
            let other = std::env::temp_dir().join(format!("yandi-setup-other-{}-{}", std::process::id(), rand::random::<u32>()));
            let odir = KeyDir::open(other.join("keys")).unwrap();
            for name in ["auth.json", "node_identity_9000.json"] {
                key_root::atomic::create_new_file(&odir.file(name), &std::fs::read(dir.file(name)).unwrap()).unwrap();
            }
            let mb = FixedMachine("machine-B".into());
            let device_b = FileDeviceKey::new(odir.file("device.key"));
            assert_eq!(unlock_root(&odir, &mb, &device_b).err().unwrap(), KeyRootError::RecoveryRequired);
            let pol = KdfPolicy::for_tests();
            let wrong = Recovery { dir: &odir, port: 9000, machine: &mb, device: &device_b, password: "not the master password", policy: &pol }.run();
            assert_eq!(wrong.err().unwrap(), KeyRootError::RecoveryFailed);
            Recovery { dir: &odir, port: 9000, machine: &mb, device: &device_b, password: MASTER, policy: &pol }.run().unwrap();
            let (root3, _) = unlock_root(&odir, &mb, &device_b).unwrap();
            let back = NodeIdentity::load_or_create_in(&odir, &mb, None, 9000, Some(&root3)).unwrap();
            assert_eq!(back.node_id().0, id.node_id().0, "the recovered node id is not the same");
            // and the login page: forgot the login password → the master password (as typed) sets a new one
            recover_login_in(&AuthState::default(), &odir, MASTER, "a new login password", &pol).unwrap();
            let doc = RootDocument::parse(&std::fs::read(odir.auth_file()).unwrap()).unwrap();
            assert!(verify_login_password("a new login password", &doc.login_hash));
            let _ = OpenContext { machine: &mb, env_password: None, root: None };
            let _ = std::fs::remove_dir_all(base);
            let _ = std::fs::remove_dir_all(other);
        }
    }
}
