//! Linux updater. The single commit point is rename of the `current` symlink.
use crate::release::*;
use anyhow::{bail, ensure, Context, Result};
use ed25519_dalek::VerifyingKey;
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    os::unix::{
        fs::{symlink, PermissionsExt},
        io::AsRawFd,
    },
    path::{Component, Path, PathBuf},
    time::Duration,
};

const MAX_FILE: u64 = 256 * 1024 * 1024;
const MAX_TOTAL: u64 = 512 * 1024 * 1024;
const MAX_UNPACKED: u64 = 1024 * 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub version: String,
    pub seq: u64,
    pub max_seq: u64,
    pub key_id: String,
}

fn bounded(mut r: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    r.by_ref().take(limit + 1).read_to_end(&mut out)?;
    ensure!(out.len() as u64 <= limit, "файл слишком большой");
    Ok(out)
}
fn local_file(dir: &Path, name: &str, limit: u64) -> Result<Vec<u8>> {
    Ok(ReleaseDir::open(dir)?.read(name, limit)?)
}
fn base_url(from: &str) -> Result<url::Url> {
    let mut u = url::Url::parse(from)?;
    ensure!(
        u.scheme() == "https"
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none(),
        "нужна HTTPS-база без credentials/query/fragment"
    );
    if !u.path().ends_with('/') {
        u.set_path(&format!("{}/", u.path()));
    }
    Ok(u)
}
fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
}
async fn download(
    client: &reqwest::Client,
    base: &url::Url,
    name: &str,
    limit: u64,
) -> Result<Vec<u8>> {
    let mut url = base.clone();
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("не URL-база"))?
        .pop_if_empty()
        .push(name);
    let mut r = client.get(url).send().await?.error_for_status()?;
    ensure!(!r.status().is_redirection(), "перенаправления запрещены");
    ensure!(
        r.content_length().unwrap_or(0) <= limit,
        "файл слишком большой"
    );
    let mut out = Vec::new();
    while let Some(chunk) = r.chunk().await? {
        ensure!(
            (out.len() as u64) + chunk.len() as u64 <= limit,
            "файл слишком большой"
        );
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

pub async fn update(from: &str, vk: &VerifyingKey, install: &Path, min_seq: u64) -> Result<State> {
    tokio::time::timeout(
        Duration::from_secs(300),
        prepare_and_install(from, vk, install, min_seq),
    )
    .await
    .context("истекло время обновления")?
}
async fn prepare_and_install(
    from: &str,
    vk: &VerifyingKey,
    install: &Path,
    min_seq: u64,
) -> Result<State> {
    // Snapshot all inputs outside the installation; hash the snapshot, not mutable source paths.
    let snapshot = tempfile::tempdir()?;
    let remote = if from.contains("://") {
        Some(base_url(from)?)
    } else {
        None
    };
    let local = if remote.is_none() { Some(ReleaseDir::open(Path::new(from))?) } else { None };
    let client = client_builder().build()?;
    async fn fetch(
        client: &reqwest::Client,
        remote: &Option<url::Url>,
        local: &Option<ReleaseDir>,
        name: &str,
        limit: u64,
    ) -> Result<Vec<u8>> {
        match remote {
            Some(u) => download(client, u, name, limit).await,
            None => Ok(local.as_ref().context("нет каталога")?.read(name, limit)?),
        }
    }
    let bytes = fetch(&client, &remote, &local, MANIFEST_NAME, 1 << 20).await?;
    let sig = fetch(&client, &remote, &local, SIGNATURE_NAME, 256).await?;
    let m = verify_manifest(&bytes, std::str::from_utf8(&sig)?, vk, min_seq)
        .map_err(|e| anyhow::anyhow!(e))?;
    let mut total = 0u64;
    for f in &m.files {
        ensure!(f.size <= MAX_FILE, "файл слишком большой");
        total = total.checked_add(f.size).context("размер переполнен")?;
        ensure!(total <= MAX_TOTAL, "выпуск слишком большой");
        let data = fetch(&client, &remote, &local, &f.name, f.size).await?;
        fs::write(snapshot.path().join(&f.name), data)?;
    }
    check_files(snapshot.path(), &m).map_err(|e| anyhow::anyhow!(e))?;
    let archives: Vec<_> = m
        .files
        .iter()
        .filter(|f| f.name.ends_with(".tar.gz"))
        .collect();
    ensure!(archives.len() == 1, "нужен ровно один архив tar.gz");
    let parent = install
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let stage = tempfile::Builder::new()
        .prefix(".yandi-stage-")
        .tempdir_in(parent)?;
    unpack(&snapshot.path().join(&archives[0].name), stage.path())?;
    let _lock = lock(install)?;
    let old = generation(install)?;
    let high = old
        .as_ref()
        .map(|p| read_state(p))
        .transpose()?
        .map(|s| s.max_seq)
        .unwrap_or(0)
        .max(min_seq);
    ensure!(m.seq > high, "повтор/откат seq: {} <= {}", m.seq, high);
    let state = State {
        version: m.version,
        seq: m.seq,
        max_seq: high.max(m.seq),
        key_id: m.key_id,
    };
    commit(install, stage, old.as_deref(), &state, false)?;
    Ok(state)
}

fn octal(bytes: &[u8]) -> Result<u64> {
    let s = std::str::from_utf8(bytes)?.trim_matches(|c| c == '\0' || c == ' ');
    Ok(if s.is_empty() {
        0
    } else {
        u64::from_str_radix(s, 8)?
    })
}
fn field(bytes: &[u8]) -> Result<&str> {
    Ok(std::str::from_utf8(
        bytes.split(|b| *b == 0).next().unwrap(),
    )?)
}
fn unpack(archive: &Path, target: &Path) -> Result<()> {
    let mut r = GzDecoder::new(fs::File::open(archive)?).take(MAX_UNPACKED + 1);
    let mut total = 0u64;
    let mut seen = std::collections::HashSet::new();
    loop {
        let mut h = [0u8; 512];
        r.read_exact(&mut h)?;
        if h.iter().all(|b| *b == 0) {
            let mut second = [0u8; 512];
            r.read_exact(&mut second)?;
            ensure!(second.iter().all(|b| *b == 0), "неверный конец tar");
            let mut tail = Vec::new();
            r.by_ref().take(10240).read_to_end(&mut tail)?;
            ensure!(
                r.limit() > 0 && tail.len() < 10240 && tail.iter().all(|b| *b == 0),
                "данные после tar"
            );
            break;
        }
        let sum: u64 = h
            .iter()
            .enumerate()
            .map(|(i, b)| {
                if (148..156).contains(&i) {
                    32
                } else {
                    *b as u64
                }
            })
            .sum();
        ensure!(octal(&h[148..156])? == sum, "tar checksum");
        ensure!(&h[257..262] == b"ustar", "нужен ustar/GNU tar");
        let name = field(&h[..100])?;
        let prefix = if &h[257..263] == b"ustar\0" {
            field(&h[345..500])?
        } else {
            ""
        };
        let full = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        ensure!(
            !full.contains('\\') && !full.chars().any(|c| c.is_control()),
            "неверный путь tar"
        );
        let path = Path::new(&full);
        ensure!(
            !path.is_absolute()
                && path
                    .components()
                    .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
            "опасный путь tar: {full}"
        );
        let clean: PathBuf = path
            .components()
            .filter(|c| matches!(c, Component::Normal(_)))
            .collect();
        ensure!(
            clean
                .components()
                .next()
                .map(|c| c.as_os_str().to_string_lossy().starts_with('.'))
                .unwrap_or(false)
                == false,
            "зарезервированный путь"
        );
        let kind = h[156];
        ensure!(
            kind == b'0' || kind == 0 || kind == b'5',
            "ссылки/устройства/расширения tar запрещены"
        );
        let size = octal(&h[124..136])?;
        total = total.checked_add(size).context("размер tar")?;
        ensure!(
            total <= MAX_UNPACKED && seen.len() < 4096,
            "архив слишком большой"
        );
        if clean.as_os_str().is_empty() {
            ensure!(kind == b'5' && size == 0, "пустой путь");
            continue;
        }
        ensure!(seen.insert(clean.clone()), "повтор пути tar");
        let dest = target.join(&clean);
        if kind == b'5' {
            ensure!(size == 0, "данные каталога");
            fs::create_dir_all(&dest)?;
            fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))?;
        } else {
            if let Some(p) = dest.parent() {
                fs::create_dir_all(p)?;
            }
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&dest)?;
            let n = std::io::copy(&mut r.by_ref().take(size), &mut f)?;
            ensure!(n == size, "обрыв tar");
            let mode = if octal(&h[100..108])? & 0o111 != 0 {
                0o755
            } else {
                0o644
            };
            f.set_permissions(fs::Permissions::from_mode(mode))?;
            f.sync_all()?;
            let pad = (512 - size % 512) % 512;
            let mut padding = vec![0; pad as usize];
            r.read_exact(&mut padding)?;
        }
    }
    ensure!(
        fs::symlink_metadata(target.join("yandi"))?.is_file(),
        "в архиве нет yandi"
    );
    fs::set_permissions(target.join("yandi"), fs::Permissions::from_mode(0o755))?;
    sync_tree(target)?;
    Ok(())
}
fn sync_tree(dir: &Path) -> Result<()> {
    fs::set_permissions(dir, fs::Permissions::from_mode(0o755))?;
    for e in fs::read_dir(dir)? {
        let e = e?;
        if e.file_type()?.is_dir() {
            sync_tree(&e.path())?;
        }
    }
    sync_dir(dir)
}
fn lock(install: &Path) -> Result<fs::File> {
    fs::create_dir_all(install)?;
    let f = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(install.join(".lock"))?;
    ensure!(
        unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } == 0,
        "lock: {}",
        std::io::Error::last_os_error()
    );
    Ok(f)
}
fn generation(install: &Path) -> Result<Option<PathBuf>> {
    match fs::read_link(install.join("current")) {
        Ok(p) => Ok(Some(install.join(p))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn read_state(p: &Path) -> Result<State> {
    Ok(serde_json::from_slice(&fs::read(p.join(".release.json"))?)?)
}
pub fn status(install: &Path) -> Result<Option<State>> {
    generation(install)?.map(|p| read_state(&p)).transpose()
}
fn sync_dir(p: &Path) -> Result<()> {
    fs::File::open(p)?.sync_all()?;
    Ok(())
}
fn commit(
    install: &Path,
    stage: tempfile::TempDir,
    previous: Option<&Path>,
    state: &State,
    interrupt: bool,
) -> Result<()> {
    for (name, target) in [
        ("previous", "current/.previous"),
        ("yandi", "current/yandi"),
        ("yandi-release", "current/yandi-release"),
    ] {
        let p = install.join(name);
        match fs::symlink_metadata(&p) {
            Ok(_) => ensure!(
                fs::read_link(&p)? == Path::new(target),
                "занятое имя установки: {name}"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => symlink(target, p)?,
            Err(e) => return Err(e.into()),
        }
    }
    let mut f = fs::File::create(stage.path().join(".release.json"))?;
    f.write_all(&serde_json::to_vec_pretty(state)?)?;
    f.set_permissions(fs::Permissions::from_mode(0o644))?;
    f.sync_all()?;
    if let Some(p) = previous {
        symlink(fs::canonicalize(p)?, stage.path().join(".previous"))?;
    }
    fs::set_permissions(stage.path(), fs::Permissions::from_mode(0o755))?;
    sync_dir(stage.path())?;
    fs::create_dir_all(install.join(".versions"))?;
    let name = stage.path().file_name().unwrap().to_owned();
    let dest = install.join(".versions").join(&name);
    fs::rename(stage.path(), &dest)?;
    sync_dir(&install.join(".versions"))?;
    // A private symlink avoids stale .next files after SIGKILL.
    let pointer = tempfile::Builder::new()
        .prefix(".yandi-pointer-")
        .tempdir_in(install)?;
    symlink(
        Path::new(".versions").join(&name),
        pointer.path().join("next"),
    )?;
    sync_dir(pointer.path())?;
    if interrupt {
        #[cfg(test)]
        if std::env::var_os("YANDI_TEST_KILL").is_some() {
            unsafe {
                libc::kill(libc::getpid(), libc::SIGKILL);
            }
        }
        bail!("тест: обрыв до commit");
    }
    fs::rename(pointer.path().join("next"), install.join("current"))?;
    sync_dir(install)?;
    Ok(())
}
pub fn rollback(install: &Path) -> Result<State> {
    let _lock = lock(install)?;
    let current = generation(install)?.context("нет установленного выпуска")?;
    let previous = fs::canonicalize(current.join(".previous")).context("нет previous")?;
    let mut state = read_state(&previous)?;
    state.max_seq = state.max_seq.max(read_state(&current)?.max_seq);
    let stage = tempfile::Builder::new()
        .prefix(".yandi-stage-")
        .tempdir_in(
            install
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?;
    copy_tree(&previous, stage.path())?;
    commit(install, stage, Some(&current), &state, false)?;
    Ok(state)
}
fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    copy_tree_inner(src, dst, true)
}
fn copy_tree_inner(src: &Path, dst: &Path, root: bool) -> Result<()> {
    for entry in fs::read_dir(src)? {
        let e = entry?;
        if root && (e.file_name() == ".previous" || e.file_name() == ".release.json") {
            continue;
        }
        let target = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            fs::create_dir(&target)?;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;
            copy_tree_inner(&e.path(), &target, false)?;
        } else {
            ensure!(e.file_type()?.is_file(), "необычный файл previous");
            fs::copy(e.path(), &target)?;
            fs::File::open(target)?.sync_all()?;
        }
    }
    sync_dir(dst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use flate2::{write::GzEncoder, Compression};
    fn archive(dir: &Path, name: &str, kind: u8, content: &[u8]) {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..108].copy_from_slice(b"0000755\0");
        h[124..136].copy_from_slice(format!("{:011o}\0", content.len()).as_bytes());
        h[148..156].fill(b' ');
        h[156] = kind;
        h[257..263].copy_from_slice(b"ustar\0");
        let sum: u64 = h.iter().map(|b| *b as u64).sum();
        h[148..156].copy_from_slice(format!("{:06o}\0 ", sum).as_bytes());
        let mut gz = GzEncoder::new(
            fs::File::create(dir.join("node.tar.gz")).unwrap(),
            Compression::default(),
        );
        gz.write_all(&h).unwrap();
        gz.write_all(content).unwrap();
        gz.write_all(&vec![0; (512 - content.len() % 512) % 512])
            .unwrap();
        gz.write_all(&[0; 1024]).unwrap();
        gz.finish().unwrap();
    }
    fn signed(dir: &Path, seq: u64) -> VerifyingKey {
        let key = SigningKey::from_bytes(&[42; 32]);
        let vk = key.verifying_key();
        let m = build_manifest(
            dir,
            &format!("v{seq}"),
            seq,
            0,
            &key_id(&vk),
            &["node.tar.gz".into()],
        )
        .unwrap();
        let bytes = manifest_bytes(&m);
        fs::write(dir.join(MANIFEST_NAME), &bytes).unwrap();
        fs::write(dir.join(SIGNATURE_NAME), sign(&bytes, &key).unwrap()).unwrap();
        vk
    }
    #[tokio::test]
    async fn r8_repeated_seq_is_refused_after_install_and_rollback() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let install = root.path().join("install");
        archive(source.path(), "yandi", b'0', b"first");
        let vk = signed(source.path(), 1);
        update(source.path().to_str().unwrap(), &vk, &install, 0).await.unwrap();
        assert!(update(source.path().to_str().unwrap(), &vk, &install, 0).await.is_err());
        archive(source.path(), "yandi", b'0', b"second");
        signed(source.path(), 2);
        update(source.path().to_str().unwrap(), &vk, &install, 1).await.unwrap();
        rollback(&install).unwrap();
        let state = status(&install).unwrap().unwrap();
        assert_eq!((state.seq, state.max_seq), (1, 2));
        assert!(update(source.path().to_str().unwrap(), &vk, &install, 0).await.is_err());
        assert_eq!(std::fs::read(install.join("yandi")).unwrap(), b"first");
    }

    #[tokio::test]
    async fn update_repeat_rollback_and_high_water() {
        let root = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let install = root.path().join("install");
        archive(source.path(), "./yandi", b'0', b"one");
        let vk = signed(source.path(), 1);
        update(source.path().to_str().unwrap(), &vk, &install, 0)
            .await
            .unwrap();
        assert_eq!(fs::read(install.join("yandi")).unwrap(), b"one");
        assert_eq!(
            fs::metadata(install.join("yandi"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        update(source.path().to_str().unwrap(), &vk, &install, 0)
            .await
            .unwrap();
        fs::create_dir_all(install.join("current/data")).unwrap();
        fs::write(
            install.join("current/data/.release.json"),
            b"nested metadata",
        )
        .unwrap();
        fs::write(install.join("current/data/.previous"), b"nested data").unwrap();
        archive(source.path(), "yandi", b'0', b"two");
        signed(source.path(), 2);
        update(source.path().to_str().unwrap(), &vk, &install, 0)
            .await
            .unwrap();
        assert_eq!(fs::read(install.join("previous/yandi")).unwrap(), b"one");
        assert_eq!(rollback(&install).unwrap().seq, 1);
        assert_eq!(
            fs::read(install.join("current/data/.release.json")).unwrap(),
            b"nested metadata"
        );
        assert_eq!(
            fs::read(install.join("current/data/.previous")).unwrap(),
            b"nested data"
        );
        assert_eq!(fs::read(install.join("yandi")).unwrap(), b"one");
        signed(source.path(), 1);
        assert!(update(source.path().to_str().unwrap(), &vk, &install, 0)
            .await
            .is_err());
        assert_eq!(status(&install).unwrap().unwrap().max_seq, 2);
    }
    #[tokio::test]
    async fn tampering_rejected_before_install_writes() {
        for name in [MANIFEST_NAME, SIGNATURE_NAME, "node.tar.gz"] {
            let source = tempfile::tempdir().unwrap();
            let root = tempfile::tempdir().unwrap();
            let install = root.path().join("install");
            archive(source.path(), "yandi", b'0', b"ok");
            let vk = signed(source.path(), 3);
            let mut bytes = fs::read(source.path().join(name)).unwrap();
            bytes[0] ^= 1;
            fs::write(source.path().join(name), bytes).unwrap();
            assert!(update(source.path().to_str().unwrap(), &vk, &install, 0)
                .await
                .is_err());
            assert!(!install.exists());
        }
    }
    #[tokio::test]
    async fn unsafe_archives_and_truncation_leave_current_unchanged() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let install = root.path().join("install");
        archive(source.path(), "yandi", b'0', b"old");
        let vk = signed(source.path(), 1);
        update(source.path().to_str().unwrap(), &vk, &install, 0)
            .await
            .unwrap();
        for (name, kind) in [
            ("../escape", b'0'),
            ("/absolute", b'0'),
            ("yandi", b'2'),
            ("yandi", b'1'),
            ("yandi", b'3'),
            ("README", b'0'),
        ] {
            archive(source.path(), name, kind, b"bad");
            signed(source.path(), 2);
            assert!(update(source.path().to_str().unwrap(), &vk, &install, 0)
                .await
                .is_err());
            assert_eq!(fs::read(install.join("yandi")).unwrap(), b"old");
            assert_eq!(status(&install).unwrap().unwrap().seq, 1);
        }
        fs::write(source.path().join("node.tar.gz"), b"truncated").unwrap();
        signed(source.path(), 2);
        assert!(update(source.path().to_str().unwrap(), &vk, &install, 0)
            .await
            .is_err());
        let stage = tempfile::Builder::new()
            .prefix("interrupt")
            .tempdir_in(root.path())
            .unwrap();
        fs::write(stage.path().join("yandi"), b"new").unwrap();
        let old = generation(&install).unwrap().unwrap();
        let state = State {
            version: "new".into(),
            seq: 2,
            max_seq: 2,
            key_id: key_id(&vk),
        };
        assert!(commit(&install, stage, Some(&old), &state, true).is_err());
        assert_eq!(fs::read(install.join("yandi")).unwrap(), b"old");
        assert_eq!(status(&install).unwrap().unwrap().seq, 1);
    }
    #[tokio::test]
    async fn size_seq_url_limits() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let install = root.path().join("install");
        archive(source.path(), "yandi", b'0', b"ok");
        let vk = signed(source.path(), 3);
        assert!(update(source.path().to_str().unwrap(), &vk, &install, 4)
            .await
            .is_err());
        for url in [
            "http://127.0.0.1/",
            "ftp://host/",
            "file:///tmp/",
            "https://user:pass@host/",
            "https://host/?x=1",
        ] {
            assert!(update(url, &vk, &install, 0).await.is_err());
        }
        let key = SigningKey::from_bytes(&[42; 32]);
        let mut m: Manifest =
            serde_json::from_slice(&fs::read(source.path().join(MANIFEST_NAME)).unwrap()).unwrap();
        m.files[0].size = MAX_FILE + 1;
        let bytes = manifest_bytes(&m);
        fs::write(source.path().join(MANIFEST_NAME), &bytes).unwrap();
        fs::write(source.path().join(SIGNATURE_NAME), sign(&bytes, &key).unwrap()).unwrap();
        assert!(update(source.path().to_str().unwrap(), &vk, &install, 0)
            .await
            .is_err());
        assert!(bounded(&b"12345"[..], 4).is_err());
        assert!(!install.exists());
    }
    #[test]
    fn killed_before_commit_child() {
        let Some(root) = std::env::var_os("YANDI_TEST_KILL") else {
            return;
        };
        let install = Path::new(&root).join("install");
        let stage = tempfile::Builder::new()
            .prefix("killed")
            .tempdir_in(&root)
            .unwrap();
        fs::write(stage.path().join("yandi"), b"new").unwrap();
        let old = generation(&install).unwrap().unwrap();
        let state = State {
            version: "new".into(),
            seq: 2,
            max_seq: 2,
            key_id: "test".into(),
        };
        commit(&install, stage, Some(&old), &state, true).unwrap();
    }
    #[tokio::test]
    async fn sigkill_before_commit_preserves_state_and_can_retry() {
        use std::os::unix::process::ExitStatusExt;
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let install = root.path().join("install");
        archive(source.path(), "yandi", b'0', b"old");
        let vk = signed(source.path(), 1);
        update(source.path().to_str().unwrap(), &vk, &install, 0)
            .await
            .unwrap();
        let exit = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "updater::tests::killed_before_commit_child",
                "--nocapture",
            ])
            .env("YANDI_TEST_KILL", root.path())
            .status()
            .unwrap();
        assert_eq!(exit.signal(), Some(libc::SIGKILL));
        assert_eq!(fs::read(install.join("yandi")).unwrap(), b"old");
        assert_eq!(status(&install).unwrap().unwrap().seq, 1);
        archive(source.path(), "yandi", b'0', b"new");
        signed(source.path(), 2);
        update(source.path().to_str().unwrap(), &vk, &install, 0)
            .await
            .unwrap();
        assert_eq!(status(&install).unwrap().unwrap().seq, 2);
    }
    async fn tls_fixture(
        response: &'static [u8],
        delay: Duration,
    ) -> (reqwest::Client, url::Url, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let cfg = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()).into(),
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = base_url(&format!(
            "https://localhost:{}/release",
            listener.local_addr().unwrap().port()
        ))
        .unwrap();
        let client = client_builder()
            .add_root_certificate(reqwest::Certificate::from_der(cert.cert.der()).unwrap())
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(cfg));
            let mut tls = acceptor.accept(socket).await.unwrap();
            let mut buf = [0; 4096];
            let _ = tls.read(&mut buf).await;
            tokio::time::sleep(delay).await;
            let _ = tls.write_all(response).await;
            let _ = tls.shutdown().await;
        });
        (client, base, server)
    }
    #[tokio::test]
    async fn https_download_limits_redirects_and_timeout_on_loopback() {
        let (client, base, task) = tls_fixture(
            b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc",
            Duration::ZERO,
        )
        .await;
        assert_eq!(download(&client, &base, "file", 3).await.unwrap(), b"abc");
        task.await.unwrap();
        for response in [
            &b"HTTP/1.1 200 OK\r\nContent-Length: 10000\r\nConnection: close\r\n\r\n"[..],
            &b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\n12345\r\n0\r\n\r\n"[..],
            &b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/evil\r\nContent-Length: 0\r\n\r\n"[..],
            &b"HTTP/1.1 302 Found\r\nLocation: https://other.invalid/evil\r\nContent-Length: 0\r\n\r\n"[..],
        ] {
            let (client, base, task) = tls_fixture(response, Duration::ZERO).await;
            assert!(download(&client, &base, "file", 4).await.is_err()); task.await.unwrap();
        }
        let (client, base, task) = tls_fixture(b"", Duration::from_secs(1)).await;
        assert!(download(&client, &base, "file", 4).await.is_err());
        task.abort();
    }
    #[tokio::test]
    async fn occupied_alias_is_refused_and_sparse_local_file_is_bounded() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let install = root.path().join("install");
        archive(source.path(), "yandi", b'0', b"new");
        let vk = signed(source.path(), 1);
        fs::create_dir(&install).unwrap();
        fs::write(install.join("yandi"), b"unmanaged").unwrap();
        assert!(update(source.path().to_str().unwrap(), &vk, &install, 0)
            .await
            .is_err());
        assert_eq!(fs::read(install.join("yandi")).unwrap(), b"unmanaged");
        assert!(status(&install).unwrap().is_none());
        let sparse = fs::File::create(source.path().join("large")).unwrap();
        sparse.set_len(MAX_FILE + 1).unwrap();
        assert!(local_file(source.path(), "large", MAX_FILE).is_err());
    }
}
