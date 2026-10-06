//! The web account, shared by both web pages: the login password, first-time creation of the keys, the reset by the recovery secret —
//! in temporary directories with a fake machine id, never the owner's real key directory.
#![cfg(unix)]
use key_root::login::*;
use key_root::*;
use std::fs;
use std::io::Write;

const LOGIN: &str = "login password 1";
const MASTER: &str = "master phrase of several words";
const FAST: KdfParams = KdfParams {
    memory_kib: 64,
    iterations: 1,
    parallelism: 1,
};

struct Dir {
    _tmp: tempfile::TempDir,
    dir: KeyDir,
}

fn dir() -> Dir {
    let tmp = tempfile::tempdir().unwrap();
    let dir = KeyDir::open(tmp.path().join("keys")).unwrap();
    Dir { _tmp: tmp, dir }
}

fn machine() -> FixedMachine {
    FixedMachine("machine-A".into())
}

fn device(d: &Dir) -> FileDeviceKey {
    FileDeviceKey::new(d.dir.file("device.key"))
}

fn made() -> (Dir, Zeroizing<[u8; 32]>) {
    let d = dir();
    let root = create_keys(
        &d.dir,
        &machine(),
        &device(&d),
        LOGIN,
        MASTER,
        FAST,
        &KdfPolicy::for_tests(),
    )
    .unwrap();
    (d, root)
}

fn snapshot(d: &Dir) -> Vec<(String, Vec<u8>)> {
    let mut all: Vec<_> = fs::read_dir(d.dir.path())
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

#[test]
fn the_keys_made_from_what_the_person_typed_open_two_ways_and_the_login_password_checks() {
    let (d, root) = made();
    let bytes = key_root::keydir::read_private_file(&d.dir.auth_file()).unwrap();
    assert_eq!(
        key_root::legacy::auth_format(&bytes).unwrap(),
        key_root::legacy::AuthFormat::RootV2
    );
    let doc = RootDocument::parse(&bytes).unwrap();
    let key = device(&d).load().unwrap().unwrap();
    assert_eq!(
        *doc.unlock_with_device(&key, &machine()).unwrap(),
        *root,
        "the device opens the root"
    );
    assert_eq!(
        *doc.unlock_with_password(MASTER, &KdfPolicy::for_tests())
            .unwrap(),
        *root,
        "the typed master password opens the root"
    );
    assert_eq!(verify_login_in(&d.dir, LOGIN), Ok(true));
    assert_eq!(verify_login_in(&d.dir, "login password 2"), Ok(false));
    assert_eq!(
        verify_login_in(&d.dir, MASTER),
        Ok(false),
        "the master password is not the login password"
    );
    assert_eq!(verify_login_in(&d.dir, ""), Ok(false));
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        !text.contains(LOGIN) && !text.contains(MASTER),
        "no typed secret is stored in the clear"
    );
}

#[test]
fn a_directory_with_no_account_is_an_error_not_a_wrong_password() {
    let d = dir();
    assert!(verify_login_in(&d.dir, LOGIN).is_err());
}

#[test]
fn creating_keys_never_overwrites_what_exists() {
    let (d, _root) = made();
    let before = snapshot(&d);
    let again = create_keys(
        &d.dir,
        &machine(),
        &device(&d),
        "another login pw",
        "another master phrase 12",
        FAST,
        &KdfPolicy::for_tests(),
    );
    assert!(again.is_err(), "a second setup is refused");
    assert_eq!(snapshot(&d), before, "and nothing changed");
    // a lone device key is somebody's key too
    let d2 = dir();
    device(&d2).create().unwrap();
    let before = snapshot(&d2);
    assert!(create_keys(
        &d2.dir,
        &machine(),
        &device(&d2),
        LOGIN,
        MASTER,
        FAST,
        &KdfPolicy::for_tests()
    )
    .is_err());
    assert_eq!(snapshot(&d2), before);
}

#[test]
fn the_rules_for_what_the_person_typed() {
    assert!(check_setup_inputs("12345678", "12345678", "twelve chars", "twelve chars").is_ok());
    assert!(check_setup_inputs("short", "short", "twelve chars", "twelve chars").is_err());
    assert!(check_setup_inputs("12345678", "12345679", "twelve chars", "twelve chars").is_err());
    assert!(check_setup_inputs("12345678", "12345678", "eleven char", "eleven char").is_err());
    assert!(check_setup_inputs("12345678", "12345678", "twelve chars", "twelve charz").is_err());
    assert!(
        check_setup_inputs(
            "same secret 12",
            "same secret 12",
            "same secret 12",
            "same secret 12"
        )
        .is_err(),
        "master must differ from login"
    );
}

#[test]
fn the_master_password_resets_the_login_password_and_touches_nothing_else() {
    let (d, root) = made();
    let policy = KdfPolicy::for_tests();
    reset_login(&d.dir, MASTER, "brand new login pw", &policy).unwrap();
    assert_eq!(verify_login_in(&d.dir, "brand new login pw"), Ok(true));
    assert_eq!(
        verify_login_in(&d.dir, LOGIN),
        Ok(false),
        "the old login password no longer works"
    );
    let doc =
        RootDocument::parse(&key_root::keydir::read_private_file(&d.dir.auth_file()).unwrap())
            .unwrap();
    let key = device(&d).load().unwrap().unwrap();
    assert_eq!(
        *doc.unlock_with_device(&key, &machine()).unwrap(),
        *root,
        "the root is unchanged"
    );
    assert_eq!(
        *doc.unlock_with_password(MASTER, &policy).unwrap(),
        *root,
        "the master password still recovers it"
    );
    let backups: Vec<_> = snapshot(&d)
        .into_iter()
        .filter(|(n, _)| n.starts_with("auth.json.before-login-reset"))
        .collect();
    assert_eq!(backups.len(), 1, "the previous file is kept");
}

#[test]
fn a_wrong_or_mistyped_secret_or_a_short_new_password_changes_nothing() {
    let (d, _root) = made();
    let policy = KdfPolicy::for_tests();
    let before = snapshot(&d);
    assert!(reset_login(
        &d.dir,
        "not the master phrase",
        "brand new login pw",
        &policy
    )
    .is_err());
    assert!(reset_login(&d.dir, "", "brand new login pw", &policy).is_err());
    assert!(reset_login(&d.dir, MASTER, "short", &policy).is_err());
    assert_eq!(snapshot(&d), before, "nothing on disk changed");
    assert_eq!(verify_login_in(&d.dir, LOGIN), Ok(true));
}

// ── the tool, as the second web page runs it ─────────────────────────────────────────────────────────────────────────

fn tool(args: &[&str], stdin_lines: &[&str]) -> (i32, String, String) {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_yandi-keys"))
        .args(args)
        .env_remove("YANDI_KEY_PASSWORD")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = stdin_lines.join("\n");
    input.push('\n');
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn the_tool_creates_the_account_checks_the_password_and_resets_it_and_never_prints_a_secret() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("keys");
    let dir = path.to_str().unwrap();
    let base = ["--dir", dir, "--machine-id", "machine-A"];
    let with = |cmd: &'static str| -> Vec<&str> {
        std::iter::once(cmd).chain(base.iter().copied()).collect()
    };

    let (code, _, err) = tool(&with("login-check"), &[LOGIN]);
    assert_eq!(
        code, 2,
        "no account yet is an error, not 'wrong password': {err}"
    );

    let (code, out, err) = tool(&with("setup"), &["a", "a", MASTER, MASTER]);
    assert_eq!(code, 1, "a login password that is too short is refused");
    assert!(err.contains("Пароль входа"), "{err}");
    assert!(!path.join("auth.json").exists(), "nothing was created");
    assert!(out.is_empty());

    let (code, out, err) = tool(&with("setup"), &[LOGIN, LOGIN, MASTER, MASTER]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.is_empty() && err.is_empty(),
        "nothing is printed on success: {out}{err}"
    );
    assert!(path.join("auth.json").exists() && path.join("device.key").exists());

    let before = fs::read(path.join("auth.json")).unwrap();
    let (code, _, err) = tool(
        &with("setup"),
        &[
            "another login pw",
            "another login pw",
            "another master phrase",
            "another master phrase",
        ],
    );
    assert_eq!(code, 1, "a second setup is refused");
    assert!(!err.contains("another"), "{err}");
    assert_eq!(
        fs::read(path.join("auth.json")).unwrap(),
        before,
        "and nothing changed"
    );

    assert_eq!(tool(&with("login-check"), &[LOGIN]).0, 0);
    let (code, out, err) = tool(&with("login-check"), &["login password 2"]);
    assert_eq!(code, 1);
    assert!(
        out.is_empty() && !err.contains("login password"),
        "{out}{err}"
    );

    let (code, _, err) = tool(
        &with("login-reset"),
        &["not the master phrase", "brand new login pw"],
    );
    assert_eq!(code, 1);
    assert!(err.contains("не подошёл"), "{err}");
    assert_eq!(
        tool(&with("login-check"), &[LOGIN]).0,
        0,
        "a refused reset changed nothing"
    );

    let (code, out, err) = tool(&with("login-reset"), &[MASTER, "brand new login pw"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.is_empty() && err.is_empty());
    assert_eq!(tool(&with("login-check"), &["brand new login pw"]).0, 0);
    assert_eq!(
        tool(&with("login-check"), &[LOGIN]).0,
        1,
        "the old password is gone"
    );
    for text in [&out, &err] {
        assert!(
            !text.contains(MASTER) && !text.contains("brand new"),
            "no secret in the output"
        );
    }
}
