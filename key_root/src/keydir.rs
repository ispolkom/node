//! The key directory (`~/.yandi_keys`) and the files in it: opened only if they are really ours and private.
use crate::error::{KeyRootError, Result};
use std::fs::{self, DirBuilder};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub(crate) fn euid() -> u32 {
    unsafe { libc::geteuid() }
}

pub struct KeyDir {
    path: PathBuf,
}

impl KeyDir {
    /// Create the directory 0700, or accept an existing one that is a real directory of this user. A directory that is only too
    /// open (0755 is what earlier versions left) is tightened to 0700; anything else strange — a link, another owner, not a
    /// directory — is refused and left alone.
    pub fn open(path: impl Into<PathBuf>) -> Result<KeyDir> {
        let path = path.into();
        match fs::symlink_metadata(&path) {
            Ok(meta) => {
                if meta.file_type().is_symlink() || !meta.is_dir() || meta.uid() != euid() {
                    return Err(KeyRootError::PermissionDenied);
                }
                if meta.mode() & 0o077 != 0 {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                DirBuilder::new().mode(0o700).create(&path)?;
            }
            Err(e) => return Err(e.into()),
        }
        Ok(KeyDir { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    pub fn auth_file(&self) -> PathBuf {
        self.file("auth.json")
    }

    pub fn identity_file(&self, port: u16) -> PathBuf {
        self.file(&format!("node_identity_{port}.json"))
    }

    /// Names of any identity-like files here (for any port, including backups), so that "nothing found" can be told from "something
    /// I cannot open".
    pub fn identity_artifacts(&self) -> Vec<String> {
        let mut found = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.path) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("node_identity_") {
                    found.push(name);
                }
            }
        }
        found.sort();
        found
    }
}

/// A key file must be a regular file (not a link) of this user that others cannot read.
pub fn check_private_file(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink()
        || !meta.is_file()
        || meta.uid() != euid()
        || meta.mode() & 0o077 != 0
    {
        return Err(KeyRootError::PermissionDenied);
    }
    Ok(())
}

/// Read a key file that has passed `check_private_file`; the read itself does not follow links either.
pub fn read_private_file(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    check_private_file(path)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let mut bytes = Vec::new();
    file.take(1 << 20).read_to_end(&mut bytes)?;
    Ok(bytes)
}
