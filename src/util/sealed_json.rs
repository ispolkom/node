//! Небольшой JSON-файл узла, который по правилам закрытой среды не должен лежать открытым.
//!
//! Режимы: без закрытой среды — обычный файл (как раньше); среда создана и открыта (выдан ключ) — файл запечатан ключом, прежняя
//! открытая копия переносится и затирается; среда заперта — ничего не читается и не пишется, в памяти пусто.

use serde::{de::DeserializeOwned, Serialize};
use std::sync::Mutex;

pub struct SealedJson<T> {
    plain: &'static str,
    sealed: &'static str,
    aad: &'static str,
    state: Mutex<State<T>>,
    /// «Среда создана»: подменяется в тестах; по умолчанию спрашивается у закрытой среды узла.
    configured: fn() -> bool,
}

struct State<T> {
    key: Option<[u8; 32]>,
    cache: Option<T>,
}

fn env_configured() -> bool {
    crate::closed_api::global().is_some_and(|e| e.configured())
}

impl<T: Serialize + DeserializeOwned + Default> SealedJson<T> {
    pub const fn new(plain: &'static str, sealed: &'static str, aad: &'static str) -> Self {
        Self::with_probe(plain, sealed, aad, env_configured)
    }

    pub const fn with_probe(plain: &'static str, sealed: &'static str, aad: &'static str, configured: fn() -> bool) -> Self {
        SealedJson { plain, sealed, aad, state: Mutex::new(State { key: None, cache: None }), configured }
    }

    fn dir() -> std::path::PathBuf {
        crate::util::data_dir::data_dir()
    }

    /// Выдан ключ области: данные читаются из запечатанного файла (открытый файл переносится).
    pub fn unlock(&self, key: [u8; 32]) {
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        g.key = Some(key);
        g.cache = None;
    }

    /// Заперто: из памяти убирается всё, файл не меняется.
    pub fn lock(&self) {
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        g.key = None;
        g.cache = None;
    }

    /// Стирание среды: оба файла удаляются.
    pub fn erase(&self) {
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        g.key = None;
        g.cache = None;
        crate::util::sealed::wipe_remove(&Self::dir().join(self.plain));
        crate::util::sealed::wipe_remove(&Self::dir().join(self.sealed));
    }

    /// Прочитать-изменить-записать. Заперто: данные недоступны (изменение уходит в пустой временный объект и пропадает).
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let plain_path = Self::dir().join(self.plain);
        let sealed_path = Self::dir().join(self.sealed);
        match g.key {
            Some(key) => {
                if g.cache.is_none() {
                    g.cache = Some(self.load_sealed(&key, &plain_path, &sealed_path));
                }
                let r = f(g.cache.as_mut().expect("cache just filled"));
                if let Ok(bytes) = serde_json::to_vec(g.cache.as_ref().expect("cache")) {
                    let _ = crate::util::sealed::write(&sealed_path, &key, self.aad, &bytes);
                }
                r
            }
            None if (self.configured)() => f(&mut T::default()),
            None => {
                if g.cache.is_none() {
                    g.cache = Some(std::fs::read_to_string(&plain_path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default());
                }
                let r = f(g.cache.as_mut().expect("cache just filled"));
                if let Ok(bytes) = serde_json::to_vec(g.cache.as_ref().expect("cache")) {
                    let _ = std::fs::create_dir_all(Self::dir());
                    let _ = crate::util::private_file::write_private(&plain_path, &bytes);
                }
                r
            }
        }
    }

    fn load_sealed(&self, key: &[u8; 32], plain: &std::path::Path, sealed: &std::path::Path) -> T {
        if let Ok(blob) = std::fs::read(sealed) {
            if let Some(v) = crate::util::sealed::open(key, self.aad, &blob).and_then(|b| serde_json::from_slice(&b).ok()) {
                // открытый файл, если ещё остался от прежней версии, больше не нужен
                if plain.exists() {
                    crate::util::sealed::wipe_remove(plain);
                }
                return v;
            }
            return T::default();
        }
        // запечатанного файла нет: переносим прежний открытый (затирается только после проверенной записи)
        if let Some(old) = std::fs::read(plain).ok().and_then(|b| serde_json::from_slice::<T>(&b).ok()) {
            if let Ok(bytes) = serde_json::to_vec(&old) {
                if crate::util::sealed::write(sealed, key, self.aad, &bytes).is_ok() {
                    crate::util::sealed::wipe_remove(plain);
                }
            }
            return old;
        }
        T::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    type Map = HashMap<String, String>;
    fn configured() -> bool {
        true
    }
    fn not_configured() -> bool {
        false
    }

    fn read(name: &str) -> Option<Vec<u8>> {
        std::fs::read(crate::util::data_dir::data_dir().join(name)).ok()
    }

    #[test]
    fn without_a_closed_environment_the_file_stays_plain_as_before() {
        static S: SealedJson<Map> = SealedJson::with_probe("sj_plain.json", "sj_plain.enc", "sj-plain", not_configured);
        S.with(|m| {
            m.insert("a".into(), "секрет".into());
        });
        assert!(String::from_utf8(read("sj_plain.json").unwrap()).unwrap().contains("секрет"));
        assert!(read("sj_plain.enc").is_none());
    }

    #[test]
    fn with_a_closed_environment_data_is_sealed_old_plain_copy_is_moved_and_wiped_and_lock_hides_it() {
        static S: SealedJson<Map> = SealedJson::with_probe("sj_env.json", "sj_env.enc", "sj-env", configured);
        // прежняя версия оставила открытый файл
        std::fs::write(crate::util::data_dir::data_dir().join("sj_env.json"), r#"{"старый":"ключ-контакта"}"#.as_bytes()).unwrap();
        // пока среда заперта (ключа нет) — ничего не читается и не пишется
        S.with(|m| {
            assert!(m.is_empty());
            m.insert("х".into(), "потеряется".into());
        });
        assert!(read("sj_env.enc").is_none());
        S.unlock([7; 32]);
        S.with(|m| {
            assert_eq!(m.get("старый").map(String::as_str), Some("ключ-контакта")); // прежние данные перенесены
            m.insert("новый".into(), "секретное-слово".into());
        });
        assert!(read("sj_env.json").is_none(), "открытая копия должна быть затёрта и удалена");
        let blob = read("sj_env.enc").unwrap();
        assert!(!blob.windows(15).any(|w| w == "секретное-слово".as_bytes()), "на диске запечатанный файл без открытого текста");
        S.lock();
        S.with(|m| assert!(m.is_empty())); // заперто — данных нет
        S.unlock([7; 32]);
        S.with(|m| assert_eq!(m.get("новый").map(String::as_str), Some("секретное-слово")));
        S.lock();
        S.unlock([8; 32]); // чужой ключ данные не открывает
        S.with(|m| assert!(m.is_empty()));
        S.erase();
        assert!(read("sj_env.enc").is_none());
    }
}
