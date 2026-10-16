//! Срок хранения файлов на узле: принятые и отправленные вложения не копятся вечно (раньше автоматического удаления не было).
//!
//! | где | срок (по времени последнего изменения) |
//! |---|---|
//! | `files/uploads` — копии отправленных файлов | 7 суток |
//! | `files/downloads` — принятые файлы | 30 суток |
//! | `files/*/*.part`, `*.parts/` — недокачанное и недособранное | 1 сутки |
//! | `files/cache/transfer_*.state` — контрольные точки передач | 2 суток |
//!
//! Сообщение с вложением в истории остаётся; сам файл по истечении срока исчезает с узла (в интерфейсе — «файл больше не хранится на узле»).
//! Файлы запечатаны ключом среды, поэтому удаление достаточно обычного: ключ к ним у посторонних и так не появится.

use std::path::Path;
use std::time::{Duration, SystemTime};

pub const UPLOADS_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
pub const DOWNLOADS_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
pub const PARTIAL_TTL: Duration = Duration::from_secs(24 * 3600);
pub const CACHE_TTL: Duration = Duration::from_secs(2 * 24 * 3600);

fn old(path: &Path, ttl: Duration, now: SystemTime) -> bool {
    let Ok(m) = std::fs::symlink_metadata(path) else { return false };
    let Ok(t) = m.modified() else { return false };
    now.duration_since(t).map(|age| age > ttl).unwrap_or(false)
}

/// Убрать из каталога устаревшее. Ссылки и вложенные каталоги, кроме `*.parts`, не трогаются. Возвращает, сколько удалено.
pub fn sweep_dir(dir: &Path, ttl: Duration, now: SystemTime) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for e in rd.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let partial = name.ends_with(".part") || name.contains(".tmp");
        if ft.is_dir() {
            if name.ends_with(".parts") && old(&path, PARTIAL_TTL, now) && std::fs::remove_dir_all(&path).is_ok() {
                removed += 1;
            }
        } else if ft.is_file() {
            let limit = if partial { PARTIAL_TTL } else { ttl };
            if old(&path, limit, now) && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// Все каталоги файлов узла.
pub fn sweep_all(data: &Path, now: SystemTime) -> usize {
    let files = data.join("files");
    sweep_dir(&files.join("uploads"), UPLOADS_TTL, now)
        + sweep_dir(&files.join("downloads"), DOWNLOADS_TTL, now)
        + sweep_dir(&files.join("cache"), CACHE_TTL, now)
}

/// Раз в сутки (первый проход — через минуту после старта).
pub async fn scheduler() {
    tokio::time::sleep(Duration::from_secs(60)).await;
    loop {
        let data = crate::util::data_dir::data_dir();
        let n = tokio::task::spawn_blocking(move || sweep_all(&data, SystemTime::now())).await.unwrap_or(0);
        if n > 0 {
            println!("🧹 Удалено файлов по сроку хранения: {n}");
        }
        tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aged(path: &Path, age: Duration) {
        let f = std::fs::OpenOptions::new().write(true).open(path).or_else(|_| std::fs::File::open(path)).unwrap();
        f.set_modified(SystemTime::now() - age).unwrap();
    }

    #[test]
    fn old_files_go_by_their_own_term_young_ones_and_foreign_entries_stay() {
        let d = tempfile::tempdir().unwrap();
        let files = d.path().join("files");
        for k in ["uploads", "downloads", "cache"] {
            std::fs::create_dir_all(files.join(k)).unwrap();
        }
        let day = Duration::from_secs(24 * 3600);
        let mk = |rel: &str, age: Duration| {
            let p = files.join(rel);
            std::fs::write(&p, b"x").unwrap();
            aged(&p, age);
            p
        };
        let up_old = mk("uploads/a__old.bin", 8 * day);
        let up_new = mk("uploads/b__new.bin", 6 * day);
        let down_mid = mk("downloads/c__mid.bin", 20 * day); // принятое хранится дольше отправленного
        let down_old = mk("downloads/d__old.bin", 31 * day);
        let part_old = mk("downloads/e__half.bin.part", 2 * day);
        let part_new = mk("downloads/f__half.bin.part", day / 4);
        let state_old = mk("cache/transfer_1.state", 3 * day);
        let state_new = mk("cache/transfer_2.state", day);
        // время каталога можно выставить только в Unix (в Windows каталог как файл не открыть)
        let parts_dir_old = files.join("uploads/.g__big.bin.parts");
        std::fs::create_dir_all(&parts_dir_old).unwrap();
        std::fs::write(parts_dir_old.join("0"), b"p").unwrap();
        #[cfg(unix)]
        aged(&parts_dir_old, 2 * day);
        // ссылка наружу не разыменовывается и не удаляется
        let outside = d.path().join("outside.txt");
        std::fs::write(&outside, b"keep").unwrap();
        aged(&outside, 100 * day);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, files.join("uploads/link")).unwrap();

        let n = sweep_all(d.path(), SystemTime::now());
        let expected = if cfg!(unix) { 5 } else { 4 };
        assert_eq!(n, expected, "старые: отправленный, принятый, недокачанный, контрольная точка (и недособранный каталог — в Unix)");
        for gone in [&up_old, &down_old, &part_old, &state_old] {
            assert!(!gone.exists(), "{} должен быть удалён", gone.display());
        }
        #[cfg(unix)]
        assert!(!parts_dir_old.exists(), "недособранный каталог удалён");
        for stay in [&up_new, &down_mid, &part_new, &state_new, &outside] {
            assert!(stay.exists(), "{} должен остаться", stay.display());
        }
        #[cfg(unix)]
        assert!(std::fs::symlink_metadata(files.join("uploads/link")).is_ok(), "ссылка не тронута");
        #[cfg(unix)]
        assert_eq!(sweep_all(d.path(), SystemTime::now()), 0, "повторный проход ничего не находит");
    }

    #[test]
    fn missing_directories_are_not_an_error() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(sweep_all(d.path(), SystemTime::now()), 0);
    }
}
