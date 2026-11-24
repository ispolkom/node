//! P1c-1: the root key, the identity, migration and recovery — all in temporary directories, with a fake machine id and never
//! the owner's real key directory.
#![cfg(unix)]
use key_root::atomic::{backup_copy, create_new_file, write_atomic};
use key_root::identity_store::seal_identity_v3;
use key_root::recovery_code::{looks_like_code, recovery_secret, RecoveryCode};
use key_root::wrap::{derive_kek, KdfParams, KdfPolicy};
use key_root::*;
use std::cell::Cell;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use zeroize::Zeroizing;

const PORT: u16 = 9000;
const PASSWORD: &str = "correct horse battery staple";
const FAST: KdfParams = KdfParams {
    memory_kib: 64,
    iterations: 1,
    parallelism: 1,
};

fn policy() -> KdfPolicy {
    KdfPolicy::for_tests()
}

fn material(seed: u8) -> IdentityMaterial {
    IdentityMaterial {
        address: [seed; 32],
        public_key: [seed.wrapping_add(1); 32],
        signing_public_key: [seed.wrapping_add(2); 32],
        created_at: "2026-06-28T16:14:00Z".into(),
        private_key: Zeroizing::new([seed.wrapping_add(3); 32]),
        signing_private_key: Zeroizing::new([seed.wrapping_add(4); 32]),
    }
}

struct Store {
    _tmp: tempfile::TempDir,
    dir: KeyDir,
}

impl Store {
    fn new() -> Store {
        let tmp = tempfile::tempdir().unwrap();
        let dir = KeyDir::open(tmp.path().join("keys")).unwrap();
        Store { _tmp: tmp, dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn snapshot(&self) -> Vec<(String, Vec<u8>)> {
        snapshot_dir(self.path())
    }
}

fn snapshot_dir(path: &Path) -> Vec<(String, Vec<u8>)> {
    let mut all: Vec<_> = fs::read_dir(path)
        .unwrap()
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    all.sort();
    all
}

/// A key directory as the node makes it: auth.json v2 (device + recovery wrappers), the device key and a v3 identity.
fn new_store(machine: &str, master: [u8; 32], seed: u8) -> (Store, IdentityMaterial) {
    new_store_with(machine, master, seed, FAST, &policy())
}

fn new_store_with(
    machine: &str,
    master: [u8; 32],
    seed: u8,
    params: KdfParams,
    pol: &KdfPolicy,
) -> (Store, IdentityMaterial) {
    let s = Store::new();
    let m = FixedMachine(machine.into());
    let device = FileDeviceKey::new(s.dir.file("device.key"));
    let device_key = device.create().unwrap();
    let doc = RootDocument::create(
        &master,
        "$argon2id$login-hash",
        &device_key,
        device.name(),
        &m,
        PASSWORD,
        params,
        pol,
    )
    .unwrap();
    create_new_file(&s.dir.auth_file(), &doc.to_json()).unwrap();
    let id = material(seed);
    create_new_file(&s.dir.identity_file(PORT), &seal_identity_v3(&master, &id)).unwrap();
    (s, id)
}

fn recover(s: &Store, machine: &str, password: &str) -> Result<RecoveryReport> {
    let m = FixedMachine(machine.into());
    let pol = policy();
    let device = FileDeviceKey::new(s.dir.file("device.key"));
    Recovery {
        dir: &s.dir,
        port: PORT,
        machine: &m,
        device: &device,
        password,
        policy: &pol,
    }
    .run()
}

fn open_identity_with(
    s: &Store,
    machine: &str,
    root: Option<&[u8; 32]>,
    env: Option<&str>,
) -> Result<IdentityMaterial> {
    let m = FixedMachine(machine.into());
    key_root::identity_store::load_identity(
        &s.dir,
        PORT,
        &OpenContext { root },
    )
}

fn file_mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

// ── the root document ────────────────────────────────────────────────────────────────────────────────────────────

fn document(root: [u8; 32], device: [u8; 32], machine: &str) -> RootDocument {
    RootDocument::create(
        &root,
        "login",
        &device,
        "file-v1",
        &FixedMachine(machine.into()),
        PASSWORD,
        FAST,
        &policy(),
    )
    .unwrap()
}

#[test]
fn the_root_opens_by_device_and_by_password_and_it_is_the_same_root() {
    let root = [7u8; 32];
    let doc = document(root, [9u8; 32], "machine-A");
    let parsed = RootDocument::parse(&doc.to_json()).unwrap();
    assert_eq!(
        *parsed
            .unlock_with_device(&[9u8; 32], &FixedMachine("machine-A".into()))
            .unwrap(),
        root
    );
    assert_eq!(
        *parsed.unlock_with_password(PASSWORD, &policy()).unwrap(),
        root
    );
}

#[test]
fn a_wrong_password_is_refused_and_a_weak_new_password_is_rejected() {
    let doc = document([7u8; 32], [9u8; 32], "machine-A");
    assert_eq!(
        doc.unlock_with_password("correct horse battery stapl", &policy())
            .unwrap_err(),
        KeyRootError::RecoveryFailed
    );
    assert_eq!(
        doc.unlock_with_password("", &policy()).unwrap_err(),
        KeyRootError::RecoveryFailed
    );
    let short = RootDocument::create(
        &[7u8; 32],
        "l",
        &[9u8; 32],
        "file-v1",
        &FixedMachine("m".into()),
        "short",
        FAST,
        &policy(),
    );
    assert!(matches!(short, Err(KeyRootError::Invalid(_))));
}

#[test]
fn a_changed_machine_id_needs_recovery_even_with_the_right_device_key() {
    let doc = document([7u8; 32], [9u8; 32], "machine-A");
    assert_eq!(
        doc.unlock_with_device(&[9u8; 32], &FixedMachine("machine-B".into()))
            .unwrap_err(),
        KeyRootError::RecoveryRequired
    );
}

#[test]
fn the_device_key_is_the_secret_and_the_machine_id_is_not() {
    let doc = document([7u8; 32], [9u8; 32], "machine-A");
    // the right machine id alone (a public value) opens nothing: no guessable key derived from it
    for guess in [
        [0u8; 32],
        [1u8; 32],
        *b"machine-A\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
        *b"YANDI_MACHINE:machine-A\0\0\0\0\0\0\0\0\0",
    ] {
        assert_eq!(
            doc.unlock_with_device(&guess, &FixedMachine("machine-A".into()))
                .unwrap_err(),
            KeyRootError::RecoveryRequired
        );
    }
    use sha2::{Digest, Sha256};
    let derived: [u8; 32] = Sha256::digest(b"machine-A").into();
    assert!(
        doc.unlock_with_device(&derived, &FixedMachine("machine-A".into()))
            .is_err(),
        "a key derived from the machine id opened the root"
    );
}

#[test]
fn recovery_does_not_depend_on_any_machine_id() {
    let doc = document([7u8; 32], [9u8; 32], "machine-A");
    // the password path takes no machine at all; the same document opens the same way "on any machine"
    assert_eq!(
        *doc.unlock_with_password(PASSWORD, &policy()).unwrap(),
        [7u8; 32]
    );
    let other = document([7u8; 32], [3u8; 32], "machine-Z");
    assert_eq!(
        *other.unlock_with_password(PASSWORD, &policy()).unwrap(),
        [7u8; 32]
    );
}

#[test]
fn the_password_goes_through_argon2id_and_stored_parameters_below_the_policy_are_refused() {
    use argon2::{Algorithm, Argon2, Params, Version};
    let salt = [5u8; 32];
    let ours = derive_kek(PASSWORD.as_bytes(), &salt, &FAST, &policy()).unwrap();
    let mut reference = [0u8; 32];
    Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(64, 1, 1, Some(32)).unwrap(),
    )
    .hash_password_into(PASSWORD.as_bytes(), &salt, &mut reference)
    .unwrap();
    assert_eq!(
        *ours, reference,
        "the recovery KEK is not Argon2id of the password"
    );
    // another parameter set gives another key
    let other = derive_kek(
        PASSWORD.as_bytes(),
        &salt,
        &KdfParams {
            memory_kib: 128,
            iterations: 1,
            parallelism: 1,
        },
        &policy(),
    )
    .unwrap();
    assert_ne!(*ours, *other);
    // a file edited to make guessing cheap is refused under the production policy
    let doc = document([7u8; 32], [9u8; 32], "machine-A");
    assert_eq!(
        doc.unlock_with_password(PASSWORD, &KdfPolicy::production())
            .unwrap_err(),
        KeyRootError::WeakParameters
    );
    // and changing the stored parameters changes the outcome (they are authenticated data)
    let mut tampered = doc.clone();
    tampered.recovery.memory_kib = 128;
    assert!(tampered.unlock_with_password(PASSWORD, &policy()).is_err());
}

fn ctx<'a>(m: &'a FixedMachine, root: Option<&'a [u8; 32]>) -> OpenContext<'a> {
    OpenContext { root }
}

fn flip_hex(value: &mut serde_json::Value, path: &[&str]) {
    let pointer = format!("/{}", path.join("/"));
    let slot = value.pointer_mut(&pointer).expect("field present");
    let s = slot.as_str().unwrap().to_owned();
    let first = if s.starts_with('0') { '1' } else { '0' };
    *slot = serde_json::Value::String(format!("{first}{}", &s[1..]));
}

#[test]
fn a_damaged_auth_file_never_opens_and_says_why() {
    let doc = document([7u8; 32], [9u8; 32], "machine-A");
    let good = doc.to_json();
    let machine = FixedMachine("machine-A".into());
    let opens = |bytes: &[u8]| {
        RootDocument::parse(bytes)
            .and_then(|d| d.unlock_with_device(&[9u8; 32], &machine).map(|_| ()))
            .is_ok()
    };
    assert!(opens(&good));
    for path in [
        &["device", "ciphertext"][..],
        &["device", "nonce"],
        &["root_id"],
    ] {
        let mut v: serde_json::Value = serde_json::from_slice(&good).unwrap();
        flip_hex(&mut v, path);
        assert!(
            !opens(&serde_json::to_vec(&v).unwrap()),
            "flipped {path:?} still opened"
        );
    }
    let mut v: serde_json::Value = serde_json::from_slice(&good).unwrap();
    flip_hex(&mut v, &["recovery", "ciphertext"]);
    assert!(RootDocument::parse(&serde_json::to_vec(&v).unwrap())
        .unwrap()
        .unlock_with_password(PASSWORD, &policy())
        .is_err());
    assert!(matches!(
        RootDocument::parse(&good[..good.len() / 2]),
        Err(KeyRootError::Corrupt)
    ));
    assert!(matches!(
        RootDocument::parse(b"not json"),
        Err(KeyRootError::Corrupt)
    ));
    assert!(matches!(
        RootDocument::parse(b"{}"),
        Err(KeyRootError::Corrupt)
    ));
    let mut v: serde_json::Value = serde_json::from_slice(&good).unwrap();
    v["version"] = 9.into();
    assert_eq!(
        RootDocument::parse(&serde_json::to_vec(&v).unwrap()).unwrap_err(),
        KeyRootError::UnsupportedVersion(9)
    );
    let mut v: serde_json::Value = serde_json::from_slice(&good).unwrap();
    v["surprise"] = 1.into();
    assert!(matches!(
        RootDocument::parse(&serde_json::to_vec(&v).unwrap()),
        Err(KeyRootError::Corrupt)
    ));
    let mut v: serde_json::Value = serde_json::from_slice(&good).unwrap();
    v["recovery"]["kdf"] = "sha256".into();
    assert!(matches!(
        RootDocument::parse(&serde_json::to_vec(&v).unwrap()),
        Err(KeyRootError::Corrupt)
    ));
}

// ── the identity file and the loading policy (F11) ───────────────────────────────────────────────────────────────

#[test]
fn identity_v3_opens_only_with_its_root_and_authenticates_its_public_fields() {
    let root = [7u8; 32];
    let id = material(1);
    let bytes = seal_identity_v3(&root, &id);
    let m = FixedMachine("m".into());
    assert!(
        key_root::identity_store::open_identity(&bytes, &ctx(&m, Some(&root)))
            .unwrap()
            .same_identity(&id)
    );
    assert_eq!(
        key_root::identity_store::open_identity(&bytes, &ctx(&m, None)).unwrap_err(),
        KeyRootError::RecoveryRequired
    );
    assert_eq!(
        key_root::identity_store::open_identity(&bytes, &ctx(&m, Some(&[8u8; 32]))).unwrap_err(),
        KeyRootError::DecryptFailed
    );
    for field in ["public_key", "signing_public_key", "address"] {
        let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        flip_hex(&mut v, &[field]);
        assert!(
            key_root::identity_store::open_identity(
                &serde_json::to_vec(&v).unwrap(),
                &ctx(&m, Some(&root))
            )
            .is_err(),
            "{field} was not authenticated"
        );
    }
    let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    flip_hex(&mut v, &["ciphertext"]);
    assert_eq!(
        key_root::identity_store::open_identity(
            &serde_json::to_vec(&v).unwrap(),
            &ctx(&m, Some(&root))
        )
        .unwrap_err(),
        KeyRootError::DecryptFailed
    );
    let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    v["version"] = 7.into();
    assert_eq!(
        key_root::identity_store::open_identity(
            &serde_json::to_vec(&v).unwrap(),
            &ctx(&m, Some(&root))
        )
        .unwrap_err(),
        KeyRootError::UnsupportedVersion(7)
    );
}

#[test]
fn an_absent_identity_is_created_once_and_then_loaded_never_replaced() {
    let s = Store::new();
    let m = FixedMachine("machine-A".into());
    let root = [3u8; 32];
    let ctx = OpenContext { root: Some(&root) };
    let first = load_or_initialize(&s.dir, PORT, &ctx, || material(1)).unwrap();
    assert!(first.created);
    let bytes = fs::read(s.dir.identity_file(PORT)).unwrap();
    assert_eq!(file_mode(&s.dir.identity_file(PORT)), 0o600);
    let second = load_or_initialize(&s.dir, PORT, &ctx, || {
        panic!("must not create a second identity")
    })
    .unwrap();
    assert!(!second.created && second.identity.same_identity(&first.identity));
    assert_eq!(fs::read(s.dir.identity_file(PORT)).unwrap(), bytes);
}

/// The counterfactual of F11: an identity exists and cannot be opened → an error, the file byte-for-byte unchanged, nothing new.
fn assert_fails_closed(
    s: &Store,
    machine: &str,
    root: Option<&[u8; 32]>,
    env: Option<&str>,
    expect: impl Fn(&KeyRootError) -> bool,
) {
    let before = s.snapshot();
    let m = FixedMachine(machine.into());
    let ctx = OpenContext { root };
    let made = Cell::new(false);
    let err = load_or_initialize(&s.dir, PORT, &ctx, || {
        made.set(true);
        material(99)
    })
    .err()
    .expect("an identity that cannot be opened must be an error, not a new identity");
    assert!(expect(&err), "unexpected error class {err:?}");
    assert!(
        !made.get(),
        "a new identity was generated although one exists"
    );
    assert_eq!(
        s.snapshot(),
        before,
        "a failed load changed the key directory"
    );
}

#[test]
fn f11_an_unsafe_identity_file_is_refused_and_left_alone() {
    let (s, _id) = new_store("machine-A", [7u8; 32], 5);
    let path = s.dir.identity_file(PORT);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert_fails_closed(&s, "machine-A", None, None, |e| {
        *e == KeyRootError::PermissionDenied
    });
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let real = s.dir.file("real.json");
    fs::rename(&path, &real).unwrap();
    std::os::unix::fs::symlink(&real, &path).unwrap();
    assert_fails_closed(&s, "machine-A", None, None, |e| {
        *e == KeyRootError::PermissionDenied
    });
}

#[test]
fn an_identity_for_another_port_blocks_creating_a_second_one() {
    let s = Store::new();
    create_new_file(
        &s.dir.identity_file(9001),
        &seal_identity_v3(&[1u8; 32], &material(2)),
    )
    .unwrap();
    let before = s.snapshot();
    let ctx = OpenContext { root: None };
    let err = load_or_initialize(&s.dir, PORT, &ctx, || material(9))
        .err()
        .unwrap();
    assert!(matches!(err, KeyRootError::Refused(_)));
    assert_eq!(s.snapshot(), before);
}

// ── atomic writes and backups ────────────────────────────────────────────────────────────────────────────────────

#[test]
fn a_failed_verification_leaves_the_original_untouched_and_no_temporary_file() {
    let s = Store::new();
    let path = s.dir.file("x.json");
    create_new_file(&path, b"original").unwrap();
    assert!(write_atomic(&path, b"new", |_| false).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"original");
    assert_eq!(
        fs::read_dir(s.path()).unwrap().count(),
        1,
        "a temporary file was left behind"
    );
    write_atomic(&path, b"new", |b| b == b"new").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"new");
    assert_eq!(file_mode(&path), 0o600);
}

#[test]
fn backups_are_exclusive_copies_that_are_never_overwritten() {
    let s = Store::new();
    let path = s.dir.file("auth.json");
    create_new_file(&path, b"one").unwrap();
    let b1 = backup_copy(&path, "legacy").unwrap();
    write_atomic(&path, b"two", |_| true).unwrap();
    let b2 = backup_copy(&path, "legacy").unwrap();
    assert_ne!(b1, b2);
    assert_eq!(fs::read(&b1).unwrap(), b"one");
    assert_eq!(fs::read(&b2).unwrap(), b"two");
    assert_eq!(file_mode(&b1), 0o600);
}

// ── the key directory ────────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn the_key_directory_is_tightened_when_only_too_open_and_refused_when_strange() {
    let tmp = tempfile::tempdir().unwrap();
    let open = tmp.path().join("open");
    fs::create_dir(&open).unwrap();
    fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).unwrap();
    let d = KeyDir::open(&open).unwrap();
    assert_eq!(file_mode(d.path()), 0o700);
    let target = tmp.path().join("real");
    fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, tmp.path().join("link")).unwrap();
    assert!(matches!(
        KeyDir::open(tmp.path().join("link")),
        Err(KeyRootError::PermissionDenied)
    ));
    fs::write(tmp.path().join("file"), "x").unwrap();
    assert!(matches!(
        KeyDir::open(tmp.path().join("file")),
        Err(KeyRootError::PermissionDenied)
    ));
    assert_eq!(
        file_mode(&target),
        0o755 & file_mode(&target),
        "the target of a refused link must not be chmod-ed"
    );
}

#[test]
fn the_device_key_file_is_private_and_a_loose_one_is_refused() {
    let s = Store::new();
    let p = FileDeviceKey::new(s.dir.file("device.key"));
    assert!(p.load().unwrap().is_none());
    let key = p.create().unwrap();
    assert_eq!(file_mode(&s.dir.file("device.key")), 0o600);
    assert_eq!(*p.load().unwrap().unwrap(), *key);
    assert!(
        p.create().is_err(),
        "create must not overwrite an existing device key"
    );
    fs::set_permissions(s.dir.file("device.key"), fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(p.load().unwrap_err(), KeyRootError::PermissionDenied);
    fs::set_permissions(s.dir.file("device.key"), fs::Permissions::from_mode(0o600)).unwrap();
    let old = p.load().unwrap().unwrap();
    let new = p.replace().unwrap();
    assert_ne!(*old, *new);
    assert_eq!(
        fs::read(s.dir.file("device.key.replaced-000")).unwrap(),
        old.as_slice(),
        "the replaced key must be kept"
    );
}

// ── migration ────────────────────────────────────────────────────────────────────────────────────────────────────

/// A device provider that hands out a wrong key after a few calls, so that the verification from disk fails.
struct Flaky {
    inner: FileDeviceKey,
    loads: Cell<u32>,
    good_loads: u32,
}

impl DeviceKeyProvider for Flaky {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn protection(&self) -> DeviceProtection {
        self.inner.protection()
    }
    fn load(&self) -> key_root::Result<Option<Zeroizing<[u8; 32]>>> {
        self.loads.set(self.loads.get() + 1);
        if self.loads.get() > self.good_loads {
            return Ok(Some(Zeroizing::new([0xEE; 32])));
        }
        self.inner.load()
    }
    fn create(&self) -> key_root::Result<Zeroizing<[u8; 32]>> {
        self.inner.create()
    }
    fn replace(&self) -> key_root::Result<Zeroizing<[u8; 32]>> {
        self.inner.replace()
    }
}

// ── recovery: the main benchmark ─────────────────────────────────────────────────────────────────────────────────

fn copy_dir(from: &KeyDir, skip: &[&str]) -> Store {
    let to = Store::new();
    for (name, bytes) in snapshot_dir(from.path()) {
        if skip.iter().any(|s| name.starts_with(s)) {
            continue;
        }
        create_new_file(&to.dir.file(&name), &bytes).unwrap();
    }
    to
}

#[test]
fn losing_the_device_is_not_losing_the_identity() {
    let master = [7u8; 32];
    let (a, id) = new_store("machine-A", master, 5);
    let node_id_a = id.address;

    // Machine B: a different machine id, and NO device key (only the files the owner backed up)
    let b = copy_dir(
        &a.dir,
        &[
            "device.key",
            "auth.json.legacy",
            "node_identity_9000.json.legacy",
        ],
    );
    let mb = FixedMachine("machine-B".into());
    let device_b = FileDeviceKey::new(b.dir.file("device.key"));
    assert_eq!(
        unlock_root(&b.dir, &mb, &device_b).err().unwrap(),
        KeyRootError::RecoveryRequired
    );
    assert_eq!(
        open_identity_with(&b, "machine-B", None, None)
            .err()
            .unwrap(),
        KeyRootError::RecoveryRequired
    );

    // a wrong password: refused, and not one byte on disk changes, no device key appears, no new identity or root
    let before = b.snapshot();
    assert_eq!(
        recover(&b, "machine-B", "not the password at all")
            .err()
            .unwrap(),
        KeyRootError::RecoveryFailed
    );
    assert_eq!(
        b.snapshot(),
        before,
        "a wrong recovery password changed the key directory"
    );

    // the right password: the SAME root, the SAME identity, a NEW device binding
    let report = recover(&b, "machine-B", PASSWORD).unwrap();
    assert_eq!(report.node_id, hex::encode(&node_id_a[..8]));
    assert!(b.dir.file("device.key").exists());
    let (root, source) = unlock_root(&b.dir, &mb, &device_b).unwrap(); // the next boot: no password
    assert_eq!((*root, source), (master, RootSource::Device));
    let recovered = open_identity_with(&b, "machine-B", Some(&root), None).unwrap();
    assert!(
        recovered.same_identity(&id),
        "the recovered identity is not the same identity"
    );
    assert_eq!(recovered.address, node_id_a);
    // a restart of the recovered node: still the same identity, still no password
    let (root2, _) = unlock_root(&b.dir, &mb, &device_b).unwrap();
    assert_eq!(
        open_identity_with(&b, "machine-B", Some(&root2), None)
            .unwrap()
            .address,
        node_id_a
    );
    // and the old machine's binding no longer opens on B (the wrapper was re-bound)
    assert_eq!(
        unlock_root(&b.dir, &FixedMachine("machine-A".into()), &device_b)
            .err()
            .unwrap(),
        KeyRootError::RecoveryRequired
    );
}

#[test]
fn a_lost_device_key_on_the_same_machine_is_recovered_the_same_way() {
    let (a, id) = new_store("machine-A", [7u8; 32], 5);
    fs::remove_file(a.dir.file("device.key")).unwrap();
    let m = FixedMachine("machine-A".into());
    let device = FileDeviceKey::new(a.dir.file("device.key"));
    assert_eq!(
        unlock_root(&a.dir, &m, &device).err().unwrap(),
        KeyRootError::RecoveryRequired
    );
    recover(&a, "machine-A", PASSWORD).unwrap();
    let (root, _) = unlock_root(&a.dir, &m, &device).unwrap();
    assert!(open_identity_with(&a, "machine-A", Some(&root), None)
        .unwrap()
        .same_identity(&id));
}

#[test]
fn copying_the_whole_directory_to_another_machine_does_not_open_by_itself_and_recovery_keeps_the_old_device_key(
) {
    let (a, id) = new_store("machine-A", [7u8; 32], 5);
    let b = copy_dir(&a.dir, &[]); // includes device.key
    let mb = FixedMachine("machine-B".into());
    let device_b = FileDeviceKey::new(b.dir.file("device.key"));
    assert_eq!(
        unlock_root(&b.dir, &mb, &device_b).err().unwrap(),
        KeyRootError::RecoveryRequired,
        "the machine binding did not hold"
    );
    let report = recover(&b, "machine-B", PASSWORD).unwrap();
    assert!(
        b.dir.file("device.key.replaced-000").exists(),
        "the replaced device key must be kept"
    );
    assert!(report
        .backups
        .iter()
        .any(|p| p.to_string_lossy().contains("before-recovery")));
    let (root, _) = unlock_root(&b.dir, &mb, &device_b).unwrap();
    assert!(open_identity_with(&b, "machine-B", Some(&root), None)
        .unwrap()
        .same_identity(&id));
}

#[test]
fn recovery_needs_the_identity_to_open_with_the_recovered_root_and_changes_nothing_otherwise() {
    let (a, _id) = new_store("machine-A", [7u8; 32], 5);
    let b = copy_dir(
        &a.dir,
        &[
            "device.key",
            ".legacy-",
            "auth.json.legacy",
            "node_identity_9000.json.legacy",
        ],
    );
    // an identity file that does not belong to this root: the password authenticates, the identity does not
    let other = seal_identity_v3(&[1u8; 32], &material(9));
    write_atomic(&b.dir.identity_file(PORT), &other, |_| true).unwrap();
    let before = b.snapshot();
    assert!(recover(&b, "machine-B", PASSWORD).is_err());
    assert_eq!(b.snapshot(), before);
    // no identity at all
    fs::remove_file(b.dir.identity_file(PORT)).unwrap();
    let before = b.snapshot();
    assert_eq!(
        recover(&b, "machine-B", PASSWORD).err().unwrap(),
        KeyRootError::NotFound
    );
    assert_eq!(b.snapshot(), before);
}

// ── no secrets in what is printed ────────────────────────────────────────────────────────────────────────────────

#[test]
fn no_debug_or_error_text_carries_a_key_or_a_password() {
    let id = material(1);
    let text = format!("{id:?}");
    assert!(!text.contains(&hex::encode(*id.private_key)) && !text.contains("private"));
    for e in [
        KeyRootError::DecryptFailed,
        KeyRootError::RecoveryFailed,
        KeyRootError::Corrupt,
        KeyRootError::RecoveryRequired,
        KeyRootError::WeakParameters,
    ] {
        let shown = format!("{e} {e:?} {}", e.category());
        assert!(
            !shown.contains('/')
                && !shown.to_lowercase().contains("aes")
                && !shown.contains(PASSWORD)
        );
    }
    assert_eq!(KeyRootError::DecryptFailed.category(), "identity_locked");
    assert_eq!(
        KeyRootError::RecoveryRequired.category(),
        "identity_recovery_required"
    );
    assert_eq!(
        KeyRootError::RecoveryFailed.category(),
        "identity_recovery_failed"
    );
    assert_eq!(
        KeyRootError::NotFound.category(),
        "identity_not_initialized"
    );
    assert_eq!(KeyRootError::Corrupt.category(), "identity_corrupt");
    assert_eq!(
        KeyRootError::UnsupportedVersion(9).category(),
        "identity_format_unsupported"
    );
}

// ── the command-line tool, as the owner will run it ──────────────────────────────────────────────────────────────

fn run_tool(args: &[&str], stdin: &str) -> (i32, String, String) {
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_yandi-keys"))
        .args(args)
        .env_remove("YANDI_KEY_PASSWORD")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn the_tool_recovers_and_never_prints_a_secret() {
    let master = [0x5Au8; 32];
    let (a, id) = new_store_with("machine-A", master, 5, KdfParams::RECOMMENDED, &KdfPolicy::production());
    // recovery on "another machine": copy without the device key
    let c = copy_dir(&a.dir, &["device.key", ".legacy-"]);
    let dir_c = c.path().to_str().unwrap().to_owned();
    let (code, out, err) = run_tool(
        &[
            "recover",
            "--dir",
            &dir_c,
            "--machine-id",
            "machine-C",
            "--password-stdin",
        ],
        "wrong password entirely\n",
    );
    assert_eq!(code, 2);
    assert!(
        err.contains("identity_recovery_failed") && !err.contains("wrong password entirely"),
        "{out}{err}"
    );
    let (code, out, err) = run_tool(
        &[
            "recover",
            "--dir",
            &dir_c,
            "--machine-id",
            "machine-C",
            "--password-stdin",
        ],
        &format!("{PASSWORD}\n"),
    );
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains(&hex::encode(&id.address[..8])));
    for secret in [
        PASSWORD.to_owned(),
        hex::encode(master),
        hex::encode(
            *HardwareDeviceKey::new(c.dir.file("device.seal"), c.dir.file("device.key"))
                .load()
                .unwrap()
                .unwrap(),
        ),
    ] {
        assert!(!out.contains(&secret) && !err.contains(&secret));
    }
    let (code, out, _) = run_tool(&["status", "--dir", &dir_c], "");
    assert_eq!(code, 0);
    assert!(
        out.contains("RootV3"),
        "{out}"
    );
}

// ── the recovery code (what the owner writes down) ───────────────────────────────────────────────────────────────

#[test]
fn recovery_codes_are_well_formed_random_and_carry_their_own_check() {
    let a = RecoveryCode::generate();
    let b = RecoveryCode::generate();
    assert_ne!(a.secret(), b.secret());
    let shown = a.display();
    assert_eq!(
        shown.len(),
        28 + 6,
        "seven groups of four with six dashes: {shown}"
    );
    assert!(shown.split('-').all(|g| g.len() == 4));
    assert!(
        shown
            .chars()
            .all(|c| c == '-' || "0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(c)),
        "look-alike characters in {shown}"
    );
    assert_eq!(RecoveryCode::parse(&shown).unwrap().secret(), a.secret());
}

#[test]
fn a_typed_code_is_read_forgivingly_but_a_typo_is_named_as_a_typo() {
    let code = RecoveryCode::generate();
    let shown = code.display();
    // case, spaces, missing dashes, underscores: all the same code
    for variant in [
        shown.to_lowercase(),
        shown.replace('-', " "),
        shown.replace('-', ""),
        format!("  {shown}\n"),
        shown.replace('-', "_").to_lowercase(),
    ] {
        assert_eq!(
            RecoveryCode::parse(&variant).unwrap().secret(),
            code.secret(),
            "{variant:?}"
        );
    }
    // O / I / L read as 0 / 1 / 1 (the alphabet has no O, I or L)
    let with_zero = RecoveryCode::parse("0000-0000-0000-0000-0000-0000-0000");
    let with_o = RecoveryCode::parse("OOOO-oooo-0000-0000-0000-0000-0000");
    assert_eq!(with_zero.is_ok(), with_o.is_ok());
    // every single wrong character, at every position, is refused as a typo (never silently accepted)
    let chars: Vec<char> = code.secret().chars().collect();
    for i in 0..chars.len() {
        let mut t = chars.clone();
        t[i] = if chars[i] == 'A' { 'B' } else { 'A' };
        let typo: String = t.into_iter().collect();
        assert!(
            RecoveryCode::parse(&typo).is_err(),
            "a typo at position {i} was accepted"
        );
    }
    // two neighbouring characters swapped
    let mut swapped = chars.clone();
    let j = (0..chars.len() - 1)
        .find(|&j| chars[j] != chars[j + 1])
        .unwrap();
    swapped.swap(j, j + 1);
    assert!(RecoveryCode::parse(&swapped.into_iter().collect::<String>()).is_err());
    assert!(
        matches!(RecoveryCode::parse(&shown[..20]), Err(KeyRootError::Invalid(m)) if m.contains("incomplete"))
    );
    assert!(
        matches!(RecoveryCode::parse(&format!("{shown}-AAAA")), Err(KeyRootError::Invalid(m)) if m.contains("too long"))
    );
    assert!(
        matches!(RecoveryCode::parse(&format!("{}U", &shown[..shown.len() - 1])), Err(KeyRootError::Invalid(m)) if m.contains("cannot be part"))
    );
    assert!(matches!(
        RecoveryCode::parse(&shown.replace("-", "-")).map(|_| ()),
        Ok(())
    ));
    // a code is keyed in its canonical form; anything else is a password, exactly as typed
    assert_eq!(
        recovery_secret(&shown.to_lowercase()).as_str(),
        code.secret()
    );
    assert_eq!(
        recovery_secret("an old chosen password").as_str(),
        "an old chosen password"
    );
    assert!(looks_like_code(&shown) && !looks_like_code("correct horse battery staple"));
}

fn migrated_store(machine: &str) -> (Store, IdentityMaterial) {
    new_store(machine, [7u8; 32], 5)
}

fn new_code_op<'a>(
    s: &'a Store,
    machine: &'a FixedMachine,
    device: &'a FileDeviceKey,
    pol: &'a KdfPolicy,
) -> NewRecoveryCode<'a> {
    NewRecoveryCode {
        dir: &s.dir,
        machine,
        device,
        params: FAST,
        policy: pol,
    }
}

#[test]
fn replacing_the_recovery_secret_by_a_code_needs_the_device_and_the_typed_confirmation() {
    let (s, id) = migrated_store("machine-A");
    let m = FixedMachine("machine-A".into());
    let device = FileDeviceKey::new(s.dir.file("device.key"));
    let pol = policy();
    let op = new_code_op(&s, &m, &device, &pol);
    let before = s.snapshot();

    // only the device may do it: another machine (the wrapper is bound to it) or a missing device key is "recovery required"
    let other = FixedMachine("machine-B".into());
    assert_eq!(
        new_code_op(&s, &other, &device, &pol)
            .prepare()
            .err()
            .unwrap(),
        KeyRootError::RecoveryRequired
    );
    let no_device = FileDeviceKey::new(s.dir.file("no-such-device.key"));
    assert_eq!(
        new_code_op(&s, &m, &no_device, &pol)
            .prepare()
            .err()
            .unwrap(),
        KeyRootError::RecoveryRequired
    );
    assert_eq!(s.snapshot(), before);

    let pending = op.prepare().unwrap();
    assert_eq!(
        s.snapshot(),
        before,
        "preparing a code must not change anything"
    );
    // a different valid code, a typo, and garbage are all refused, and nothing changes
    assert!(matches!(
        op.commit(&pending, &RecoveryCode::generate().display()),
        Err(KeyRootError::Invalid(_))
    ));
    let mut typo = pending.code().display();
    typo.replace_range(0..1, if typo.starts_with('A') { "B" } else { "A" });
    assert!(matches!(
        op.commit(&pending, &typo),
        Err(KeyRootError::Invalid(_))
    ));
    assert!(op.commit(&pending, "").is_err());
    assert_eq!(
        s.snapshot(),
        before,
        "an unconfirmed code changed the key directory"
    );

    // typed back correctly (any case, spaces instead of dashes): committed
    let typed = pending.code().display().to_lowercase().replace('-', " ");
    op.commit(&pending, &typed).unwrap();
    let leftovers: Vec<_> = fs::read_dir(s.dir.file("."))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("before-new-code"))
        .collect();
    assert!(leftovers.is_empty(), "the old recovery wrapper must not stay beside the new one: {leftovers:?}");
    // the OLD password is dead; the code works; the device still works; same root, same identity, same login hash
    let doc = RootDocument::parse(&fs::read(s.dir.auth_file()).unwrap()).unwrap();
    assert_eq!(
        doc.unlock_with_password(PASSWORD, &pol).unwrap_err(),
        KeyRootError::RecoveryFailed
    );
    let by_code = doc
        .unlock_with_password(pending.code().secret(), &pol)
        .unwrap();
    let (by_device, _) = unlock_root(&s.dir, &m, &device).unwrap();
    assert_eq!((*by_code, *by_device), ([7u8; 32], [7u8; 32]));
    assert_eq!(doc.login_hash, "$argon2id$login-hash");
    assert!(open_identity_with(&s, "machine-A", Some(&by_device), None)
        .unwrap()
        .same_identity(&id));
}

#[test]
fn the_new_code_recovers_the_same_identity_on_another_machine_and_the_old_password_does_not() {
    let (a, id) = migrated_store("machine-A");
    let m = FixedMachine("machine-A".into());
    let device = FileDeviceKey::new(a.dir.file("device.key"));
    let pol = policy();
    let op = new_code_op(&a, &m, &device, &pol);
    let pending = op.prepare().unwrap();
    op.commit(&pending, &pending.code().display()).unwrap();
    let b = copy_dir(
        &a.dir,
        &[
            "device.key",
            ".before-new-code-",
            "auth.json.legacy",
            "node_identity_9000.json.legacy",
        ],
    );
    let before = b.snapshot();
    assert_eq!(
        recover(&b, "machine-B", PASSWORD).err().unwrap(),
        KeyRootError::RecoveryFailed
    );
    assert_eq!(b.snapshot(), before);
    recover(
        &b,
        "machine-B",
        &recovery_secret(&pending.code().display().to_lowercase()),
    )
    .unwrap();
    let mb = FixedMachine("machine-B".into());
    let (root, _) =
        unlock_root(&b.dir, &mb, &FileDeviceKey::new(b.dir.file("device.key"))).unwrap();
    assert!(open_identity_with(&b, "machine-B", Some(&root), None)
        .unwrap()
        .same_identity(&id));
}

#[test]
fn a_login_password_reset_changes_only_the_login_hash() {
    let doc = document([7u8; 32], [9u8; 32], "machine-A");
    let next = doc.with_login_hash("$new-hash");
    assert_eq!(next.login_hash, "$new-hash");
    let parsed = RootDocument::parse(&next.to_json()).unwrap();
    assert_eq!(
        *parsed.unlock_with_password(PASSWORD, &policy()).unwrap(),
        [7u8; 32]
    );
    assert_eq!(
        *parsed
            .unlock_with_device(&[9u8; 32], &FixedMachine("machine-A".into()))
            .unwrap(),
        [7u8; 32]
    );
    assert_eq!(
        (
            next.root_id.clone(),
            next.device.ciphertext.clone(),
            next.recovery.ciphertext.clone()
        ),
        (
            doc.root_id.clone(),
            doc.device.ciphertext.clone(),
            doc.recovery.ciphertext.clone()
        )
    );
}

/// Run the tool with a piped stdin and answer the confirmation with whatever `answer` makes of the code it printed.
fn run_tool_with_code(
    args: &[&str],
    answer: impl Fn(&str) -> String,
) -> (i32, String, String, String) {
    use std::io::{BufRead, BufReader, Read, Write};
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_yandi-keys"))
        .args(args)
        .env_remove("YANDI_KEY_PASSWORD")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        err.read_to_string(&mut s).ok();
        s
    });
    let mut out = String::new();
    let mut shown = String::new();
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let line = line.unwrap();
        if let Some(rest) = line.split("RECOVERY CODE:").nth(1) {
            shown = rest.replace('│', "").trim().to_owned();
            stdin
                .write_all(format!("{0}\n{0}\n{0}\n", answer(&shown)) /* the tool asks up to three times */.as_bytes())
                .unwrap();
            stdin.flush().unwrap();
        }
        out.push_str(&line);
        out.push('\n');
    }
    drop(stdin);
    let status = child.wait().unwrap();
    (
        status.code().unwrap_or(-1),
        out,
        err_thread.join().unwrap(),
        shown,
    )
}

#[test]
fn core_key_prints_exactly_the_key_the_node_gives_the_core_and_nothing_else() {
    use base64::Engine;
    let (s, _id) = migrated_store("machine-A");
    let dir = s.path().to_str().unwrap().to_owned();
    let (code, out, err) = run_tool(
        &[
            "core-key",
            "--dir",
            &dir,
            "--port",
            "9000",
            "--machine-id",
            "machine-A",
        ],
        "",
    );
    assert_eq!(code, 0, "{err}");
    let (root, _) = unlock_root(
        &s.dir,
        &FixedMachine("machine-A".into()),
        &HardwareDeviceKey::new(s.dir.file("device.seal"), s.dir.file("device.key")),
    )
    .unwrap();
    let expected = base64::engine::general_purpose::STANDARD
        .encode(derive_domain(&root, DOMAIN_CORE).as_slice());
    assert_eq!(out.trim_end(), expected, "it prints the derived Core key");
    assert_eq!(out.lines().count(), 1, "one line, nothing else on stdout");
    assert!(err.is_empty(), "nothing on stderr: {err}");
    assert!(
        !out.contains(&base64::engine::general_purpose::STANDARD.encode(root.as_slice())),
        "never the root itself"
    );
    // the same directory read on another machine has no way to open it without recovery: no key, a category, no output
    let (code, out, err) = run_tool(
        &[
            "core-key",
            "--dir",
            &dir,
            "--port",
            "9000",
            "--machine-id",
            "machine-B",
        ],
        "",
    );
    assert_ne!(code, 0);
    assert!(out.is_empty(), "no key on stdout: {out}");
    assert!(err.contains("yandi-keys:"), "{err}");
}

#[test]
fn core_key_never_prints_into_a_terminal() {
    use std::os::fd::FromRawFd;
    use std::process::{Command, Stdio};
    let (s, _id) = migrated_store("machine-A");
    let dir = s.path().to_str().unwrap().to_owned();
    let (mut master, mut slave) = (0i32, 0i32);
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(rc, 0, "a pseudo-terminal is needed for this test");
    let out = Command::new(env!("CARGO_BIN_EXE_yandi-keys"))
        .args([
            "core-key",
            "--dir",
            &dir,
            "--port",
            "9000",
            "--machine-id",
            "machine-A",
        ])
        .stdin(Stdio::null())
        .stdout(unsafe { Stdio::from_raw_fd(slave) }) // the child's stdout IS a terminal
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0), "it must refuse");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("never to a terminal"), "{err}");
    // nothing that could be a key reached the terminal
    unsafe {
        libc::fcntl(master, libc::F_SETFL, libc::O_NONBLOCK);
    }
    let mut buf = [0u8; 256];
    let n = unsafe { libc::read(master, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
    assert!(n <= 0, "something was written to the terminal: {n} bytes");
    unsafe {
        libc::close(master);
    }
}

/// Каталог ключей перенесён на другой компьютер: device.seal привязан к чужому железу, но пароль верен — восстановление завершается.
#[test]
fn recovery_on_another_machine_replaces_a_foreign_hardware_seal() {
    use key_root::hw_print::HwPrint;
    let (s, id) = migrated_store("machine-A");
    let print = |nic: &str, board: &str| HwPrint {
        parts: [vec![board.into()], vec!["cpu".into()], vec![nic.into()], vec!["disk".into()]],
    };
    let hw = |p: HwPrint| {
        HardwareDeviceKey::with_print(s.dir.file("device.seal"), s.dir.file("device.key"), Box::new(move || p.clone()), 8 * 1024, 1)
    };
    // печать, сделанная на старом компьютере
    hw(print("aa:aa", "Z390")).create().unwrap();
    assert!(s.dir.file("device.seal").exists());
    let new_machine = FixedMachine("machine-B".into());
    let pol = policy();
    let device = hw(print("cc:cc", "ASUS"));
    assert!(device.load().unwrap().is_none(), "чужое железо: ключа нет");
    let report = Recovery { dir: &s.dir, port: PORT, machine: &new_machine, device: &device, password: PASSWORD, policy: &pol }
        .run()
        .expect("верный пароль завершает восстановление и на другом железе");
    assert!(device.load().unwrap().is_some(), "новая печать открывается на этом железе");
    assert_eq!(report.node_id, hex::encode(&id.address[..8]));
    assert!(std::fs::read_dir(s.dir.file(".")).unwrap().filter_map(|e| e.ok()).any(|e| e.file_name().to_string_lossy().contains("device.seal.replaced")), "старая печать сохранена копией");
}

/// Мастер-пароль, по виду похожий на код восстановления (буквы с дефисами нужной длины), восстанавливает как пароль, а не отвергается как опечатка.
#[test]
fn the_tool_accepts_a_master_password_that_only_looks_like_a_recovery_code() {
    let look_alike = "abcdefgh-ijklmnop-qrstuvwx";
    assert!(looks_like_code(look_alike) && RecoveryCode::parse(look_alike).is_err(), "условие теста");
    let master = [0x5Au8; 32];
    let s = Store::new();
    let m = FixedMachine("machine-A".into());
    let device = FileDeviceKey::new(s.dir.file("device.key"));
    let device_key = device.create().unwrap();
    let doc = RootDocument::create(&master, "$argon2id$login-hash", &device_key, device.name(), &m, look_alike, KdfParams::RECOMMENDED, &KdfPolicy::production()).unwrap();
    create_new_file(&s.dir.auth_file(), &doc.to_json()).unwrap();
    let id = material(5);
    create_new_file(&s.dir.identity_file(PORT), &seal_identity_v3(&master, &id)).unwrap();
    let c = copy_dir(&s.dir, &["device.key", ".legacy-"]);
    let dir_c = c.path().to_str().unwrap().to_owned();
    let (code, out, err) = run_tool(&["recover", "--dir", &dir_c, "--machine-id", "machine-C", "--password-stdin"], &format!("{look_alike}\n"));
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains(&hex::encode(&id.address[..8])));
    let (code, _out, err) = run_tool(&["recover", "--dir", &dir_c, "--machine-id", "machine-C", "--password-stdin"], "abcdefgh-ijklmnop-qrstuvww\n");
    assert_ne!(code, 0);
    assert!(err.contains("typo"), "неверный ввод, похожий на код, получает подсказку про опечатку: {err}");
}
