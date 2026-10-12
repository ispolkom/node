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

/// Создаёт каталог данных (и недостающие родителей) приватным, 0700, до того как в нём что-то запишут:
/// сокет оболочки не принимает каталог с чужими правами и никогда не чинит его сам.
/// Уже существующие каталоги не трогает.
pub fn ensure_private() -> std::io::Result<()> {
    ensure_private_at(&data_dir())
}

fn ensure_private_at(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_new_data_directory_is_private_and_an_existing_one_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        let leaf = root.path().join("a/b/yandi");
        super::ensure_private_at(&leaf).unwrap();
        for p in [&leaf, &root.path().join("a/b"), &root.path().join("a")] {
            assert_eq!(std::fs::metadata(p).unwrap().permissions().mode() & 0o777, 0o700);
        }
        std::fs::set_permissions(&leaf, std::fs::Permissions::from_mode(0o755)).unwrap();
        super::ensure_private_at(&leaf).unwrap();
        assert_eq!(std::fs::metadata(&leaf).unwrap().permissions().mode() & 0o777, 0o755);
    }
}
