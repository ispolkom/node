//! Файлы переписки (принятые, отправляемые, части передач) лежат на диске только запечатанными ключом защитной среды.
//!
//! Формат: `"YFV1" || id(16) || блок₀ || блок₁ …`, блок = `nonce(12) || AES-256-GCM(до BLOCK байт)`; к проверке подлинности
//! привязаны номер блока и случайный номер файла (блоки нельзя переставлять или переносить между файлами). Размер блока равен
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
const HEAD: usize = 4 + 16;
const OVERHEAD: usize = 12 + 16;

static KEY: RwLock<Option<Zeroizing<[u8; 32]>>> = RwLock::new(None);

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

fn read_id(f: &mut std::fs::File) -> std::io::Result<[u8; 16]> {
    let mut h = [0u8; HEAD];
    f.seek(SeekFrom::Start(0))?;
    f.read_exact(&mut h)?;
    if &h[..4] != MAGIC {
        return Err(bad("файл не запечатан"));
    }
    let mut id = [0u8; 16];
    id.copy_from_slice(&h[4..]);
    Ok(id)
}

/// Создать пустой запечатанный файл под `plain_size` открытых байт: блоки заполняются по мере прихода (в любом порядке).
pub fn create(path: &Path, plain_size: u64) -> std::io::Result<()> {
    cipher()?; // заперто — не создаём
    let mut head = Vec::with_capacity(HEAD);
    head.extend_from_slice(MAGIC);
    let mut id = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut id);
    head.extend_from_slice(&id);
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
    let c = cipher()?;
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    let id = read_id(&mut f)?;
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
    let id = read_id(&mut f)?;
    f.seek(SeekFrom::Start(HEAD as u64 + index * (BLOCK + OVERHEAD) as u64))?;
    let mut buf = vec![0u8; plain + OVERHEAD];
    f.read_exact(&mut buf)?;
    c.decrypt(Nonce::from_slice(&buf[..12]), Payload { msg: &buf[12..], aad: &aad(&id, index) })
        .map_err(|_| bad("блок не открывается: файл повреждён или записан не полностью"))
}

/// Записать файл целиком (атомарно: временный файл рядом и переименование).
pub fn write_all(path: &Path, plain: &[u8]) -> std::io::Result<()> {
    let c = cipher()?;
    let mut id = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut id);
    let mut out = Vec::with_capacity(sealed_len(plain.len() as u64) as usize);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&id);
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
    let total = plain_len(path)?;
    let mut out = Vec::with_capacity(total as usize);
    for i in 0..total.div_ceil(BLOCK as u64) {
        out.extend_from_slice(&read_block(path, i)?);
    }
    Ok(out)
}

/// Прочитать диапазон открытых байт `[offset, offset+len)` (за концом файла — короче).
pub fn read_range(path: &Path, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
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

    #[test]
    fn sizes_are_consistent() {
        for n in [0u64, 1, 699, 700, 701, 1400, 123456] {
            assert_eq!(plain_from_sealed(sealed_len(n)), Some(n), "n={n}");
        }
    }
}
