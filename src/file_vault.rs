//! Файлы переписки (принятые, отправляемые, части передач) лежат на диске только запечатанными ключом защитной среды.
//!
//! Формат: `"YFV1" || id(16) || nonce(12) || tag(16) || блок₀ || блок₁ …`, блок = `nonce(12) || AES-256-GCM(до BLOCK байт)`;
//! к проверке подлинности привязаны номер блока и случайный номер файла (блоки нельзя переставлять или переносить между файлами).
//! `tag` в заголовке — AEAD над пустым текстом с id и полным открытым размером файла: усечение (даже по границе блока) или
//! удлинение файла не проходит проверку. Размер блока равен
//! размеру куска передачи, поэтому кусок, пришедший не по порядку, записывается на своё место без перезаписи остального, а
//! отправка читает ровно нужный блок.
//!
//! Ключа нет, пока среда заперта: читать и писать нечем (файлы без пароля — просто набор шума).
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::RwLock;
use zeroize::Zeroizing;

/// Открытых байт в блоке (= размер куска передачи файла).
pub const BLOCK: usize = 700;
const MAGIC: &[u8; 4] = b"YFV1";
const HEAD: usize = 4 + 16 + 12 + 16;
const OVERHEAD: usize = 12 + 16;

static KEY: RwLock<Option<Zeroizing<[u8; 32]>>> = RwLock::new(None);

/// Барьер файловых операций: запись блока/создание файла держат его в общем режиме на всё время ввода-вывода, перенос под новый ключ
/// (`rekey_dir`) — в исключительном. Так перенос не пропускает блок, который в этот момент пишет старая операция со старым ключом,
/// и она не пишет в уже заменённый файл.
static IO_BARRIER: RwLock<()> = RwLock::new(());

fn io_shared() -> std::sync::RwLockReadGuard<'static, ()> {
    IO_BARRIER.read().unwrap_or_else(|e| e.into_inner())
}

/// Ключ выдан (вход владельца или старт узла): файлы читаются и пишутся.
pub fn unlock(key: [u8; 32]) {
    *KEY.write().unwrap_or_else(|e| e.into_inner()) = Some(Zeroizing::new(key));
}

/// Блокировка: ключ стирается из памяти.
pub fn lock() {
    *KEY.write().unwrap_or_else(|e| e.into_inner()) = None;
}

pub fn is_unlocked() -> bool {
    key().is_some()
}

fn key() -> Option<[u8; 32]> {
    if let Some(k) = KEY.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return Some(**k);
    }
    // в тестах (нет закрытой среды) — постоянный тестовый ключ
    #[cfg(test)]
    return Some([0x5a; 32]);
    // платформа без закрытой среды (Windows): ключ лежит отдельным закрытым файлом установки — защита слабее, чем у среды
    #[cfg(all(not(test), not(unix)))]
    return install_key();
    #[cfg(all(not(test), unix))]
    None
}

#[cfg(all(not(test), not(unix)))]
fn install_key() -> Option<[u8; 32]> {
    let p = crate::util::data_dir::data_dir().join("files_key.bin");
    let make = || {
        let mut k = vec![0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut k);
        k
    };
    let bytes = crate::util::private_file::read_or_create_private(&p, &make, &|b| b.len() == 32).ok()?;
    let mut k = [0u8; 32];
    k.copy_from_slice(&bytes);
    Some(k)
}

fn locked<T>() -> std::io::Result<T> {
    Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "закрытая среда заперта: файлы переписки недоступны"))
}

fn cipher() -> std::io::Result<Aes256Gcm> {
    let Some(k) = key() else { return locked() };
    Aes256Gcm::new_from_slice(&k).map_err(|e| std::io::Error::other(e.to_string()))
}

fn aad(id: &[u8; 16], index: u64) -> [u8; 24] {
    let mut a = [0u8; 24];
    a[..16].copy_from_slice(id);
    a[16..].copy_from_slice(&index.to_be_bytes());
    a
}

/// Внешний контекст файла: имя без служебных суффиксов `.part`/`.rekey.tmp`. Входит в AAD заголовка, поэтому целый контейнер,
/// скопированный под имя другого вложения, не открывается (подмена вложения B под видом A невозможна без ключа).
fn context_of(path: &Path) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    for suffix in [".rekey.tmp", ".part"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            name = stripped.to_string();
        }
    }
    Sha256::digest(name.as_bytes()).into()
}

fn head_aad(id: &[u8; 16], total: u64, ctx: &[u8; 32]) -> Vec<u8> {
    let mut a = Vec::with_capacity(56);
    a.extend_from_slice(id);
    a.extend_from_slice(&total.to_be_bytes());
    a.extend_from_slice(ctx);
    a
}

/// Заголовок файла для открытого размера `total`: magic, id, nonce, tag.
fn make_head(c: &Aes256Gcm, total: u64, path: &Path) -> std::io::Result<Vec<u8>> {
    let mut id = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut id);
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let tag = c
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: b"", aad: &head_aad(&id, total, &context_of(path)) })
        .map_err(|_| bad("шифрование заголовка"))?;
    let mut head = Vec::with_capacity(HEAD);
    head.extend_from_slice(MAGIC);
    head.extend_from_slice(&id);
    head.extend_from_slice(&nonce);
    head.extend_from_slice(&tag);
    Ok(head)
}

fn bad(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg.to_string())
}

/// Размер запечатанного файла для `plain` открытых байт.
pub fn sealed_len(plain: u64) -> u64 {
    let blocks = plain.div_ceil(BLOCK as u64);
    HEAD as u64 + plain + blocks * OVERHEAD as u64
}

/// Открытый размер по размеру запечатанного файла.
fn plain_from_sealed(len: u64) -> Option<u64> {
    let body = len.checked_sub(HEAD as u64)?;
    let blocks = body.div_ceil((BLOCK + OVERHEAD) as u64);
    body.checked_sub(blocks * OVERHEAD as u64)
}

/// Открытый размер файла.
pub fn plain_len(path: &Path) -> std::io::Result<u64> {
    let len = std::fs::metadata(path)?.len();
    plain_from_sealed(len).ok_or_else(|| bad("файл переписки повреждён"))
}

/// Запечатан ли файл (по заголовку).
pub fn is_sealed(path: &Path) -> bool {
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut m = [0u8; 4];
    f.read_exact(&mut m).is_ok() && &m == MAGIC
}

/// Запечатан ли файл: `Ok(false)` — файл не нашего формата (в том числе короче заголовка); `Err` — прочитать не удалось (это не «чужой файл»).
fn sealed_state(path: &Path) -> std::io::Result<bool> {
    let mut f = std::fs::File::open(path)?;
    let mut m = [0u8; 4];
    match f.read_exact(&mut m) {
        Ok(()) => Ok(&m == MAGIC),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e),
    }
}

/// Прочитать заголовок и проверить его против фактического размера файла (усечение/удлинение не проходит).
fn read_id(f: &mut std::fs::File, c: &Aes256Gcm, path: &Path) -> std::io::Result<[u8; 16]> {
    let mut h = [0u8; HEAD];
    f.seek(SeekFrom::Start(0))?;
    f.read_exact(&mut h)?;
    if &h[..4] != MAGIC {
        return Err(bad("файл не запечатан"));
    }
    let total = plain_from_sealed(f.metadata()?.len()).ok_or_else(|| bad("файл переписки повреждён"))?;
    let mut id = [0u8; 16];
    id.copy_from_slice(&h[4..20]);
    c.decrypt(Nonce::from_slice(&h[20..32]), Payload { msg: &h[32..], aad: &head_aad(&id, total, &context_of(path)) })
        .map_err(|_| bad("заголовок файла не проходит проверку: размер изменён или файл повреждён"))?;
    Ok(id)
}

/// Проверить заголовок файла (нужно и пустому файлу, у которого блоков для проверки нет).
fn check_head(path: &Path, c: &Aes256Gcm) -> std::io::Result<()> {
    let mut f = std::fs::File::open(path)?;
    read_id(&mut f, c, path).map(|_| ())
}

/// Создать пустой запечатанный файл под `plain_size` открытых байт: блоки заполняются по мере прихода (в любом порядке).
pub fn create(path: &Path, plain_size: u64) -> std::io::Result<()> {
    let _io = io_shared();
    let head = make_head(&cipher()?, plain_size, path)?;
    crate::util::private_file::write_private(path, &head)?;
    let f = std::fs::OpenOptions::new().write(true).open(path)?;
    f.set_len(sealed_len(plain_size))?;
    Ok(())
}

/// Записать блок номер `index` (все, кроме последнего, ровно BLOCK байт).
pub fn write_block(path: &Path, index: u64, data: &[u8]) -> std::io::Result<()> {
    if data.is_empty() || data.len() > BLOCK {
        return Err(bad("неверный размер блока"));
    }
    let _io = io_shared();
    let c = cipher()?;
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    let id = read_id(&mut f, &c, path)?;
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let ct = c
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: data, aad: &aad(&id, index) })
        .map_err(|_| bad("шифрование блока"))?;
    f.seek(SeekFrom::Start(HEAD as u64 + index * (BLOCK + OVERHEAD) as u64))?;
    f.write_all(&nonce)?;
    f.write_all(&ct)?;
    Ok(())
}

/// Прочитать и расшифровать блок номер `index`.
pub fn read_block(path: &Path, index: u64) -> std::io::Result<Vec<u8>> {
    let c = cipher()?;
    let total = plain_len(path)?;
    let blocks = total.div_ceil(BLOCK as u64);
    if index >= blocks {
        return Err(bad("нет такого блока"));
    }
    let plain = if index + 1 == blocks { (total - index * BLOCK as u64) as usize } else { BLOCK };
    let mut f = std::fs::File::open(path)?;
    let id = read_id(&mut f, &c, path)?;
    f.seek(SeekFrom::Start(HEAD as u64 + index * (BLOCK + OVERHEAD) as u64))?;
    let mut buf = vec![0u8; plain + OVERHEAD];
    f.read_exact(&mut buf)?;
    c.decrypt(Nonce::from_slice(&buf[..12]), Payload { msg: &buf[12..], aad: &aad(&id, index) })
        .map_err(|_| bad("блок не открывается: файл повреждён или записан не полностью"))
}

fn cipher_of(key: &[u8; 32]) -> std::io::Result<Aes256Gcm> {
    Aes256Gcm::new_from_slice(key).map_err(|e| std::io::Error::other(e.to_string()))
}

fn read_block_with(c: &Aes256Gcm, path: &Path, index: u64) -> std::io::Result<Vec<u8>> {
    let total = plain_len(path)?;
    let blocks = total.div_ceil(BLOCK as u64);
    if index >= blocks {
        return Err(bad("нет такого блока"));
    }
    let plain = if index + 1 == blocks { (total - index * BLOCK as u64) as usize } else { BLOCK };
    let mut f = std::fs::File::open(path)?;
    let id = read_id(&mut f, c, path)?;
    f.seek(SeekFrom::Start(HEAD as u64 + index * (BLOCK + OVERHEAD) as u64))?;
    let mut buf = vec![0u8; plain + OVERHEAD];
    f.read_exact(&mut buf)?;
    c.decrypt(Nonce::from_slice(&buf[..12]), Payload { msg: &buf[12..], aad: &aad(&id, index) })
        .map_err(|_| bad("блок не открывается: файл повреждён или записан не полностью"))
}

/// Блок целиком из нулевых байт (nonce и шифртекст): в него ещё ничего не записано.
fn block_is_unwritten(path: &Path, index: u64, total: u64) -> std::io::Result<bool> {
    let blocks = total.div_ceil(BLOCK as u64);
    let plain = if index + 1 == blocks { (total - index * BLOCK as u64) as usize } else { BLOCK };
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(HEAD as u64 + index * (BLOCK + OVERHEAD) as u64))?;
    let mut buf = vec![0u8; plain + OVERHEAD];
    f.read_exact(&mut buf)?;
    Ok(buf.iter().all(|b| *b == 0))
}

/// Перезапечатать один файл с `old` на `new` (блок за блоком во временный файл, затем замена). `Ok(false)` — файл уже под `new`.
fn rekey_file(path: &Path, old: &[u8; 32], new: &[u8; 32]) -> std::io::Result<bool> {
    let (co, cn) = (cipher_of(old)?, cipher_of(new)?);
    {
        let mut f = std::fs::File::open(path)?;
        if read_id(&mut f, &cn, path).is_ok() {
            return Ok(false);
        }
        read_id(&mut f, &co, path)?; // не открывается ни тем, ни другим — ошибка
    }
    let total = plain_len(path)?;
    let tmp = path.with_extension("rekey.tmp");
    let _ = std::fs::remove_file(&tmp);
    let run = || -> std::io::Result<()> {
        std::fs::write(&tmp, make_head(&cn, total, path)?)?;
        let f = std::fs::OpenOptions::new().write(true).open(&tmp)?;
        f.set_len(sealed_len(total))?;
        drop(f);
        let mut tf = std::fs::OpenOptions::new().read(true).write(true).open(&tmp)?;
        let id = read_id(&mut tf, &cn, path)?;
        for i in 0..total.div_ceil(BLOCK as u64) {
            // ещё не полученный блок незавершённой передачи — нули (файл создаётся полного размера): остаётся нулями под новым
            // ключом, чтобы передачу можно было продолжить; открыть его невозможно, и это не повреждение
            if block_is_unwritten(path, i, total)? {
                continue;
            }
            let data = read_block_with(&co, path, i)?;
            let mut nonce = [0u8; 12];
            rand::rngs::OsRng.fill_bytes(&mut nonce);
            let ct = cn
                .encrypt(Nonce::from_slice(&nonce), Payload { msg: &data, aad: &aad(&id, i) })
                .map_err(|_| bad("шифрование блока"))?;
            tf.seek(SeekFrom::Start(HEAD as u64 + i * (BLOCK + OVERHEAD) as u64))?;
            tf.write_all(&nonce)?;
            tf.write_all(&ct)?;
        }
        tf.sync_all()?;
        std::fs::rename(&tmp, path)
    };
    let r = run();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r.map(|_| true)
}

/// Перезапечатать все файлы каталога с ключа `old` на `new` (рекурсивно); возвращает число файлов, которые не удалось перенести.
/// Файлы под `new` не трогаются, не запечатанные (чужие/частичные без заголовка) пропускаются.
pub fn rekey_dir(dir: &Path, old: &[u8; 32], new: &[u8; 32]) -> usize {
    // исключительный режим: дожидаемся начатых записей и не пускаем новые, пока каталог переносится
    let _io = IO_BARRIER.write().unwrap_or_else(|e| e.into_inner());
    rekey_dir_locked(dir, old, new)
}

fn rekey_dir_locked(dir: &Path, old: &[u8; 32], new: &[u8; 32]) -> usize {
    let mut failed = 0usize;
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        // нет каталога — переносить нечего; иная ошибка — перенос не выполнен, метку миграции снимать нельзя
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return 0,
        Err(_) => return 1,
    };
    for e in rd {
        // ошибка итератора, типа записи, открытия или заголовка — не «пропустить», а неудавшийся перенос: метку миграции снимать нельзя
        let Ok(e) = e else {
            failed += 1;
            continue;
        };
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => failed += rekey_dir_locked(&p, old, new),
            Ok(t) if t.is_file() => match sealed_state(&p) {
                Ok(false) => {}
                Ok(true) => {
                    if rekey_file(&p, old, new).is_err() {
                        failed += 1;
                    }
                }
                Err(_) => failed += 1,
            },
            Ok(_) => {}
            Err(_) => failed += 1,
        }
    }
    failed
}

/// Записать файл целиком (атомарно: временный файл рядом и переименование).
pub fn write_all(path: &Path, plain: &[u8]) -> std::io::Result<()> {
    let c = cipher()?;
    let mut out = make_head(&c, plain.len() as u64, path)?;
    out.reserve(sealed_len(plain.len() as u64) as usize);
    let mut id = [0u8; 16];
    id.copy_from_slice(&out[4..20]);
    for (i, part) in plain.chunks(BLOCK).enumerate() {
        let mut nonce = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let ct = c
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: part, aad: &aad(&id, i as u64) })
            .map_err(|_| bad("шифрование блока"))?;
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
    }
    crate::util::private_file::write_private(path, &out)
}

/// Прочитать файл целиком.
pub fn read_all(path: &Path) -> std::io::Result<Vec<u8>> {
    check_head(path, &cipher()?)?;
    let total = plain_len(path)?;
    let mut out = Vec::with_capacity(total as usize);
    for i in 0..total.div_ceil(BLOCK as u64) {
        out.extend_from_slice(&read_block(path, i)?);
    }
    Ok(out)
}

/// Прочитать диапазон открытых байт `[offset, offset+len)` (за концом файла — короче).
pub fn read_range(path: &Path, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
    check_head(path, &cipher()?)?;
    let total = plain_len(path)?;
    if offset >= total || len == 0 {
        return Ok(Vec::new());
    }
    let end = total.min(offset + len as u64);
    let mut out = Vec::with_capacity((end - offset) as usize);
    let (first, last) = (offset / BLOCK as u64, (end - 1) / BLOCK as u64);
    for i in first..=last {
        let block = read_block(path, i)?;
        let from = if i == first { (offset - i * BLOCK as u64) as usize } else { 0 };
        let to = if i == last { (end - i * BLOCK as u64) as usize } else { block.len() };
        out.extend_from_slice(&block[from..to]);
    }
    Ok(out)
}

/// Последовательная запись запечатанного файла кусками любой длины (куски ложатся в блоки).
pub struct Writer {
    path: std::path::PathBuf,
    buf: Vec<u8>,
    index: u64,
    total: u64,
    expected: u64,
}

impl Writer {
    /// Создать файл под `plain_size` открытых байт.
    pub fn create(path: &Path, plain_size: u64) -> std::io::Result<Self> {
        create(path, plain_size)?;
        Ok(Self { path: path.to_path_buf(), buf: Vec::with_capacity(BLOCK * 2), index: 0, total: 0, expected: plain_size })
    }

    pub fn push(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.total += data.len() as u64;
        if self.total > self.expected {
            return Err(bad("записано больше заявленного размера"));
        }
        self.buf.extend_from_slice(data);
        while self.buf.len() >= BLOCK {
            write_block(&self.path, self.index, &self.buf[..BLOCK])?;
            self.buf.drain(..BLOCK);
            self.index += 1;
        }
        Ok(())
    }

    /// Дописать хвост; размер должен совпасть с заявленным.
    pub fn finish(mut self) -> std::io::Result<()> {
        if !self.buf.is_empty() {
            write_block(&self.path, self.index, &self.buf)?;
            self.buf.clear();
        }
        if self.total != self.expected {
            return Err(bad("записано меньше заявленного размера"));
        }
        Ok(())
    }
}

/// Собрать запечатанный файл из отдельных запечатанных частей (по порядку): куски любой длины ложатся в блоки.
/// Размер собранного должен совпасть с `plain_size`, иначе файл не создаётся.
pub fn assemble(parts: &[std::path::PathBuf], out: &Path, plain_size: u64) -> std::io::Result<()> {
    let run = || -> std::io::Result<()> {
        let mut w = Writer::create(out, plain_size)?;
        for part in parts {
            w.push(&read_all(part)?)?;
        }
        w.finish()
    };
    let r = run();
    if r.is_err() {
        let _ = std::fs::remove_file(out);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_roundtrip_ranges_and_sizes() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.bin");
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        write_all(&p, &data).unwrap();
        assert!(is_sealed(&p));
        assert_eq!(plain_len(&p).unwrap(), 5000);
        assert_eq!(read_all(&p).unwrap(), data);
        assert_eq!(read_range(&p, 690, 30).unwrap(), data[690..720]);
        assert_eq!(read_range(&p, 4990, 100).unwrap(), data[4990..]);
        assert!(read_range(&p, 6000, 10).unwrap().is_empty());
        let raw = std::fs::read(&p).unwrap();
        assert!(!raw.windows(32).any(|w| w == &data[100..132]), "открытого содержимого на диске нет");
        // пустой файл
        let e = d.path().join("empty");
        write_all(&e, b"").unwrap();
        assert_eq!(plain_len(&e).unwrap(), 0);
        assert!(read_all(&e).unwrap().is_empty());
    }

    #[test]
    fn blocks_may_arrive_in_any_order_and_missing_ones_fail() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("in.part");
        let data: Vec<u8> = (0..(BLOCK * 3 + 100)).map(|i| (i % 199) as u8).collect();
        create(&p, data.len() as u64).unwrap();
        assert_eq!(plain_len(&p).unwrap(), data.len() as u64);
        write_block(&p, 3, &data[BLOCK * 3..]).unwrap();
        write_block(&p, 0, &data[..BLOCK]).unwrap();
        assert!(read_all(&p).is_err(), "не все блоки на месте");
        write_block(&p, 2, &data[BLOCK * 2..BLOCK * 3]).unwrap();
        write_block(&p, 1, &data[BLOCK..BLOCK * 2]).unwrap();
        assert_eq!(read_all(&p).unwrap(), data);
    }

    #[test]
    fn blocks_cannot_be_swapped_or_moved_between_files() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        let data = vec![7u8; BLOCK * 2];
        write_all(&a, &data).unwrap();
        write_all(&b, &data).unwrap();
        // блок 1 файла a — на место блока 0 того же файла
        let mut raw = std::fs::read(&a).unwrap();
        let sz = BLOCK + OVERHEAD;
        let blk1 = raw[HEAD + sz..HEAD + 2 * sz].to_vec();
        raw[HEAD..HEAD + sz].copy_from_slice(&blk1);
        std::fs::write(&a, &raw).unwrap();
        assert!(read_block(&a, 0).is_err(), "перестановка блоков не проходит");
        // блок из другого файла
        let rb = std::fs::read(&b).unwrap();
        let mut ra = std::fs::read(&a).unwrap();
        ra[HEAD + sz..HEAD + 2 * sz].copy_from_slice(&rb[HEAD + sz..HEAD + 2 * sz]);
        std::fs::write(&a, &ra).unwrap();
        assert!(read_block(&a, 1).is_err(), "блок чужого файла не проходит");
    }

    #[test]
    fn parts_of_any_size_assemble_into_blocks() {
        let d = tempfile::tempdir().unwrap();
        let data: Vec<u8> = (0..4000u32).map(|i| (i % 13) as u8).collect();
        let mut parts = Vec::new();
        for (i, c) in data.chunks(1111).enumerate() {
            let p = d.path().join(format!("p{i}"));
            write_all(&p, c).unwrap();
            parts.push(p);
        }
        let out = d.path().join("out");
        assemble(&parts, &out, 4000).unwrap();
        assert_eq!(read_all(&out).unwrap(), data);
        let bad_out = d.path().join("bad");
        assert!(assemble(&parts, &bad_out, 3999).is_err(), "размер не совпал");
        assert!(!bad_out.exists());
        assert!(assemble(&parts, &bad_out, 4001).is_err());
    }

    /// Незавершённая передача (часть блоков ещё не получена) переносится на новый ключ без отказа и остаётся достраиваемой.
    #[test]
    fn a_partially_received_file_is_resealed_and_keeps_its_empty_blocks() {
        let d = tempfile::tempdir().unwrap();
        let (old, new) = ([0x5au8; 32], [3u8; 32]);
        let p = d.path().join("in.part");
        let data: Vec<u8> = (0..(BLOCK * 3)).map(|i| (i % 97) as u8).collect();
        create(&p, data.len() as u64).unwrap();
        write_block(&p, 0, &data[..BLOCK]).unwrap();
        assert_eq!(rekey_dir(d.path(), &old, &new), 0, "перенос не отказал из-за неполученных блоков");
        let cn = cipher_of(&new).unwrap();
        assert_eq!(read_block_with(&cn, &p, 0).unwrap(), data[..BLOCK], "полученный блок под новым ключом");
        assert!(block_is_unwritten(&p, 1, data.len() as u64).unwrap(), "неполученный блок остался пустым");
    }

    /// Перенос ждёт начатую запись блока и не стартует поверх неё (общий барьер ввода-вывода).
    #[test]
    fn rekey_waits_for_writes_in_flight() {
        let d = tempfile::tempdir().unwrap();
        let (old, new) = ([0x5au8; 32], [3u8; 32]);
        let path = d.path().to_path_buf();
        let in_flight = io_shared(); // как будто write_block уже идёт
        let (tx, rx) = std::sync::mpsc::channel();
        let h = std::thread::spawn(move || {
            let r = rekey_dir(&path, &old, &new);
            tx.send(r).unwrap();
        });
        assert!(rx.recv_timeout(std::time::Duration::from_millis(200)).is_err(), "перенос ждёт окончания записи");
        drop(in_flight);
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), 0);
        h.join().unwrap();
    }

    /// Файл, который не удалось даже открыть для проверки заголовка, — неудача переноса, а не «чужой файл».
    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_counts_as_a_failed_migration() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let (old, new) = ([0x5au8; 32], [2u8; 32]);
        let p = d.path().join("locked.bin");
        write_all(&p, b"data").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
        let unreadable = std::fs::File::open(&p).is_err(); // под root права не мешают
        let failed = rekey_dir(d.path(), &old, &new);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        if unreadable {
            assert_eq!(failed, 1, "нечитаемый файл — неудача переноса");
        }
        assert_eq!(rekey_dir(d.path(), &old, &new), 0, "после возврата прав перенос проходит");
    }

    /// Целый контейнер, скопированный под имя другого вложения, не открывается; переименование .part → итоговое имя работает.
    #[test]
    fn a_whole_container_copied_under_another_name_does_not_open() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("id_a__report.pdf");
        let b = d.path().join("id_b__photo.jpg");
        write_all(&a, b"secret A").unwrap();
        write_all(&b, b"secret B").unwrap();
        std::fs::copy(&b, &a).unwrap(); // подмена: B под именем A
        assert!(read_all(&a).is_err(), "подменённый контейнер отвергается");
        assert_eq!(read_all(&b).unwrap(), b"secret B");
        let part = d.path().join("id_c__x.bin.part");
        write_all(&part, b"data").unwrap();
        let fin = d.path().join("id_c__x.bin");
        std::fs::rename(&part, &fin).unwrap();
        assert_eq!(read_all(&fin).unwrap(), b"data", "после снятия .part файл читается");
    }

    #[test]
    fn files_are_resealed_from_the_working_key_to_the_owners_key() {
        // общий ключ процесса не трогаем (параллельные проверки): файл пишется тестовым ключом 0x5a, проверка — явными ключами
        let d = tempfile::tempdir().unwrap();
        let (old, new) = ([0x5au8; 32], [2u8; 32]);
        let p = d.path().join("sub").join("a.bin");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let data: Vec<u8> = (0..(BLOCK * 2 + 77)).map(|i| (i % 211) as u8).collect();
        write_all(&p, &data).unwrap();
        let read_with = |key: &[u8; 32]| -> std::io::Result<Vec<u8>> {
            let c = cipher_of(key)?;
            let mut out = Vec::new();
            for i in 0..plain_len(&p)?.div_ceil(BLOCK as u64) {
                out.extend(read_block_with(&c, &p, i)?);
            }
            Ok(out)
        };
        assert_eq!(read_with(&old).unwrap(), data);
        assert_eq!(rekey_dir(d.path(), &old, &new), 0);
        assert!(read_with(&old).is_err(), "под прежним ключом файл больше не открывается");
        assert_eq!(read_with(&new).unwrap(), data);
        assert_eq!(rekey_dir(d.path(), &old, &new), 0, "повтор: файл уже под новым ключом");
        assert_eq!(read_with(&new).unwrap(), data);
    }

    #[test]
    fn truncation_even_at_a_block_boundary_is_detected() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("t");
        write_all(&p, &vec![3u8; BLOCK * 3]).unwrap();
        let raw = std::fs::read(&p).unwrap();
        // отрезан хвост ровно по границе блока
        std::fs::write(&p, &raw[..HEAD + BLOCK + OVERHEAD]).unwrap();
        assert!(read_all(&p).is_err(), "усечение по границе блока");
        assert!(read_range(&p, 0, 10).is_err());
        assert!(read_block(&p, 0).is_err());
        // оставлен один заголовок
        std::fs::write(&p, &raw[..HEAD]).unwrap();
        assert!(read_all(&p).is_err(), "усечение до заголовка");
        // допись лишнего блока
        let mut more = raw.clone();
        more.extend_from_slice(&raw[HEAD..HEAD + BLOCK + OVERHEAD]);
        std::fs::write(&p, &more).unwrap();
        assert!(read_all(&p).is_err(), "удлинение");
        // целый файл читается
        std::fs::write(&p, &raw).unwrap();
        assert_eq!(read_all(&p).unwrap().len(), BLOCK * 3);
    }

    #[test]
    fn sizes_are_consistent() {
        for n in [0u64, 1, 699, 700, 701, 1400, 123456] {
            assert_eq!(plain_from_sealed(sealed_len(n)), Some(n), "n={n}");
        }
    }
}
