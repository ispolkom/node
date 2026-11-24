// Число коммитов в версию бинаря: «0.2.0+g<N>» (по ней узел узнаёт, новее ли выпуск на сайте). Вне git — 0.
fn main() {
    let n = std::process::Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or_else(|| "0".to_string());
    println!("cargo:rustc-env=YANDI_BUILD_COUNT={n}");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");
}
