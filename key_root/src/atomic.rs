//! Writing that can fail without harm: the original bytes are replaced only by a file that has been written, flushed and read back
//! and passed the caller's own check. Backups are copies that are never overwritten and never removed by this code.
use crate::error::{KeyRootError, Result};
use rand::RngCore;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

fn temp_name(path: &Path) -> PathBuf {
    let mut tag = [0u8; 6];
    rand::rngs::OsRng.fill_bytes(&mut tag);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.tmp-{}", hex::encode(tag)))
}

/// Replace `path` with `bytes` (mode 0600) only if the written file passes `verify`. On any failure the original is untouched
/// and the temporary file is removed.
pub fn write_atomic(path: &Path, bytes: &[u8], verify: impl FnOnce(&[u8]) -> bool) -> Result<()> {
    let tmp = temp_name(path);
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        let back = fs::read(&tmp)?;
        if back != bytes || !verify(&back) {
            return Err(KeyRootError::Corrupt);
        }
        fs::rename(&tmp, path)?;
        if let Some(dir) = path.parent() {
            if let Ok(d) = fs::File::open(dir) {
                let _ = d.sync_all();
            }
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Create `path` with `bytes` only if nothing is there (never overwrites), mode 0600, flushed.
pub fn create_new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let result = file.write_all(bytes).and_then(|_| file.sync_all());
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result.map_err(Into::into)
}

/// Copy `path` to `<name>.<label>-<counter>` next to it, exclusively (an earlier backup is never overwritten). Returns the copy.
pub fn backup_copy(path: &Path, label: &str) -> Result<PathBuf> {
    let bytes = fs::read(path)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    for n in 0..1000u32 {
        let target = path.with_file_name(format!("{name}.{label}-{n:03}"));
        match create_new_file(&target, &bytes) {
            Ok(()) => return Ok(target),
            Err(KeyRootError::Io) | Err(KeyRootError::PermissionDenied) if target.exists() => {
                continue
            }
            Err(e) => return Err(e),
        }
    }
    Err(KeyRootError::Io)
}
