//! Компьютер как собеседник телефонов владельца: переписка «телефон ↔ этот узел» (веб-чат на странице узла).
//!
//! У узла своя пара ключей для телефонов (x25519 для шифрования, ed25519 для подписи связки), она лежит в `mobile_self.json` в папке данных
//! (права 0600). Телефон видит компьютер в контактах под номером узла, берёт его подписанную связку из `/mobile/pubkey/<номер узла>` и
//! шифрует, как для другого телефона (подписанный конверт E4, `e2e_crypto.dart`). Узел здесь — конечная точка переписки: он расшифровывает
//! сообщение и кладёт его в историю чата, которую показывает страница. Ответ со страницы узел шифрует открытым ключом телефона
//! (из подписанной связки, которую телефон зарегистрировал) и кладёт в очередь телефона. Открытым текстом между телефоном и узлом
//! ничего не ходит, кроме TLS.
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, MutexGuard, OnceLock};
use zeroize::Zeroizing;
use x25519_dalek::{PublicKey, StaticSecret};

/// E4 header + AES-GCM ciphertext/tag + Ed25519 signature; see docs/CLIENT_WIRE.md.
const MAGIC: u8 = 0xE4;
const AAD_PREFIX: &[u8] = b"YANDI-E2E-E4\0";
const SIGN_DOMAIN: &[u8] = b"YANDI-E2E-E4-SIGN\0";
const HEADER: usize = 134;
const REPLAY_TTL_MS: u64 = 30 * 24 * 3600 * 1000;
const BUNDLE_DOMAIN: &[u8] = b"YANDI-MOBILE-KEYS-V1\0";

#[derive(Serialize, Deserialize)]
struct Stored {
    x25519_secret: String,
    ed25519_secret: String,
}

struct Keys {
    x_secret: StaticSecret,
    x_pub: [u8; 32],
    ed_pub: [u8; 32],
    sig: [u8; 64],
    signing: ed25519_dalek::SigningKey,
}

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn unb64(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

#[derive(Default)]
struct KeyHolder {
    master: Option<Zeroizing<[u8; 32]>>,
    keys: Option<Keys>,
    /// узел без мастер-ключа и без закрытой среды (первый запуск): прежний открытый файл ключей, как до закрытой среды
    plain_mode: bool,
}

impl KeyHolder {
    fn revoke(&mut self) {
        // X25519 StaticSecret и Ed25519 SigningKey стирают секреты при Drop.
        self.keys = None;
        self.master = None;
        self.plain_mode = false;
    }
    fn replace(&mut self, master: Zeroizing<[u8; 32]>) {
        self.revoke();
        self.master = Some(master);
    }
}

fn key_cell() -> &'static Mutex<KeyHolder> {
    // OnceLock хранит только адрес изменяемого держателя, не ключ или сеанс.
    static CELL: OnceLock<Mutex<KeyHolder>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(KeyHolder::default()))
}

struct KeyGuard<'a>(MutexGuard<'a, KeyHolder>);
impl std::ops::Deref for KeyGuard<'_> {
    type Target = Keys;
    fn deref(&self) -> &Keys { self.0.keys.as_ref().expect("ключи проверены под guard") }
}

fn keys_from(holder: &Mutex<KeyHolder>, load: impl FnOnce(Option<&[u8; 32]>) -> Keys) -> Option<KeyGuard<'_>> {
    let mut state = match holder.lock() {
        Ok(state) => state,
        Err(error) => { error.into_inner().revoke(); return None; }
    };
    if state.keys.is_none() {
        // запертый держатель не читает и не создаёт открытый файл; без мастер-ключа — только в явном plain_mode
        let master = match state.master.as_deref() {
            Some(m) => Some(m),
            None if state.plain_mode => None,
            None => return None,
        };
        let loaded = load(master);
        state.keys = Some(loaded);
    }
    Some(KeyGuard(state))
}

fn keys() -> Option<KeyGuard<'static>> {
    keys_from(key_cell(), |master| load_protected(&crate::util::data_dir::data_dir(), master))
}

/// Перенос ключей «компьютера как собеседника» на ключ закрытой среды: из файла под прежним мастер-ключом или из открытого
/// `mobile_self.json`. Идемпотентно. Прежнее содержимое заменяется только после проверки записанного; открытый файл затирается.
pub fn rekey(dir: &std::path::Path, old: Option<&[u8; 32]>, new: &[u8; 32]) -> Result<(), String> {
    let enc = dir.join("mobile_self.enc");
    let plain = dir.join("mobile_self.json");
    let open_with = |master: &[u8; 32]| -> Option<([u8; 32], [u8; 32])> {
        let raw = std::fs::read(&enc).ok()?;
        if raw.len() < 28 {
            return None;
        }
        let key = protect_key(master);
        let body = Aes256Gcm::new_from_slice(&key).ok()?.decrypt(Nonce::from_slice(&raw[..12]), Payload { msg: &raw[12..], aad: b"mobile_self" }).ok()?;
        let st: Stored = serde_json::from_slice(&body).ok()?;
        Some((unb64(&st.x25519_secret)?.try_into().ok()?, unb64(&st.ed25519_secret)?.try_into().ok()?))
    };
    let from_plain = || {
        std::fs::read_to_string(&plain)
            .ok()
            .and_then(|t| serde_json::from_str::<Stored>(&t).ok())
            .and_then(|s| Some((<[u8; 32]>::try_from(unb64(&s.x25519_secret)?).ok()?, <[u8; 32]>::try_from(unb64(&s.ed25519_secret)?).ok()?)))
    };
    let wipe_plain = || {
        if let Ok(meta) = std::fs::metadata(&plain) {
            let _ = std::fs::write(&plain, vec![0u8; meta.len() as usize]);
        }
        let _ = std::fs::remove_file(&plain);
    };
    if open_with(new).is_some() {
        if plain.exists() {
            wipe_plain();
        }
        return Ok(());
    }
    let pair = old.and_then(|o| open_with(o)).or_else(from_plain);
    let Some((x, e)) = pair else {
        return if enc.exists() || plain.exists() { Err("ключи компьютера не открываются ни прежним, ни новым ключом; файлы не тронуты".into()) } else { Ok(()) };
    };
    let body = serde_json::to_vec(&Stored { x25519_secret: b64(&x), ed25519_secret: b64(&e) }).map_err(|e| e.to_string())?;
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let key = protect_key(new);
    let ct = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| e.to_string())?
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: &body, aad: b"mobile_self" })
        .map_err(|e| e.to_string())?;
    let blob = [nonce.as_slice(), &ct].concat();
    crate::util::private_file::write_private(&enc, &blob).map_err(|e| e.to_string())?;
    if open_with(new) != Some((x, e)) {
        return Err("проверка записанных ключей компьютера не прошла".into());
    }
    if plain.exists() {
        wipe_plain();
    }
    Ok(())
}

/// Отзыв ждёт завершения операции, удерживающей guard; после возврата старых
/// signing/X25519 ключей в держателе нет и ленивое чтение не может их воскресить.
pub fn lock_keys() {
    revoke_holder(key_cell());
}

fn revoke_holder(holder: &Mutex<KeyHolder>) {
    // Panic предыдущей операции не должен препятствовать стиранию секретов.
    holder.lock().unwrap_or_else(|e| e.into_inner()).revoke();
}

/// Узел без мастер-ключа и без закрытой среды: ключи в прежнем открытом файле (как до закрытой среды). Отзыв отменяет режим.
pub fn allow_plain_keys() {
    match key_cell().lock() {
        Ok(mut state) => {
            if state.master.is_none() {
                state.plain_mode = true;
            }
        }
        Err(e) => e.into_inner().revoke(),
    }
}

/// Переходный legacy-вход main. Парольный controller должен устанавливать сеанс
/// только после проверки поколения/миграции; автоматическая загрузка root в main
/// пока остаётся отдельной незавершённой интеграцией, а не защитой новой среды.
pub fn set_master(mk: [u8; 32]) {
    let master = Zeroizing::new(mk);
    match key_cell().lock() {
        Ok(mut state) => state.replace(master),
        Err(e) => e.into_inner().revoke(), // после panic остаёмся запертыми
    }
}

fn protect_key(master: &[u8; 32]) -> [u8; 32] {
    let mut k = [0u8; 32];
    hkdf::Hkdf::<sha2::Sha256>::new(Some(b"yandi-mobile-self-file:v1"), master).expand(b"identity", &mut k).expect("hkdf");
    k
}

/// Ключи компьютера: при наличии мастер-ключа — в `mobile_self.enc` (AES-256-GCM); старый открытый `mobile_self.json` переносится
/// в шифрованный и затирается. Повреждённый или не открывающийся файл не трогаем (временная пара, как и раньше).
fn load_protected(dir: &std::path::Path, master: Option<&[u8; 32]>) -> Keys {
    let plain = dir.join("mobile_self.json");
    let Some(master) = master else { return load_or_create(&plain) };
    let key = protect_key(master);
    let enc = dir.join("mobile_self.enc");
    let seal = |x: &[u8; 32], e: &[u8; 32]| -> Option<Vec<u8>> {
        let body = serde_json::to_vec(&Stored { x25519_secret: b64(x), ed25519_secret: b64(e) }).ok()?;
        let mut nonce = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ct = Aes256Gcm::new_from_slice(&key).ok()?.encrypt(Nonce::from_slice(&nonce), Payload { msg: &body, aad: b"mobile_self" }).ok()?;
        Some([nonce.as_slice(), &ct].concat())
    };
    let open = |raw: &[u8]| -> Option<([u8; 32], [u8; 32])> {
        if raw.len() < 28 {
            return None;
        }
        let body = Aes256Gcm::new_from_slice(&key).ok()?.decrypt(Nonce::from_slice(&raw[..12]), Payload { msg: &raw[12..], aad: b"mobile_self" }).ok()?;
        let st: Stored = serde_json::from_slice(&body).ok()?;
        Some((unb64(&st.x25519_secret)?.try_into().ok()?, unb64(&st.ed25519_secret)?.try_into().ok()?))
    };
    let wipe = |path: &std::path::Path| {
        if let Ok(meta) = std::fs::metadata(path) {
            let _ = std::fs::write(path, vec![0u8; meta.len() as usize]);
        }
        std::fs::remove_file(path)
    };
    let plain_pair = || {
        std::fs::read_to_string(&plain)
            .ok()
            .and_then(|t| serde_json::from_str::<Stored>(&t).ok())
            .and_then(|s| Some((<[u8; 32]>::try_from(unb64(&s.x25519_secret)?).ok()?, <[u8; 32]>::try_from(unb64(&s.ed25519_secret)?).ok()?)))
    };
    match std::fs::read(&enc) {
        Ok(raw) => match open(&raw) {
            Some((x, e)) => {
                // перенос мог оборваться между записью шифрованного файла и удалением открытого: доубираем открытый, если в нём та же пара
                if plain.exists() {
                    if plain_pair() == Some((x, e)) {
                        if let Err(err) = wipe(&plain) {
                            eprintln!("[mobile-self] ⚠️  открытый файл ключей {} не удалён ({err}): удалите его вручную", plain.display());
                        }
                    } else {
                        eprintln!("[mobile-self] ⚠️  рядом с шифрованным лежит открытый файл ключей {} с другой парой; он не тронут, разберитесь вручную", plain.display());
                    }
                }
                return make_keys(x, e);
            }
            None => {
                eprintln!("[mobile-self] ⚠️  файл ключей {} не открывается (повреждён или другой мастер-ключ); он не тронут, используется временная пара", enc.display());
                let mut x = [0u8; 32];
                let mut e = [0u8; 32];
                rand::thread_rng().fill_bytes(&mut x);
                rand::thread_rng().fill_bytes(&mut e);
                return make_keys(x, e);
            }
        },
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            // файл есть, но прочесть его нельзя: заменять нечем и нельзя — новая пара не записывается поверх (прежняя личность
            // иначе была бы потеряна безвозвратно); работает временная пара, которая нигде не сохраняется
            eprintln!("[mobile-self] ⚠️  файл ключей {} не читается ({err}); используется временная пара, файл не тронут", enc.display());
            let mut x = [0u8; 32];
            let mut e = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut x);
            rand::thread_rng().fill_bytes(&mut e);
            return make_keys(x, e);
        }
        Err(_) => {}
    }
    // шифрованного файла нет: берём (или создаём) прежний открытый и переносим
    let had_plain = plain.exists();
    let k = load_or_create(&plain);
    let moved = plain_pair().is_some_and(|(x, e)| {
        seal(&x, &e).is_some_and(|blob| crate::util::private_file::write_private(&enc, &blob).is_ok())
            && std::fs::read(&enc).ok().and_then(|r| open(&r)) == Some((x, e))
    });
    if moved {
        // перенесено и проверено: старый открытый файл затираем и удаляем
        match wipe(&plain) {
            Ok(()) if had_plain => eprintln!("[mobile-self] ключи компьютера перенесены в шифрованный файл {}", enc.display()),
            Ok(()) => {}
            Err(err) => eprintln!("[mobile-self] ⚠️  открытый файл ключей {} не удалён ({err}): удалите его вручную", plain.display()),
        }
    } else if plain.exists() {
        // открытую копию не удаляем, пока нет проверенной шифрованной: иначе пропадёт единственная
        eprintln!("[mobile-self] ⚠️  ключи компьютера НЕ перенесены в шифрованный файл {} (нет прав на запись?); пока лежат открыто в {}", enc.display(), plain.display());
    }
    k
}

fn load_or_create(path: &std::path::Path) -> Keys {
    let random_pair = || {
        let mut x = [0u8; 32];
        let mut e = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut x);
        rand::thread_rng().fill_bytes(&mut e);
        (x, e)
    };
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let parsed: Option<([u8; 32], [u8; 32])> = serde_json::from_str::<Stored>(&text)
                .ok()
                .and_then(|s| Some((unb64(&s.x25519_secret)?.try_into().ok()?, unb64(&s.ed25519_secret)?.try_into().ok()?)));
            match parsed {
                Some((x, e)) => make_keys(x, e),
                None => {
                    // повреждённый файл ключей НЕ перезаписываем: его можно восстановить. Работаем на временной паре (телефоны увидят
                    // смену ключа и откажутся от связи — это честнее, чем молча подменить личность компьютера навсегда).
                    eprintln!("[mobile-self] ⚠️  файл ключей {} повреждён; он не тронут, используется временная пара до восстановления (или удалите файл, чтобы создать новую)", path.display());
                    let (x, e) = random_pair();
                    make_keys(x, e)
                }
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let (x, e) = random_pair();
            match serde_json::to_vec(&Stored { x25519_secret: b64(&x), ed25519_secret: b64(&e) }) {
                Ok(s) => {
                    if let Err(err) = crate::util::private_file::write_private(path, &s) {
                        eprintln!("[mobile-self] ⚠️  не удалось сохранить новые ключи ({err}); после перезапуска они сменятся");
                    }
                }
                Err(err) => eprintln!("[mobile-self] ⚠️  не удалось подготовить ключи к записи: {err}"),
            }
            make_keys(x, e)
        }
        Err(err) => {
            eprintln!("[mobile-self] ⚠️  файл ключей {} не читается ({err}); он не тронут, используется временная пара", path.display());
            let (x, e) = random_pair();
            make_keys(x, e)
        }
    }
}

fn make_keys(x: [u8; 32], e: [u8; 32]) -> Keys {
    use ed25519_dalek::{Signer, SigningKey};
    let x_secret = StaticSecret::from(x);
    let x_pub = PublicKey::from(&x_secret).to_bytes();
    let signing = SigningKey::from_bytes(&e);
    let ed_pub = signing.verifying_key().to_bytes();
    let sig = signing.sign(&[BUNDLE_DOMAIN, &ed_pub, &x_pub].concat()).to_bytes();
    Keys { x_secret, x_pub, ed_pub, sig, signing }
}

/// Подпись ключом «компьютера как собеседника» (тот же, что в связке для телефонов): так ПК доказывает контактам, что это он.
/// `None` — закрытая среда заперта.
pub fn sign_identity(message: &[u8]) -> Option<Vec<u8>> {
    use ed25519_dalek::Signer;
    let k = keys()?;
    Some(k.signing.sign(message).to_bytes().to_vec())
}

/// Подписанная связка ключей узла для `/mobile/pubkey/<номер узла>` (тот же вид, что у телефонов).
pub fn bundle_json() -> serde_json::Value {
    let public_path = crate::util::data_dir::data_dir().join("mobile_self_public.json");
    let Some(k) = keys() else {
        // среда заперта: открытая часть связки (только открытые ключи и подпись) лежит отдельно, чтобы телефоны могли писать ПК
        // и пока владельца нет у экрана — сообщение зашифровано ключом ПК и дождётся входа
        return std::fs::read_to_string(&public_path)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .filter(|v| v.get("x25519_pub").is_some())
            .unwrap_or_else(|| serde_json::json!({"error":"vault_locked"}));
    };
    let v = serde_json::json!({"x25519_pub": b64(&k.x_pub), "ed25519_pub": b64(&k.ed_pub), "signature": b64(&k.sig)});
    if let Ok(text) = serde_json::to_string(&v) {
        if std::fs::read_to_string(&public_path).ok().as_deref() != Some(text.as_str()) {
            let _ = crate::util::private_file::write_private(&public_path, text.as_bytes());
        }
    }
    v
}

fn aad(header: &[u8]) -> Vec<u8> {
    [AAD_PREFIX, header].concat()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn derived_key(shared: &[u8], header: &[u8]) -> Option<[u8; 32]> {
    use sha2::{Digest, Sha256};
    if shared.iter().all(|b| *b == 0) { return None; }
    let mut key = [0; 32];
    hkdf::Hkdf::<Sha256>::new(Some(&Sha256::digest(aad(header))), shared)
        .expand(b"YANDI-E2E-E4-KEY", &mut key).ok()?;
    Some(key)
}

/// Зашифровать текст для телефона (его открытый x25519 из подписанной связки).
pub fn seal(text: &str, recipient_x25519: &[u8; 32], from: &str, to: &str, kind: u8) -> Result<Vec<u8>, String> {
    let k = keys().ok_or("закрытая среда заперта")?;
    seal_with(&k, text, recipient_x25519, from, to, kind, now_ms())
}

fn seal_with(sender: &Keys, text: &str, recipient_x25519: &[u8; 32], from: &str, to: &str, kind: u8, ts: u64) -> Result<Vec<u8>, String> {
    use ed25519_dalek::Signer;
    let from = hex::decode(from).ok().filter(|v| v.len() == 32).ok_or("invalid sender")?;
    let to = hex::decode(to).ok().filter(|v| v.len() == 32).ok_or("invalid recipient")?;
    if kind != 1 && kind != 2 { return Err("invalid E4 kind".into()); }
    let mut eph_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut eph_bytes);
    let eph = StaticSecret::from(eph_bytes);
    let eph_pub = PublicKey::from(&eph).to_bytes();
    let shared = eph.diffie_hellman(&PublicKey::from(*recipient_x25519));
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let mut id = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut id);
    let header = [&[MAGIC, kind][..], &from, &to, &id, &ts.to_be_bytes(), &eph_pub, &nonce].concat();
    let key = derived_key(shared.as_bytes(), &header).ok_or("invalid X25519 key")?;
    let cipher = Aes256Gcm::new_from_slice(&key).expect("32-byte key");
    let ct = cipher.encrypt(Nonce::from_slice(&nonce), Payload { msg: text.as_bytes(), aad: &aad(&header) }).map_err(|_| "encrypt failed")?;
    let wire = [header, ct].concat();
    let sig = sender.signing.sign(&[SIGN_DOMAIN, &wire].concat()).to_bytes();
    Ok([wire, sig.to_vec()].concat())
}

/// Authenticated E4 only; context mismatch, tampering, replay or persistence error -> None.
pub fn open(blob: &[u8], from: &str, to: &str, kind: u8, sender_ed: &[u8; 32]) -> Option<String> {
    let k = keys()?;
    let text = open_with(&k.x_secret, blob, from, to, kind, sender_ed, now_ms())?;
    let path = crate::util::data_dir::data_dir().join("mobile_replay_e4.json");
    claim_replay(&path, blob, now_ms()).ok().filter(|v| *v)?;
    Some(text)
}

/// Как [`open`], но номер письма в журнале повторов НЕ занимается: письмо проверено (подпись, AEAD, срок) и ещё не виденное.
/// Занять номер — отдельно, [`commit_replay`], когда результат обработки уже долговечен (сообщение сохранено): иначе сбой
/// сохранения после занятия номера терял бы сообщение (повтор отвергался бы как «уже виденное»).
pub fn open_deferred(blob: &[u8], from: &str, to: &str, kind: u8, sender_ed: &[u8; 32]) -> Option<String> {
    let k = keys()?;
    let text = open_with(&k.x_secret, blob, from, to, kind, sender_ed, now_ms())?;
    let path = crate::util::data_dir::data_dir().join("mobile_replay_e4.json");
    match replay_seen(&path, blob, now_ms()) {
        Ok(false) => Some(text),
        _ => None, // уже видели либо журнал повреждён (закрытый отказ, как у open)
    }
}

/// Занять номер письма после успешной обработки. `false` — не удалось записать журнал: письмо надо обработать повторно
/// (повтор безопасен: сохранение сообщения идемпотентно по номеру).
pub fn commit_replay(blob: &[u8]) -> bool {
    let path = crate::util::data_dir::data_dir().join("mobile_replay_e4.json");
    claim_replay(&path, blob, now_ms()).is_ok()
}

fn replay_seen(path: &std::path::Path, blob: &[u8], now: u64) -> Result<bool, String> {
    let _lock = REPLAY_LOCK.lock().map_err(|_| "replay lock poisoned")?;
    let seen: std::collections::BTreeMap<String, u64> = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| "corrupt replay ledger")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(e) => return Err(e.to_string()),
    };
    if blob.len() < 82 {
        return Err("short blob".into());
    }
    let id = hex::encode(&blob[2..82]);
    Ok(seen.get(&id).is_some_and(|expires| *expires >= now))
}

fn open_with(secret: &StaticSecret, blob: &[u8], from: &str, to: &str, kind: u8, sender_ed: &[u8; 32], now: u64) -> Option<String> {
    use ed25519_dalek::{Signature, VerifyingKey};
    if blob.len() < HEADER + 16 + 64 || blob[0] != MAGIC || blob[1] != kind || !matches!(kind, 1 | 2) {
        return None;
    }
    if hex::decode(from).ok()?.as_slice() != &blob[2..34] || hex::decode(to).ok()?.as_slice() != &blob[34..66] { return None; }
    let ts = u64::from_be_bytes(blob[82..90].try_into().ok()?);
    if ts > now.saturating_add(300_000) || now > ts.saturating_add(REPLAY_TTL_MS) { return None; }
    let end = blob.len() - 64;
    let sig = Signature::from_bytes(blob[end..].try_into().ok()?);
    VerifyingKey::from_bytes(sender_ed).ok()?.verify_strict(&[SIGN_DOMAIN, &blob[..end]].concat(), &sig).ok()?;
    let eph: [u8; 32] = blob[90..122].try_into().ok()?;
    let shared = secret.diffie_hellman(&PublicKey::from(eph));
    let key = derived_key(shared.as_bytes(), &blob[..HEADER])?;
    let cipher = Aes256Gcm::new_from_slice(&key).ok()?;
    let plain = cipher.decrypt(Nonce::from_slice(&blob[122..134]), Payload { msg: &blob[HEADER..end], aad: &aad(&blob[..HEADER]) }).ok()?;
    String::from_utf8(plain).ok()
}

static REPLAY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Called only after successful signature and AEAD checks. Never evict unexpired IDs.
fn claim_replay(path: &std::path::Path, blob: &[u8], now: u64) -> Result<bool, String> {
    let _lock = REPLAY_LOCK.lock().map_err(|_| "replay lock poisoned")?;
    let mut seen: std::collections::BTreeMap<String, u64> = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| "corrupt replay ledger")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(e) => return Err(e.to_string()),
    };
    seen.retain(|_, expires| *expires >= now);
    let id = hex::encode(&blob[2..82]); // sender + recipient + random signed id
    if seen.contains_key(&id) { return Ok(false); }
    if seen.len() >= 100_000 { return Err("replay ledger full".into()); }
    let ts = u64::from_be_bytes(blob[82..90].try_into().map_err(|_| "invalid timestamp")?);
    seen.insert(id, ts.saturating_add(REPLAY_TTL_MS));
    let bytes = serde_json::to_vec(&seen).map_err(|e| e.to_string())?;
    crate::util::private_file::write_private(path, &bytes).map_err(|e| e.to_string())?;
    Ok(true)
}

// ── Файлы между телефоном и компьютером ──
//
// Та же схема, что между телефонами (`file_crypto.dart`, docs/CLIENT_WIRE.md «Файлы между телефонами»): свой AES-256-GCM-ключ на файл,
// кусок 256 КБ, на проводе `[12 nonce][шифртекст][16 метка]`, AAD = `yandi-file:v1|<номер передачи>|<номер куска>|<всего>`. Ключ, имя, размер
// и sha256 уходят зашифрованным предложением `\u0001yandi-file:{json}`. Узел здесь — конечная точка: принимает файл от телефона в папку
// загрузок страницы (`files/downloads`), отдаёт файл со страницы телефону через хранилище кусков `mobile_files`.

pub const FILE_MARKER: &str = "\u{1}yandi-file:";
/// Размер куска: файлы от 5 МБ — по 1 МБ (быстрее), остальные — по 256 КБ, как раньше.
const FILE_CHUNK_SMALL: usize = 256 * 1024;
const FILE_CHUNK_BIG: usize = 1 << 20;
const BIG_FILE: u64 = 5 << 20;

fn file_chunk_for(size: u64) -> usize {
    if size >= BIG_FILE { FILE_CHUNK_BIG } else { FILE_CHUNK_SMALL }
}

#[derive(Serialize, Deserialize)]
struct Offer {
    #[serde(default)]
    v: u32,
    tid: String,
    name: String,
    size: u64,
    chunks: u32,
    #[serde(default)]
    mime: String,
    key: String,
    #[serde(default)]
    sha: String,
}

fn file_aad(tid: &str, idx: u32, total: u32) -> Vec<u8> {
    format!("yandi-file:v1|{tid}|{idx}|{total}").into_bytes()
}

fn seal_chunk(key: &[u8; 32], tid: &str, idx: u32, total: u32, plain: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    let ct = cipher.encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad: &file_aad(tid, idx, total) }).expect("aes-gcm encrypt");
    [&nonce[..], &ct].concat()
}

fn open_chunk(key: &[u8], tid: &str, idx: u32, total: u32, wire: &[u8]) -> Option<Vec<u8>> {
    if wire.len() < 12 + 16 {
        return None;
    }
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    cipher.decrypt(Nonce::from_slice(&wire[..12]), Payload { msg: &wire[12..], aad: &file_aad(tid, idx, total) }).ok()
}

/// Принятый от телефона файл: куда он лёг и как показать его на странице чата.
pub struct ReceivedFile {
    pub attachment: crate::communication::FileAttachment,
    /// Номер файлового сообщения у отправившего телефона.
    pub receipt_id: String,
}

/// Телефон прислал предложение файла компьютеру: забрать куски из `mobile_files`, расшифровать, сверить размер и sha256, положить в
/// загрузки страницы. Куски с узла удаляются после успеха.
pub async fn receive_file(node_hex: &str, from_device: &str, offer_text: &str) -> Result<ReceivedFile, String> {
    use sha2::{Digest, Sha256};
    let o: Offer = serde_json::from_str(offer_text.strip_prefix(FILE_MARKER).ok_or("не предложение файла")?).map_err(|e| e.to_string())?;
    let key = unb64(&o.key).filter(|k| k.len() == 32).ok_or("неверный ключ файла")?;
    let (chunks, size) = crate::mobile_files::ready_for(&o.tid, from_device, node_hex).await.ok_or("передача не найдена или не завершена")?;
    if chunks != o.chunks || size != o.size {
        return Err("предложение не совпадает с передачей на узле".into());
    }
    let file_id = format!("phone_{}", o.tid);
    let dir = crate::communication::files_dir("downloads");
    tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
    let stored = format!("{}__{}", file_id, crate::web::server::sanitize_storage_filename(&o.name));
    let part = dir.join(format!("{stored}.part"));
    // принятое сразу ложится запечатанными блоками (ключ защитной среды): открытого файла на диске нет
    let mut out = crate::file_vault::Writer::create(&part, o.size).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let res: Result<(), String> = async {
        for i in 0..o.chunks {
            let wire = crate::mobile_files::read_chunk(&o.tid, i).await.ok_or(format!("нет куска {}", i + 1))?;
            let plain = open_chunk(&key, &o.tid, i, o.chunks, &wire).ok_or(format!("кусок {} не прошёл проверку: подменён или повреждён", i + 1))?;
            hash.update(&plain);
            total += plain.len() as u64;
            if total > o.size {
                return Err(format!("размер не совпал: больше {}", o.size));
            }
            out.push(&plain).map_err(|e| e.to_string())?;
        }
        if total != o.size {
            return Err(format!("размер не совпал: {total} из {}", o.size));
        }
        out.finish().map_err(|e| e.to_string())?;
        if !o.sha.is_empty() && hex::encode(hash.finalize()) != o.sha.to_lowercase() {
            return Err("контрольная сумма не совпала".into());
        }
        Ok(())
    }
    .await;
    if let Err(e) = res {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    tokio::fs::rename(&part, dir.join(&stored)).await.map_err(|e| e.to_string())?;
    // серверные куски остаются: их удаляет вызывающий (`drop_transfer`) после записи сообщения в историю
    let mime = if o.mime.is_empty() { "application/octet-stream".to_string() } else { o.mime };
    Ok(ReceivedFile {
        receipt_id: format!("file_{}", o.tid),
        attachment: crate::communication::FileAttachment {
            filename: o.name,
            size: o.size,
            mime_type: mime,
            data: None,
            file_ref: Some(crate::communication::FileReference { file_id, total_chunks: o.chunks, local_name: Some(stored) }),
        },
    })
}

/// Отправить файл со страницы телефону: зашифровать куски своим ключом, положить в `mobile_files` (от узла телефону) и вернуть текст
/// предложения (его надо запечатать ключом телефона и положить в его очередь).
pub async fn send_file(node_hex: &str, device: &str, path: &std::path::Path, name: &str, mime: &str) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let size = crate::file_vault::plain_len(path).map_err(|e| e.to_string())?;
    let chunk = file_chunk_for(size);
    let chunks = (size.div_ceil(chunk as u64)).max(1) as u32;
    let tid = crate::mobile_files::node_begin(node_hex, device, size, chunks).await?;
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    let mut hash = Sha256::new();
    let res: Result<(), String> = async {
        for i in 0..chunks {
            let want = (size - i as u64 * chunk as u64).min(chunk as u64) as usize;
            let (p, off) = (path.to_path_buf(), i as u64 * chunk as u64);
            let buf = tokio::task::spawn_blocking(move || crate::file_vault::read_range(&p, off, want))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            hash.update(&buf);
            crate::mobile_files::node_put(&tid, i, &seal_chunk(&key, &tid, i, chunks, &buf)).await?;
        }
        crate::mobile_files::node_finish(&tid).await
    }
    .await;
    if let Err(e) = res {
        crate::mobile_files::drop_transfer(&tid).await;
        return Err(e);
    }
    let offer = Offer {
        v: 1,
        tid,
        name: name.to_string(),
        size,
        chunks,
        mime: if mime.is_empty() { "application/octet-stream".into() } else { mime.to_string() },
        key: b64(&key),
        sha: hex::encode(hash.finalize()),
    };
    Ok(format!("{FILE_MARKER}{}", serde_json::to_string(&offer).map_err(|e| e.to_string())?))
}

// ── Служебные конверты приложения (`receipt.dart` MsgChannel): статусы доставки и правки ──
//
// Всё ходит внутри того же шифрования, узел-собеседник разбирает их после расшифровки. Свои сообщения узел заворачивает в конверт с
// номером (cmid = msg_id сообщения в hex), чтобы телефон прислал «доставлено» и «прочитано»; входящим конвертам телефона узел отвечает
// «доставлено» сразу и «прочитано», когда владелец открыл переписку на странице.

const MSG_MARKER: &str = "\u{1}yandi-msg:";
const RCPT_MARKER: &str = "\u{1}yandi-rcpt:";
const CAP_MARKER: &str = "\u{1}yandi-cap:";
const EDIT_MARKER: &str = "\u{1}yandi-edit:";

pub enum Envelope {
    /// сообщение с номером отправителя
    Message { cmid: String, text: String, reply: Option<(String, String)> },
    /// квитанция: «delivered» или «read» по номерам наших сообщений
    Receipt { read: bool, ids: Vec<String> },
    /// правка ранее присланного сообщения
    Edit { cmid: String, text: String },
    /// устройство умеет квитанции
    CapPing,
    /// смайл на сообщение (пустой — снят): номер сообщения как у приложения
    Reaction { cmid: String, emoji: String },
    /// служебный конверт, которого этот узел не знает (например, аватарки `yandi-avh/avq/ava` между телефонами): молча отбрасывается,
    /// чтобы не показываться на странице чата как мусорное сообщение
    Ignored,
    /// обычный текст (старая сборка)
    Plain(String),
}

pub fn classify(text: String) -> Envelope {
    #[derive(Deserialize)]
    struct M {
        cmid: String,
        text: String,
        #[serde(default)]
        reply: Option<Rp>,
    }
    #[derive(Deserialize)]
    struct Rp {
        id: String,
        text: String,
    }
    #[derive(Deserialize)]
    struct R {
        kind: String,
        ids: Vec<String>,
    }
    if let Some(j) = text.strip_prefix(MSG_MARKER) {
        if let Ok(m) = serde_json::from_str::<M>(j) {
            if !m.cmid.is_empty() {
                return Envelope::Message { cmid: m.cmid, text: m.text, reply: m.reply.map(|r| (r.id, r.text)) };
            }
        }
    } else if let Some(j) = text.strip_prefix(RCPT_MARKER) {
        if let Ok(r) = serde_json::from_str::<R>(j) {
            if r.kind == "delivered" || r.kind == "read" {
                return Envelope::Receipt { read: r.kind == "read", ids: r.ids };
            }
        }
    } else if let Some(j) = text.strip_prefix(EDIT_MARKER) {
        if let Ok(m) = serde_json::from_str::<M>(j) {
            if !m.cmid.is_empty() {
                return Envelope::Edit { cmid: m.cmid, text: m.text };
            }
        }
    } else if text.starts_with(CAP_MARKER) {
        return Envelope::CapPing;
    } else if text.starts_with(crate::chat_reactions::MARKER) {
        return match crate::chat_reactions::parse(&text) {
            Some((cmid, emoji)) => Envelope::Reaction { cmid, emoji },
            None => Envelope::Ignored,
        };
    } else if text.starts_with("\u{1}yandi-") && !text.starts_with(FILE_MARKER) {
        return Envelope::Ignored;
    }
    Envelope::Plain(text)
}

pub fn wrap_message(cmid: &str, text: &str) -> String {
    wrap_message_reply(cmid, text, None)
}

/// Сообщение с ответом-цитатой: `reply` = (номер сообщения на узле, выдержка его текста).
pub fn wrap_message_reply(cmid: &str, text: &str, reply: Option<(&str, &str)>) -> String {
    let mut j = serde_json::json!({"v": 1, "cmid": cmid, "text": text});
    if let Some((id, quote)) = reply {
        j["reply"] = serde_json::json!({"id": id, "text": quote});
    }
    format!("{MSG_MARKER}{j}")
}

pub fn receipt(read: bool, ids: &[String]) -> String {
    format!("{RCPT_MARKER}{}", serde_json::json!({"kind": if read { "read" } else { "delivered" }, "ids": ids}))
}

pub fn edit(cmid: &str, text: &str) -> String {
    format!("{EDIT_MARKER}{}", serde_json::json!({"cmid": cmid, "text": text}))
}

pub fn cap_ping() -> String {
    format!("{CAP_MARKER}1")
}

/// Номер входящего сообщения телефона в истории узла: выводится из номера устройства и его cmid (по нему же находится правка).
pub fn incoming_id(device: &str, cmid: &str) -> crate::util::HashId {
    use sha2::{Digest, Sha256};
    crate::util::HashId(Sha256::digest(format!("yandi-phone-msg:{device}:{cmid}").as_bytes()).into())
}

/// Входящие конверты, на которые ещё не ушло «прочитано»: номер в истории → (устройство, cmid). Файл `mobile_self_unread.json`.
#[derive(Serialize, Deserialize, Clone)]
struct Unread {
    device: String,
    cmid: String,
}

static UNREAD: crate::util::sealed_json::SealedJson<std::collections::HashMap<String, Unread>> =
    crate::util::sealed_json::SealedJson::new("mobile_self_unread.json", "mobile_self_unread.enc", "mobile_self_unread");

fn with_unread<R>(f: impl FnOnce(&mut std::collections::HashMap<String, Unread>) -> R) -> R {
    UNREAD.with(|map| {
        let r = f(map);
        if map.len() > 20000 {
            map.clear(); // старьё: «прочитано» по ним уже не нужно
        }
        r
    })
}

/// Закрытая среда: ключ области выдан / среда заперта / среда стёрта (индекс непрочитанного — закрытый уровень).
pub fn unread_unlock(key: [u8; 32]) {
    UNREAD.unlock(key)
}
pub fn unread_lock() {
    UNREAD.lock()
}
pub fn unread_erase() {
    UNREAD.erase()
}

pub fn remember_unread(local: &crate::util::HashId, device: &str, cmid: &str) {
    with_unread(|m| {
        m.insert(hex::encode(local.0), Unread { device: device.to_string(), cmid: cmid.to_string() });
    });
}

/// Забрать номера (cmid) входящих этого телефона, по которым ещё не отправлено «прочитано».
pub fn take_unread(device: &str) -> Vec<String> {
    with_unread(|m| {
        let keys: Vec<String> = m.iter().filter(|(_, u)| u.device == device).map(|(k, _)| k.clone()).collect();
        keys.into_iter().filter_map(|k| m.remove(&k)).map(|u| u.cmid).collect()
    })
}

/// Передачи файлов узел → телефон: номер передачи → (устройство, номер сообщения в истории). Когда телефон забрал файл и удалил передачу,
/// сообщение на странице становится «доставлено».
static FILE_MSGS: std::sync::Mutex<Option<std::collections::HashMap<String, (String, crate::util::HashId)>>> = std::sync::Mutex::new(None);

pub fn remember_file(tid: &str, device: &str, msg: crate::util::HashId) {
    FILE_MSGS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(tid.to_string(), (device.to_string(), msg));
}

pub fn take_file(tid: &str) -> Option<(String, crate::util::HashId)> {
    FILE_MSGS.lock().unwrap_or_else(|e| e.into_inner()).as_mut()?.remove(tid)
}

/// Номер передачи из текста предложения файла.
pub fn offer_tid(offer_text: &str) -> Option<String> {
    serde_json::from_str::<Offer>(offer_text.strip_prefix(FILE_MARKER)?).ok().map(|o| o.tid)
}

// ── Телефоны, спрятанные из списка веб-чата («Удалить» у телефона на странице). Сопряжение не трогается. ──

fn hidden_path() -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("web_hidden_phones.json")
}

fn with_hidden<R>(f: impl FnOnce(&mut Vec<String>) -> R) -> R {
    static H: std::sync::Mutex<Option<Vec<String>>> = std::sync::Mutex::new(None);
    let mut g = H.lock().unwrap_or_else(|e| e.into_inner());
    let list = g.get_or_insert_with(|| std::fs::read_to_string(hidden_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default());
    let before = list.clone();
    let r = f(list);
    if *list != before {
        if let Ok(s) = serde_json::to_vec(&*list) {
            let _ = crate::util::private_file::write_private(&hidden_path(), &s);
        }
    }
    r
}

// ── Какие устройства узла — «контакты» ──
//
// Телефоны, сопряжённые с узлом, не обязаны быть контактами владельца: в чате компьютера контактами считаются только добавленные.
// Остальные — в отдельном списке «Другие устройства узла», где их можно добавить в контакты, скрыть или заблокировать. Заблокированное
// устройство молча не доставляет компьютеру ни сообщений, ни файлов, ни звонков, и компьютер ему не пишет. При первом запуске этой
// возможности все уже видимые телефоны остаются контактами (чтобы ничего не пропало из привычного списка).
#[derive(Serialize, Deserialize, Default, Clone, PartialEq)]
struct PhoneStatus {
    #[serde(default)]
    saved: Vec<String>,
    #[serde(default)]
    blocked: Vec<String>,
}

fn status_path() -> std::path::PathBuf {
    crate::util::data_dir::data_dir().join("web_phone_status.json")
}

fn with_status<R>(f: impl FnOnce(&mut PhoneStatus) -> R) -> R {
    static S: std::sync::Mutex<Option<PhoneStatus>> = std::sync::Mutex::new(None);
    let mut g = S.lock().unwrap_or_else(|e| e.into_inner());
    let st = g.get_or_insert_with(|| match std::fs::read_to_string(status_path()).ok().and_then(|s| serde_json::from_str::<PhoneStatus>(&s).ok()) {
        Some(v) => v,
        None => {
            // первый запуск: уже видимые телефоны остаются контактами
            let saved: Vec<String> = crate::mobile_api::devices_overview().iter().filter_map(|d| d["peer_id"].as_str().map(str::to_string)).filter(|p| !is_hidden_phone(p)).collect();
            let v = PhoneStatus { saved, blocked: Vec::new() };
            if let Ok(b) = serde_json::to_vec(&v) {
                let _ = crate::util::private_file::write_private(&status_path(), &b);
            }
            v
        }
    });
    let before = st.clone();
    let r = f(st);
    if *st != before {
        if let Ok(b) = serde_json::to_vec(&*st) {
            let _ = crate::util::private_file::write_private(&status_path(), &b);
        }
    }
    r
}

/// Учёт контактов создаётся при первом обращении (уже сопряжённые устройства остаются контактами) — до того, как придут новые.
pub fn init_phone_status() {
    with_status(|_| {});
}

pub fn is_saved_phone(peer: &str) -> bool {
    with_status(|s| s.saved.iter().any(|x| x == peer))
}

pub fn is_blocked_phone(peer: &str) -> bool {
    with_status(|s| s.blocked.iter().any(|x| x == peer))
}

/// Действие над устройством: add / remove (из контактов), block / unblock. Скрытие — `hide_phone` / `unhide_phone`.
pub fn phone_action(peer: &str, action: &str) -> Result<(), String> {
    if peer.len() != 64 {
        return Err("неверный номер устройства".into());
    }
    match action {
        "add" => with_status(|s| {
            s.blocked.retain(|x| x != peer);
            if !s.saved.iter().any(|x| x == peer) {
                s.saved.push(peer.to_string());
            }
        }),
        "remove" => with_status(|s| s.saved.retain(|x| x != peer)),
        "block" => with_status(|s| {
            s.saved.retain(|x| x != peer);
            if !s.blocked.iter().any(|x| x == peer) {
                s.blocked.push(peer.to_string());
            }
        }),
        "unblock" => with_status(|s| s.blocked.retain(|x| x != peer)),
        "hide" => hide_phone(peer),
        "unhide" => unhide_phone(peer),
        _ => return Err("неизвестное действие".into()),
    }
    if action == "add" {
        unhide_phone(peer);
    }
    Ok(())
}

/// Спрятать телефон из списка чата (номер устройства полностью или его начало, не короче 16 знаков).
pub fn hide_phone(id: &str) {
    if id.len() >= 16 {
        with_hidden(|l| {
            if !l.iter().any(|x| x == id) {
                l.push(id.to_string());
            }
        });
    }
}

pub fn is_hidden_phone(peer: &str) -> bool {
    with_hidden(|l| l.iter().any(|x| peer.starts_with(x.as_str())))
}

/// Спрятанный телефон написал — вернуть его в список, чтобы сообщение не потерялось незаметно.
pub fn unhide_phone(peer: &str) {
    with_hidden(|l| l.retain(|x| !peer.starts_with(x.as_str())));
}

// ── Звонки телефон ↔ страница узла ──
//
// Сигналы звонка (`\u0001yandi-call:` — invite/accept/offer/answer/ice/reject/busy/hangup, как в call_signal.dart) телефон шифрует для
// узла; узел их расшифровывает и отдаёт странице чата (она опрашивает очередь раз в секунду), а ответы страницы шифрует телефону. Звук и
// видео идут напрямую браузер ↔ телефон (WebRTC, DTLS-SRTP) — узел их не видит. Пока страница не открыта, звонок на компьютер получает
// «не на связи».

pub const CALL_MARKER: &str = "\u{1}yandi-call:";

struct CallQueue {
    seq: u64,
    events: std::collections::VecDeque<(u64, String, String)>,
    last_poll: Option<std::time::Instant>,
}

static CALLS: std::sync::Mutex<CallQueue> = std::sync::Mutex::new(CallQueue { seq: 0, events: std::collections::VecDeque::new(), last_poll: None });

/// Страница чата открыта (опрашивала очередь звонков последние несколько секунд)?
pub fn call_page_alive() -> bool {
    CALLS.lock().unwrap_or_else(|e| e.into_inner()).last_poll.map(|t| t.elapsed() < std::time::Duration::from_secs(6)).unwrap_or(false)
}

/// Номера звонков, на которые страница не ответила (её не было открыто). Хранятся только в памяти и только номера: ни имён, ни времени.
static MISSED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Запомнить пропущенный звонок: сигнал «invite» свежий и корректный, а страница не открыта. Повтор того же звонка не считается.
pub fn note_missed_call(text: &str) {
    let Some(rest) = text.strip_prefix(CALL_MARKER) else { return };
    let Ok(m) = serde_json::from_str::<serde_json::Value>(rest) else { return };
    let (Some("invite"), Some(cid), Some(ts)) = (m["t"].as_str(), m["cid"].as_str(), m["ts"].as_i64()) else { return };
    if m["v"] != 1 || cid.len() < 8 || cid.len() > 64 {
        return;
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    if (now - ts).abs() >= 90_000 {
        return;
    }
    let mut l = MISSED.lock().unwrap_or_else(|e| e.into_inner());
    if !l.iter().any(|c| c == cid) && l.len() < 99 {
        l.push(cid.to_string());
    }
}

pub fn missed_calls() -> usize {
    MISSED.lock().unwrap_or_else(|e| e.into_inner()).len()
}

pub fn clear_missed_calls() {
    MISSED.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

pub fn push_call(from_device: &str, text: &str) {
    let mut q = CALLS.lock().unwrap_or_else(|e| e.into_inner());
    q.seq += 1;
    let seq = q.seq;
    q.events.push_back((seq, from_device.to_string(), text.to_string()));
    while q.events.len() > 500 {
        q.events.pop_front();
    }
}

/// События после `after` для страницы: (номер, от кого, текст сигнала); и последний номер.
pub fn poll_calls(after: u64) -> (Vec<(u64, String, String)>, u64) {
    let (events, last) = {
        let mut q = CALLS.lock().unwrap_or_else(|e| e.into_inner());
        q.last_poll = Some(std::time::Instant::now());
        (q.events.iter().filter(|(s, _, _)| *s > after).cloned().collect::<Vec<_>>(), q.seq)
    };
    // устройство заблокировали или забыли, пока сигнал ждал в очереди: страница его не получает
    (events.into_iter().filter(|(_, from, _)| !is_blocked_phone(from) && crate::mobile_api::is_device_peer(from)).collect(), last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missed_calls_count_only_fresh_invites_once() {
        clear_missed_calls();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
        let invite = |t: &str, cid: &str, ts: u128| format!("{CALL_MARKER}{{\"v\":1,\"t\":\"{t}\",\"cid\":\"{cid}\",\"video\":false,\"ts\":{ts}}}");
        note_missed_call(&invite("invite", "abcdefgh1", now));
        note_missed_call(&invite("invite", "abcdefgh1", now)); // повтор того же звонка
        note_missed_call(&invite("hangup", "abcdefgh2", now)); // не вызов
        note_missed_call(&invite("invite", "abcdefgh3", now - 600_000)); // устарел
        note_missed_call("обычный текст");
        assert_eq!(missed_calls(), 1);
        clear_missed_calls();
        assert_eq!(missed_calls(), 0);
    }

    #[test]
    fn bug_r15_mobile_key_holder_revokes_without_lazy_plain_reopen() {
        let holder = Mutex::new(KeyHolder::default());
        assert!(keys_from(&holder, |_| panic!("запертый держатель не должен обращаться к диску")).is_none());
        holder.lock().unwrap().replace(Zeroizing::new([7; 32]));
        let old = keys_from(&holder, |master| make_keys(*master.unwrap(), *master.unwrap())).unwrap().ed_pub;
        holder.lock().unwrap().revoke();
        assert!(keys_from(&holder, |_| panic!("отозванный ключ не должен восстанавливаться")).is_none());
        assert!(holder.lock().unwrap().master.is_none());
        holder.lock().unwrap().replace(Zeroizing::new([8; 32]));
        let next = keys_from(&holder, |master| make_keys(*master.unwrap(), *master.unwrap())).unwrap().ed_pub;
        assert_ne!(old, next);
    }

    #[test]
    fn bug_r15_mobile_revoke_waits_for_key_guard_and_then_erases_holder() {
        let holder = std::sync::Arc::new(Mutex::new(KeyHolder::default()));
        holder.lock().unwrap().replace(Zeroizing::new([7; 32]));
        let guard = keys_from(&holder, |master| make_keys(*master.unwrap(), *master.unwrap())).unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let closing = holder.clone();
        let closer = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            closing.lock().unwrap().revoke();
            done_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(done_rx.try_recv().is_err());
        assert!(holder.try_lock().is_err());
        drop(guard);
        done_rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        closer.join().unwrap();
        let state = holder.lock().unwrap();
        assert!(state.keys.is_none() && state.master.is_none());
    }

    #[test]
    fn bug_r15_poisoned_mobile_holder_can_still_be_revoked() {
        let holder = std::sync::Arc::new(Mutex::new(KeyHolder::default()));
        holder.lock().unwrap().replace(Zeroizing::new([7; 32]));
        drop(keys_from(&holder, |master| make_keys(*master.unwrap(), *master.unwrap())).unwrap());
        let poisoned = holder.clone();
        assert!(std::thread::spawn(move || {
            let _guard = poisoned.lock().unwrap();
            panic!("отказ операции");
        }).join().is_err());
        assert!(keys_from(&holder, |_| panic!("после panic не открываем ключи")).is_none());
        let state = match holder.lock() { Err(e) => e.into_inner(), Ok(_) => panic!("замок должен быть отравлен") };
        assert!(state.keys.is_none() && state.master.is_none());
        drop(state);
        revoke_holder(&holder); // повторная явная блокировка тоже не падает
    }

    #[test]
    fn mobile_e4_independent_python_vector() {
        let v: serde_json::Value = serde_json::from_str(include_str!("../tests/mobile_e4_vector.json")).unwrap();
        let wire = hex::decode(v["wire"].as_str().unwrap()).unwrap();
        let secret = StaticSecret::from(<[u8;32]>::try_from(hex::decode(v["x_secret"].as_str().unwrap()).unwrap()).unwrap());
        let ed = <[u8;32]>::try_from(hex::decode(v["ed_public"].as_str().unwrap()).unwrap()).unwrap();
        let from = v["sender"].as_str().unwrap();
        let to = v["recipient"].as_str().unwrap();
        let now = v["timestamp"].as_u64().unwrap();
        assert_eq!(open_with(&secret,&wire,from,to,1,&ed,now).as_deref(),v["text"].as_str());
        // Every header byte, ciphertext/tag and signature byte is authenticated.
        for i in 0..wire.len() {
            let mut bad = wire.clone(); bad[i] ^= 1;
            assert!(open_with(&secret,&bad,from,to,1,&ed,now).is_none(),"byte {i}");
        }
    }

    #[test]
    fn sealed_for_a_key_opens_only_with_it_and_tampering_is_refused() {
        let me = make_keys([1; 32], [2; 32]);
        let other = make_keys([3; 32], [4; 32]);
        let from = hex::encode([10; 32]);
        let to = hex::encode([11; 32]);
        let blob = seal_with(&other, "привет", &me.x_pub, &from, &to, 1, now_ms()).unwrap();
        assert_eq!(blob[0], MAGIC);
        let check = |wire: &[u8]| open_with(&me.x_secret, wire, &from, &to, 1, &other.ed_pub, now_ms());
        assert_eq!(check(&blob).as_deref(), Some("привет"));
        assert!(open_with(&other.x_secret, &blob, &from, &to, 1, &other.ed_pub, now_ms()).is_none(), "чужой ключ");
        assert!(open_with(&me.x_secret, &blob, &to, &from, 1, &other.ed_pub, now_ms()).is_none(), "адреса");
        assert!(open_with(&me.x_secret, &blob, &from, &to, 2, &other.ed_pub, now_ms()).is_none(), "тип");
        assert!(open_with(&me.x_secret, &blob, &from, &to, 1, &me.ed_pub, now_ms()).is_none(), "подмена отправителя");
        let mut bad = blob.clone();
        let n = bad.len();
        bad[n - 1] ^= 1;
        assert!(check(&bad).is_none(), "подпись не сходится");
        let mut bad_eph = blob.clone();
        bad_eph[5] ^= 1;
        assert!(check(&bad_eph).is_none(), "контекст подписан");
        // Even a re-signed header cannot transplant existing ciphertext to a
        // new message id: AAD/HKDF independently bind the entire header.
        let mut rebound = blob.clone();
        rebound[66] ^= 1;
        let end = rebound.len() - 64;
        use ed25519_dalek::Signer;
        let sig = other.signing.sign(&[SIGN_DOMAIN, &rebound[..end]].concat()).to_bytes();
        rebound[end..].copy_from_slice(&sig);
        assert!(check(&rebound).is_none(), "новый контекст с корректной подписью не проходит AEAD");
        assert!(check(b"plain text that is long enough to pass the length check........").is_none());
        let expired = seal_with(&other, "old", &me.x_pub, &from, &to, 1, 1).unwrap();
        assert!(check(&expired).is_none());
        assert!(seal_with(&other, "bad", &[0; 32], &from, &to, 1, now_ms()).is_err());
    }

    #[test]
    fn the_bundle_signature_verifies_like_the_phone_checks_it() {
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};
        let k = make_keys([5; 32], [6; 32]);
        let vk = VerifyingKey::from_bytes(&k.ed_pub).unwrap();
        assert!(vk.verify(&[BUNDLE_DOMAIN, &k.ed_pub, &k.x_pub].concat(), &Signature::from_bytes(&k.sig)).is_ok());
    }

    #[test]
    fn keys_survive_a_restart() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("mobile_self.json");
        let a = load_or_create(&p);
        let b = load_or_create(&p);
        assert_eq!(a.x_pub, b.x_pub);
        assert_eq!(a.ed_pub, b.ed_pub);
    }

    #[test]
    fn big_files_use_big_chunks_and_stay_within_the_node_limits() {
        assert_eq!(file_chunk_for(5 * 1024 * 1024 - 1), 256 * 1024);
        assert_eq!(file_chunk_for(5 * 1024 * 1024), 1 << 20);
        // самый большой файл (1 ГБ) крупными кусками укладывается в предел числа кусков, а кусок с накладными — в предел на проводе
        let chunks = (crate::mobile_files::MAX_FILE).div_ceil(file_chunk_for(crate::mobile_files::MAX_FILE) as u64);
        assert!(chunks <= crate::mobile_files::MAX_CHUNKS as u64);
        assert!(((1usize << 20) + 28) <= crate::mobile_files::MAX_CHUNK_BYTES);
    }

    #[test]
    fn file_chunks_open_only_in_their_place() {
        let key = [9u8; 32];
        let tid = "0123456789abcdef0123456789abcdef";
        let wire = seal_chunk(&key, tid, 1, 3, b"chunk");
        assert_eq!(open_chunk(&key, tid, 1, 3, &wire).as_deref(), Some(&b"chunk"[..]));
        assert!(open_chunk(&key, tid, 0, 3, &wire).is_none(), "другое место");
        assert!(open_chunk(&key, tid, 1, 2, &wire).is_none(), "обрезанная передача");
        assert!(open_chunk(&[8u8; 32], tid, 1, 3, &wire).is_none(), "чужой ключ");
    }

    #[test]
    fn service_envelopes_round_trip_like_the_app_writes_them() {
        assert!(matches!(classify(wrap_message("12", "hi")), Envelope::Message { cmid, text, .. } if cmid == "12" && text == "hi"));
        assert!(matches!(
            classify(wrap_message_reply("13", "ok", Some(("abc", "было")))),
            Envelope::Message { cmid, text, reply: Some((id, q)) } if cmid == "13" && text == "ok" && id == "abc" && q == "было"
        ));
        assert!(matches!(classify(receipt(true, &["a".into()])), Envelope::Receipt { read: true, ids } if ids == ["a"]));
        assert!(matches!(classify(edit("7", "new")), Envelope::Edit { cmid, text } if cmid == "7" && text == "new"));
        assert!(matches!(classify(cap_ping()), Envelope::CapPing));
        assert!(matches!(classify("просто текст".into()), Envelope::Plain(_)));
        assert!(matches!(classify("\u{1}yandi-avh:{\"h\":\"ab\"}".into()), Envelope::Ignored), "аватарка не должна стать сообщением на странице");
        assert!(matches!(classify("\u{1}yandi-somethingnew:{}".into()), Envelope::Ignored));
        assert!(matches!(classify(format!("{FILE_MARKER}{{}}")), Envelope::Plain(_)), "предложение файла разбирает приёмник файлов");
        // как пишет Dart: '\u0001yandi-rcpt:{"kind":"delivered","ids":["1791"]}'
        assert!(matches!(classify("\u{1}yandi-rcpt:{\"kind\":\"delivered\",\"ids\":[\"1791\"]}".into()), Envelope::Receipt { read: false, .. }));
        assert_eq!(incoming_id("d", "1"), incoming_id("d", "1"));
        assert_ne!(incoming_id("d", "1"), incoming_id("e", "1"));
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_key_file_is_never_replaced_by_a_new_identity() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let master = [9u8; 32];
        // настоящая личность записывается как обычно
        let first = load_protected(dir.path(), Some(&master));
        let enc = dir.path().join("mobile_self.enc");
        let before = std::fs::read(&enc).unwrap();
        // файл временно нечитаем (режим 000): новая пара поверх него не пишется
        std::fs::set_permissions(&enc, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&enc).is_ok() {
            return; // запуск от root: режим не мешает чтению, проверять нечего
        }
        let temp = load_protected(dir.path(), Some(&master));
        std::fs::set_permissions(&enc, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(std::fs::read(&enc).unwrap(), before, "прежний файл личности не заменён");
        assert!(!dir.path().join("mobile_self.json").exists(), "открытый промежуточный файл не создан");
        let again = load_protected(dir.path(), Some(&master));
        assert_eq!(again.x_pub, first.x_pub, "после починки прав — прежняя личность");
        assert_ne!(temp.x_pub, first.x_pub, "временная пара — другая");
    }

    /// Номер письма занимается после обработки: пока не занят, письмо «не виденное» и повтор проходит; после — отвергается.
    #[test]
    fn a_letter_is_seen_only_after_its_number_is_committed() {
        let phone = make_keys([7; 32], [8; 32]);
        let blob = seal_with(&phone, "x", &phone.x_pub, &hex::encode([1;32]), &hex::encode([2;32]), 1, now_ms()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("replays.json");
        assert!(!replay_seen(&path, &blob, now_ms()).unwrap(), "ещё не видели");
        // обработка сорвалась до фиксации: повтор допустим
        assert!(!replay_seen(&path, &blob, now_ms()).unwrap());
        assert!(claim_replay(&path, &blob, now_ms()).unwrap());
        assert!(replay_seen(&path, &blob, now_ms()).unwrap(), "после фиксации — уже виденное");
        std::fs::write(&path, b"broken").unwrap();
        assert!(replay_seen(&path, &blob, now_ms()).is_err(), "повреждённый журнал — закрытый отказ");
    }

    #[test]
    fn a_replayed_envelope_is_refused() {
        let phone = make_keys([7; 32], [8; 32]);
        let blob = seal_with(&phone, "x", &phone.x_pub, &hex::encode([1;32]), &hex::encode([2;32]), 1, now_ms()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("replays.json");
        assert!(claim_replay(&path, &blob, now_ms()).unwrap());
        assert!(!claim_replay(&path, &blob, now_ms()).unwrap(), "fresh disk read after restart");
        std::fs::write(&path, b"broken").unwrap();
        assert!(claim_replay(&path, &blob, now_ms()).is_err(), "never reset corrupt ledger");
    }

    #[test]
    fn mobile_replay_never_evicts_fresh_entries_and_writes_fail_closed() {
        let sender = make_keys([7;32],[8;32]);
        let wire = seal_with(&sender,"test",&sender.x_pub,&hex::encode([1;32]),&hex::encode([2;32]),1,100).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("replays.json");
        let full: std::collections::BTreeMap<String,u64> = (0..100_000).map(|i|(i.to_string(),100+REPLAY_TTL_MS)).collect();
        std::fs::write(&path,serde_json::to_vec(&full).unwrap()).unwrap();
        assert!(claim_replay(&path,&wire,100).is_err());
        assert_eq!(serde_json::from_slice::<std::collections::BTreeMap<String,u64>>(&std::fs::read(&path).unwrap()).unwrap().len(),100_000);
        assert!(claim_replay(dir.path(),&wire,100).is_err());
        // Pruning is allowed only when signed freshness checks reject old envelopes.
        let new = seal_with(&sender,"new",&sender.x_pub,&hex::encode([1;32]),&hex::encode([2;32]),1,101+REPLAY_TTL_MS).unwrap();
        assert!(claim_replay(&path,&new,101+REPLAY_TTL_MS).unwrap());
    }

    #[tokio::test]
    async fn file_receipts_use_phone_ids_and_pc_envelopes_keep_the_offer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("receipt-test.txt");
        let data = b"file receipt round trip";
        crate::file_vault::write_all(&path, data).unwrap();
        let device = format!("receipt-test-{}", rand::random::<u64>());
        let offer = send_file(&device, "receipt-test-pc", &path, "receipt-test.txt", "text/plain").await.unwrap();
        let tid = offer_tid(&offer).unwrap();
        let pc_id = crate::util::HashId([21; 32]);
        let cmid = hex::encode(pc_id.0);
        let phone = make_keys([22; 32], [23; 32]);
        let from = hex::encode([24;32]);
        let to = hex::encode([25;32]);
        let wire = seal_with(&phone, &wrap_message(&cmid, &offer), &phone.x_pub, &from, &to, 1, now_ms()).unwrap();
        assert!(matches!(classify(open_with(&phone.x_secret, &wire, &from, &to, 1, &phone.ed_pub, now_ms()).unwrap()),
            Envelope::Message { cmid: id, text, .. } if id == cmid && text == offer && offer_tid(&text) == Some(tid.clone())));

        let received = receive_file("receipt-test-pc", &device, &offer).await.unwrap();
        assert_eq!(received.receipt_id, format!("file_{tid}"));
        let saved = crate::communication::files_dir("downloads")
            .join(received.attachment.file_ref.unwrap().local_name.unwrap());
        assert_eq!(crate::file_vault::read_all(&saved).unwrap(), data);
        assert!(crate::mobile_files::ready_for(&tid, &device, "receipt-test-pc").await.is_some(), "куски остаются до записи сообщения");
        crate::mobile_files::drop_transfer(&tid).await;
        assert!(crate::mobile_files::ready_for(&tid, &device, "receipt-test-pc").await.is_none());
        let id = received.receipt_id;
        assert!(matches!(classify(receipt(false, &[id.clone()])), Envelope::Receipt { read: false, ids } if ids == [id.clone()]));
        let local = incoming_id(&device, &id);
        unread_unlock([9; 32]); // индекс непрочитанного теперь закрытый: в тесте выдаём ему ключ области
        remember_unread(&local, &device, &id);
        assert!(take_unread("another-device").is_empty());
        let unread = take_unread(&device);
        assert_eq!(unread, [id]);
        assert!(matches!(classify(receipt(true, &unread)), Envelope::Receipt { read: true, ids } if ids == unread));
        assert!(take_unread(&device).is_empty());
        tokio::fs::remove_file(saved).await.unwrap();
    }

    #[tokio::test]
    async fn a_corrupt_file_has_no_success_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.txt");
        crate::file_vault::write_all(&path, b"must not acknowledge corruption").unwrap();
        let from = format!("receipt-bad-{}", rand::random::<u64>());
        let offer = send_file(&from, "receipt-bad-pc", &path, "bad.txt", "text/plain").await.unwrap();
        let mut parsed: Offer = serde_json::from_str(offer.strip_prefix(FILE_MARKER).unwrap()).unwrap();
        parsed.sha = "00".repeat(32);
        let bad = format!("{FILE_MARKER}{}", serde_json::to_string(&parsed).unwrap());
        assert!(receive_file("receipt-bad-pc", &from, &bad).await.is_err());
        assert!(take_unread(&from).is_empty());
        assert!(crate::mobile_files::ready_for(&parsed.tid, &from, "receipt-bad-pc").await.is_some());
        crate::mobile_files::drop_transfer(&parsed.tid).await;
    }

    #[test]
    fn corrupt_pc_identity_is_left_untouched() {
        // тест Codex (аудит №8): повреждённый файл ключей не заменяется молча новой парой
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture_identity.json");
        let original = b"damaged identity fixture";
        std::fs::write(&path, original).unwrap();
        let _generated = load_or_create(&path);
        assert_eq!(std::fs::read(&path).unwrap(), original, "существующий повреждённый файл ключей не должен перезаписываться");
    }

    #[test]
    fn missing_pc_identity_is_created_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("id.json");
        let a = load_or_create(&path);
        let b = load_or_create(&path);
        assert_eq!(a.x_pub, b.x_pub, "после создания та же пара читается снова");
        assert_eq!(a.ed_pub, b.ed_pub);
    }

    #[test]
    fn interrupted_migration_is_finished_and_a_different_plain_pair_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (plain, master) = (dir.path().join("mobile_self.json"), [5u8; 32]);
        let old = load_or_create(&plain);
        let copy = std::fs::read(&plain).unwrap();
        load_protected(dir.path(), Some(&master));
        // как будто перенос оборвался после записи .enc, до удаления открытого файла
        std::fs::write(&plain, &copy).unwrap();
        let k = load_protected(dir.path(), Some(&master));
        assert_eq!(k.x_pub, old.x_pub);
        assert!(!plain.exists(), "открытая копия той же пары доубрана");
        // открытый файл с другой парой не трогаем (и не берём): только предупреждение
        let other = tempfile::tempdir().unwrap();
        let foreign = load_or_create(&other.path().join("mobile_self.json"));
        std::fs::copy(other.path().join("mobile_self.json"), &plain).unwrap();
        let k = load_protected(dir.path(), Some(&master));
        assert_eq!(k.x_pub, old.x_pub);
        assert_ne!(k.x_pub, foreign.x_pub);
        assert!(plain.exists());
    }

    #[test]
    fn pc_identity_moves_into_an_encrypted_file_and_stays_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let master = [9u8; 32];
        let old = load_or_create(&dir.path().join("mobile_self.json")); // как было: открытый файл
        let moved = load_protected(dir.path(), Some(&master));
        assert_eq!((moved.x_pub, moved.ed_pub), (old.x_pub, old.ed_pub), "личность та же");
        assert!(!dir.path().join("mobile_self.json").exists(), "открытый файл убран");
        let raw = std::fs::read(dir.path().join("mobile_self.enc")).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("x25519_secret"), "на диске нет открытых ключей");
        let again = load_protected(dir.path(), Some(&master));
        assert_eq!(again.x_pub, old.x_pub, "после перезапуска читается та же пара");
        let foreign = load_protected(dir.path(), Some(&[1u8; 32]));
        assert_ne!(foreign.x_pub, old.x_pub, "чужой мастер-ключ — временная пара");
        assert_eq!(std::fs::read(dir.path().join("mobile_self.enc")).unwrap(), raw, "и файл не тронут");
        // без мастер-ключа — прежнее поведение (открытый файл)
        let d2 = tempfile::tempdir().unwrap();
        let _ = load_protected(d2.path(), None);
        assert!(d2.path().join("mobile_self.json").exists());
    }

    #[test]
    fn rekey_keeps_the_computers_identity_and_wipes_plain_copy() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = ([4u8; 32], [5u8; 32]);
        let before = load_protected(dir.path(), Some(&old)).ed_pub;
        rekey(dir.path(), Some(&old), &new).unwrap();
        assert_eq!(load_protected(dir.path(), Some(&new)).ed_pub, before, "личность компьютера не меняется");
        rekey(dir.path(), Some(&old), &new).unwrap(); // повтор безопасен

        // открытый файл прежних версий переносится и затирается
        let d2 = tempfile::tempdir().unwrap();
        let plain_pub = load_or_create(&d2.path().join("mobile_self.json")).ed_pub;
        assert!(d2.path().join("mobile_self.json").exists());
        rekey(d2.path(), None, &new).unwrap();
        assert!(!d2.path().join("mobile_self.json").exists());
        assert_eq!(load_protected(d2.path(), Some(&new)).ed_pub, plain_pub);

        // чужой ключ: ничего не меняется, ошибка
        let d3 = tempfile::tempdir().unwrap();
        let _ = load_protected(d3.path(), Some(&[9u8; 32]));
        let raw = std::fs::read(d3.path().join("mobile_self.enc")).unwrap();
        assert!(rekey(d3.path(), Some(&old), &new).is_err());
        assert_eq!(std::fs::read(d3.path().join("mobile_self.enc")).unwrap(), raw);
    }

    #[test]
    fn plain_mode_is_explicit_and_revoked_by_lock() {
        let holder = Mutex::new(KeyHolder::default());
        assert!(keys_from(&holder, |_| panic!("без разрешения ключи не создаются")).is_none());
        holder.lock().unwrap().plain_mode = true;
        let mut asked_without_master = false;
        let k = keys_from(&holder, |m| {
            asked_without_master = m.is_none();
            make_keys([1; 32], [2; 32])
        });
        assert!(k.is_some() && asked_without_master);
        drop(k);
        holder.lock().unwrap().revoke();
        assert!(keys_from(&holder, |_| panic!("после отзыва открытый режим выключен")).is_none());
    }
}
