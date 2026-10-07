//! Запись файлов с секретами (ключи, пароли, токены).
//!
//! Файл создаётся СРАЗУ с правами только для владельца (0600): окна, когда он уже есть, но ещё читаем другими, нет. Пишется во временный файл
//! рядом и переименовывается поверх старого: при сбое не остаётся половинчатого файла, а ссылка на чужой файл на месте старого не открывается.
//! Ошибка установки прав — ошибка записи, а не молчаливо проглоченная мелочь.
use std::io::Write;
use std::path::Path;

/// Записать `data` в `path` так, чтобы читать мог только владелец.
pub fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    // a name no other writer uses (also not another thread of this process): two writers must never share a temporary file
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = path.with_file_name(format!(".{name}.tmp{}-{}", std::process::id(), COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let _ = std::fs::remove_file(&tmp);
    let mut f = open_new_private(&tmp)?;
    let res = f.write_all(data).and_then(|_| f.sync_all());
    drop(f);
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Read a secret that is created once: if it is there and `valid`, that is the secret (a file left readable by others is made
/// private again); if not, ONE caller creates it — the file is created exclusively, so callers racing for the first use
/// (threads or processes) all end up with the same bytes instead of each overwriting the other's secret.
pub fn read_or_create_private(path: &Path, make: &dyn Fn() -> Vec<u8>, valid: &dyn Fn(&[u8]) -> bool) -> std::io::Result<Vec<u8>> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    for _ in 0..40 {
        if let Ok(bytes) = std::fs::read(path) {
            if valid(&bytes) {
                tighten(path);
                return Ok(bytes);
            }
        }
        match open_new_private(path) {
            Ok(mut f) => {
                let data = make();
                f.write_all(&data).and_then(|_| f.sync_all())?;
                return Ok(data);
            }
            // someone else is creating it right now (or an unusable file is there): look again shortly
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => return Err(e),
        }
    }
    // a file that stays unusable (damaged, too short) is replaced
    let data = make();
    write_private(path, &data)?;
    Ok(data)
}

#[cfg(unix)]
fn tighten(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(m) = std::fs::metadata(path) {
        if m.permissions().mode() & 0o077 != 0 {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

#[cfg(not(unix))]
fn tighten(_path: &Path) {}

#[cfg(unix)]
fn open_new_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(path)
}

#[cfg(not(unix))]
fn open_new_private(path: &Path) -> std::io::Result<std::fs::File> {
    // На Windows права наследуются от каталога данных пользователя (профиль), отдельного режима 0600 нет.
    std::fs::OpenOptions::new().write(true).create_new(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callers_racing_for_the_first_use_all_get_the_same_secret() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("secret");
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let p = p.clone();
                std::thread::spawn(move || {
                    read_or_create_private(&p, &|| {
                        let mut v = vec![0u8; 32];
                        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut v);
                        v
                    }, &|b| b.len() == 32)
                    .unwrap()
                })
            })
            .collect();
        let all: Vec<Vec<u8>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(all.windows(2).all(|w| w[0] == w[1]), "every caller got the same secret");
        assert_eq!(std::fs::read(&p).unwrap(), all[0], "and it is the one on disk");
    }

    #[cfg(unix)]
    #[test]
    fn a_secret_left_readable_by_others_is_made_private_when_it_is_read() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("secret");
        std::fs::write(&p, b"0123456789abcdef0123456789abcdef").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        let got = read_or_create_private(&p, &|| vec![1u8; 32], &|b| b.len() == 32).unwrap();
        assert_eq!(got, b"0123456789abcdef0123456789abcdef".to_vec());
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn a_new_file_is_private_and_holds_the_data() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub/secret.key");
        write_private(&p, b"hunter2").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"hunter2");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_old_loose_file_is_replaced_by_a_private_one_and_nothing_is_left_behind() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("k");
        std::fs::write(&p, "old").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&p, b"new").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"new");
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600, "the loose mode of the old file does not survive");
        let left: Vec<_> = std::fs::read_dir(d.path()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 1, "no temporary files: {left:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_the_place_of_the_file_is_not_followed() {
        let d = tempfile::tempdir().unwrap();
        let victim = d.path().join("victim");
        std::fs::write(&victim, "keep").unwrap();
        let p = d.path().join("k");
        std::os::unix::fs::symlink(&victim, &p).unwrap();
        write_private(&p, b"new").unwrap();
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep", "what the link pointed to is untouched");
        assert_eq!(std::fs::read(&p).unwrap(), b"new");
        assert!(!std::fs::symlink_metadata(&p).unwrap().file_type().is_symlink(), "the link itself was replaced");
    }

    #[cfg(unix)]
    #[test]
    fn an_unwritable_directory_is_an_error() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        assert!(write_private(&d.path().join("k"), b"x").is_err());
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}
