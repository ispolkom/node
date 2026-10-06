//! Каталог данных пользователя (переживает удаление программы): Windows `%APPDATA%\yandi`, macOS `~/Library/Application Support/yandi`,
//! Linux `$XDG_DATA_HOME/yandi` или `~/.local/share/yandi`.
use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    let home = || std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    if cfg!(windows) {
        return std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(home).join("yandi");
    }
    if cfg!(target_os = "macos") {
        return home().join("Library/Application Support/yandi");
    }
    match std::env::var_os("XDG_DATA_HOME") {
        Some(x) if !x.is_empty() => PathBuf::from(x).join("yandi"),
        _ => home().join(".local/share/yandi"),
    }
}
