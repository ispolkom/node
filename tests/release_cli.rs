//! Regression tests use synthetic keys and never the owner's phrase.
use std::process::Command;
use yandi::release::*;

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_yandi-release"))
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn r1_cli_bounds_manifest_before_reading_or_requesting_phrase() {
    let root = tempfile::tempdir().unwrap();
    std::fs::File::create(root.path().join(MANIFEST_NAME))
        .unwrap()
        .set_len(MAX_MANIFEST as u64 + 1)
        .unwrap();
    let output = cli(&["sign", root.path().to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("file too large"));
    assert!(!root.path().join(SIGNATURE_NAME).exists());
}

#[test]
fn r8_cli_requires_explicit_minimum_for_verify_and_update() {
    let root = tempfile::tempdir().unwrap();
    let key = ed25519_dalek::SigningKey::from_bytes(&[42; 32]);
    let vk = key.verifying_key();
    let public = hex::encode(vk.to_bytes());
    std::fs::write(root.path().join("file"), b"verified bytes").unwrap();
    let manifest = build_manifest(root.path(), "1", 1, 0, &key_id(&vk), &["file".into()]).unwrap();
    let bytes = manifest_bytes(&manifest);
    std::fs::write(root.path().join(MANIFEST_NAME), &bytes).unwrap();
    std::fs::write(
        root.path().join(SIGNATURE_NAME),
        sign(&bytes, &key).unwrap(),
    )
    .unwrap();
    for args in [
        vec!["verify", root.path().to_str().unwrap(), "--pubkey", &public],
        vec![
            "update",
            "--from",
            root.path().to_str().unwrap(),
            "--pubkey",
            &public,
        ],
    ] {
        let output = cli(&args);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("нужен --min-seq"));
    }
}

#[test]
fn r9_cli_refuses_invalid_manifest_before_phrase_and_signature_write() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(MANIFEST_NAME), b"{}").unwrap();
    let output = cli(&["sign", root.path().to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("манифест повреждён"));
    assert!(!root.path().join(SIGNATURE_NAME).exists());
}
