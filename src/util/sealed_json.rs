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
    /// содержимое в том виде, в каком оно лежит на диске (или было прочитано): запись — только когда оно изменилось
    on_disk: Vec<u8>,
    /// файл есть, но не открылся (повреждён, чужой ключ, не разобран): исходник не трогаем, сохранение отключено
    broken: bool,
}

#[cfg(unix)]
fn env_configured() -> bool {
    crate::closed_api::global().is_some_and(|e| e.configured())
}

/// Закрытой среды вне Unix нет: файл остаётся обычным.
#[cfg(not(unix))]
fn env_configured() -> bool {
    false
}

enum Loaded<T> {
    Ok(T, Vec<u8>),
    /// файла нет вовсе
    Missing,
    /// файл есть, но прочитать нельзя
    Broken,
}

impl<T: Serialize + DeserializeOwned + Default> SealedJson<T> {
    pub const fn new(plain: &'static str, sealed: &'static str, aad: &'static str) -> Self {
        Self::with_probe(plain, sealed, aad, env_configured)
    }

    pub const fn with_probe(plain: &'static str, sealed: &'static str, aad: &'static str, configured: fn() -> bool) -> Self {
        SealedJson { plain, sealed, aad, state: Mutex::new(State { key: None, cache: None, on_disk: Vec::new(), broken: false }), configured }
    }

    fn dir() -> std::path::PathBuf {
        crate::util::data_dir::data_dir()
    }

    fn reset(g: &mut State<T>) {
        g.cache = None;
        g.on_disk.clear();
        g.broken = false;
    }

    /// Выдан ключ области: данные читаются из запечатанного файла (открытый файл переносится).
    pub fn unlock(&self, key: [u8; 32]) {
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        g.key = Some(key);
        Self::reset(&mut g);
    }

    /// Заперто: из памяти убирается всё, файл не меняется.
    pub fn lock(&self) {
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        g.key = None;
        Self::reset(&mut g);
    }

    /// Стирание среды: оба файла удаляются.
    pub fn erase(&self) {
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        g.key = None;
        Self::reset(&mut g);
        crate::util::sealed::wipe_remove(&Self::dir().join(self.plain));
        crate::util::sealed::wipe_remove(&Self::dir().join(self.sealed));
    }

    /// Прочитать (и при необходимости изменить): файл переписывается ТОЛЬКО если содержимое действительно изменилось. Заперто: данные
    /// недоступны (изменение уходит в пустой временный объект и пропадает), повреждённый файл не затирается. Ошибки сохранения здесь не
    /// возвращаются — для изменений, о которых надо честно сообщить человеку, есть [`Self::try_with`].
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        match self.run(f) {
            Ok(r) => r,
            Err(Fail::Locked(f)) | Err(Fail::Broken(f)) | Err(Fail::Write(f, _)) => f,
        }
    }

    /// То же, но сообщает: среда заперта, файл не открывается, запись не удалась — тогда изменение в памяти НЕ остаётся.
    pub fn try_with<R>(&self, f: impl FnOnce(&mut T) -> R) -> Result<R, String> {
        self.run(f).map_err(|e| match e {
            Fail::Locked(_) => "закрытая среда заперта: разблокируйте узел".to_string(),
            Fail::Broken(_) => "файл не открывается (повреждён или не тот ключ): сохранение отключено, исходник не тронут".to_string(),
            Fail::Write(_, why) => format!("не удалось сохранить: {why}"),
        })
    }

    fn run<R>(&self, f: impl FnOnce(&mut T) -> R) -> Result<R, Fail<R>> {
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let plain_path = Self::dir().join(self.plain);
        let sealed_path = Self::dir().join(self.sealed);
        match g.key {
            Some(key) => {
                if g.cache.is_none() {
                    match self.load_sealed(&key, &plain_path, &sealed_path) {
                        Loaded::Ok(v, bytes) => {
                            g.cache = Some(v);
                            g.on_disk = bytes;
                            g.broken = false;
                        }
                        Loaded::Missing => {
                            g.cache = Some(T::default());
                            g.on_disk.clear();
                            g.broken = false;
                        }
                        Loaded::Broken => {
                            g.cache = Some(T::default());
                            g.on_disk.clear();
                            g.broken = true;
                        }
                    }
                }
                let r = f(g.cache.as_mut().expect("cache just filled"));
                if g.broken {
                    Self::reset(&mut g); // изменения поверх нечитаемого файла нигде не остаются: ни на диске, ни в памяти
                    return Err(Fail::Broken(r));
                }
                let bytes = serde_json::to_vec(g.cache.as_ref().expect("cache")).unwrap_or_default();
                if bytes != g.on_disk {
                    match crate::util::sealed::write(&sealed_path, &key, self.aad, &bytes) {
                        Ok(()) => g.on_disk = bytes,
                        Err(why) => {
                            Self::reset(&mut g); // изменение, которое не удалось сохранить, в памяти не остаётся
                            return Err(Fail::Write(r, why));
                        }
                    }
                }
                Ok(r)
            }
            None if (self.configured)() => {
                let r = f(&mut T::default());
                Err(Fail::Locked(r))
            }
            None => {
                if g.cache.is_none() {
                    match std::fs::read(&plain_path) {
                        Ok(b) => match serde_json::from_slice::<T>(&b) {
                            Ok(v) => {
                                g.cache = Some(v);
                                g.on_disk = b;
                                g.broken = false;
                            }
                            Err(_) => {
                                g.cache = Some(T::default());
                                g.on_disk.clear();
                                g.broken = true; // не разобран: исходник не затираем
                            }
                        },
                        Err(_) => {
                            g.cache = Some(T::default());
                            g.on_disk.clear();
                            g.broken = false;
                        }
                    }
                }
                let r = f(g.cache.as_mut().expect("cache just filled"));
                if g.broken {
                    Self::reset(&mut g); // изменения поверх нечитаемого файла нигде не остаются: ни на диске, ни в памяти
                    return Err(Fail::Broken(r));
                }
                let bytes = serde_json::to_vec(g.cache.as_ref().expect("cache")).unwrap_or_default();
                if bytes != g.on_disk {
                    let _ = std::fs::create_dir_all(Self::dir());
                    match crate::util::private_file::write_private(&plain_path, &bytes) {
                        Ok(()) => g.on_disk = bytes,
                        Err(why) => {
                            Self::reset(&mut g);
                            return Err(Fail::Write(r, why.to_string()));
                        }
                    }
                }
                Ok(r)
            }
        }
    }

    fn load_sealed(&self, key: &[u8; 32], plain: &std::path::Path, sealed: &std::path::Path) -> Loaded<T> {
        match std::fs::read(sealed) {
            Ok(blob) => {
                let Some(bytes) = crate::util::sealed::open(key, self.aad, &blob) else { return Loaded::Broken };
                let Ok(v) = serde_json::from_slice::<T>(&bytes) else { return Loaded::Broken };
                // открытый файл, если ещё остался от прежней версии, больше не нужен
                if plain.exists() {
                    crate::util::sealed::wipe_remove(plain);
                }
                // сравнение «изменилось ли» идёт по нашему же представлению
                let canon = serde_json::to_vec(&v).unwrap_or(bytes);
                return Loaded::Ok(v, canon);
            }
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Loaded::Broken,
            Err(_) => {}
        }
        // запечатанного файла нет: переносим прежний открытый (затирается только после проверенной записи)
        match std::fs::read(plain) {
            Ok(b) => match serde_json::from_slice::<T>(&b) {
                Ok(old) => {
                    let bytes = serde_json::to_vec(&old).unwrap_or_default();
                    if crate::util::sealed::write(sealed, key, self.aad, &bytes).is_ok() {
                        crate::util::sealed::wipe_remove(plain);
                    }
                    Loaded::Ok(old, bytes)
                }
                Err(_) => Loaded::Broken,
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded::Missing,
            Err(_) => Loaded::Broken,
        }
    }
}

enum Fail<R> {
    Locked(R),
    Broken(R),
    Write(R, String),
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

    #[test]
    fn reading_never_rewrites_the_file_and_a_broken_file_is_not_overwritten() {
        static S: SealedJson<Map> = SealedJson::with_probe("sj_ro.json", "sj_ro.enc", "sj-ro", configured);
        S.unlock([1; 32]);
        S.with(|m| {
            m.insert("a".into(), "1".into());
        });
        let before = read("sj_ro.enc").unwrap();
        for _ in 0..3 {
            S.with(|m| assert_eq!(m.len(), 1)); // чтение
        }
        assert_eq!(read("sj_ro.enc").unwrap(), before, "чтение не перезапечатывает файл (иначе каждое чтение меняло бы его байты и копию)");
        // чужой ключ: файл не открывается — исходник не тронут, изменение не принимается
        S.lock();
        S.unlock([2; 32]);
        S.with(|m| {
            assert!(m.is_empty());
            m.insert("x".into(), "y".into());
        });
        assert_eq!(read("sj_ro.enc").unwrap(), before, "повреждённый или чужой файл не затирается пустым");
        assert!(S.try_with(|m| m.insert("z".into(), "1".into())).unwrap_err().contains("не открывается"));
        // верный ключ — данные на месте
        S.lock();
        S.unlock([1; 32]);
        S.with(|m| assert_eq!(m.get("a").map(String::as_str), Some("1")));
        S.erase();
    }

    #[test]
    fn a_change_that_could_not_be_saved_or_a_locked_environment_is_reported_not_swallowed() {
        static S: SealedJson<Map> = SealedJson::with_probe("sj_fail.json", "sj_fail.enc", "sj-fail", configured);
        // заперто (ключа нет, среда создана): изменение возвращает ошибку
        assert!(S.try_with(|m| m.insert("k".into(), "v".into())).unwrap_err().contains("заперта"));
        S.unlock([3; 32]);
        // на месте файла каталог: запись невозможна — ошибка, и изменение в памяти не остаётся
        std::fs::create_dir_all(crate::util::data_dir::data_dir().join("sj_fail.enc")).unwrap();
        let err = S.try_with(|m| m.insert("k".into(), "v".into())).unwrap_err();
        assert!(!err.is_empty());
        S.with(|m| assert!(m.is_empty(), "несохранённое значение не показывается как сохранённое"));
        let _ = std::fs::remove_dir_all(crate::util::data_dir::data_dir().join("sj_fail.enc"));
        S.lock();
    }
}
