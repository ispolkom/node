//! Язык консоли (`yandi setup`, `yandi admin`): русский или английский.
//!
//! Выбирается по порядку: ключ `--lang ru|en`, переменная `YANDI_LANG`, сохранённый выбор (`~/.config/yandi/lang`), язык системы
//! (`LC_ALL`, `LC_MESSAGES`, `LANG`: начинается с `ru` — русский, иначе английский). Подтверждение «да/нет» строго по раскладке выбранного
//! языка: русский принимает только кириллицу (Д/н), английский только латиницу (Y/n). Чужая раскладка не угадывается: ввод не считается
//! ни «да», ни «нет», вопрос повторяется с подсказкой (латинская `Y` на русской раскладке печатает «н», то есть «нет»).

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Ru,
    En,
}

static LANG: AtomicU8 = AtomicU8::new(0);

// в тестах язык свой у каждого потока: параллельные тесты не меняют друг другу язык под ногами (по умолчанию русский)
#[cfg(test)]
thread_local! {
    static TEST_LANG: std::cell::Cell<Lang> = const { std::cell::Cell::new(Lang::Ru) };
}

#[cfg(not(test))]
pub fn set(l: Lang) {
    LANG.store(if l == Lang::Ru { 0 } else { 1 }, Ordering::Relaxed);
}

#[cfg(test)]
pub fn set(l: Lang) {
    TEST_LANG.with(|c| c.set(l));
}

#[cfg(not(test))]
pub fn get() -> Lang {
    if LANG.load(Ordering::Relaxed) == 0 {
        Lang::Ru
    } else {
        Lang::En
    }
}

#[cfg(test)]
pub fn get() -> Lang {
    let _ = &LANG;
    TEST_LANG.with(|c| c.get())
}

/// Выбор по месту вызова: `t!("по-русски", "in English")`. Обе ветки вычисляются лениво, поэтому годятся и `format!`.
#[macro_export]
macro_rules! t {
    ($ru:expr, $en:expr) => {
        if $crate::i18n::get() == $crate::i18n::Lang::Ru {
            $ru
        } else {
            $en
        }
    };
}

/// `ru`/`en` (и полные названия) → язык.
pub fn parse(s: &str) -> Option<Lang> {
    match s.trim().to_lowercase().as_str() {
        "ru" | "rus" | "russian" | "русский" | "рус" => Some(Lang::Ru),
        "en" | "eng" | "english" | "английский" | "англ" => Some(Lang::En),
        _ => None,
    }
}

/// Язык системы по значениям `LC_ALL`, `LC_MESSAGES`, `LANG` (первое непустое): `ru…` — русский, иначе английский.
pub fn from_locale(lc_all: Option<&str>, lc_messages: Option<&str>, lang: Option<&str>) -> Lang {
    let v = [lc_all, lc_messages, lang].into_iter().flatten().find(|s| !s.is_empty()).unwrap_or("");
    if v.to_lowercase().starts_with("ru") {
        Lang::Ru
    } else {
        Lang::En
    }
}

fn saved_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("yandi").join("lang"))
}

/// Сохранённый выбор языка (если есть).
pub fn saved() -> Option<Lang> {
    parse(&std::fs::read_to_string(saved_path()?).ok()?)
}

/// Запомнить выбор языка для следующих запусков (ошибка записи не мешает работе).
pub fn save(l: Lang) {
    if let Some(p) = saved_path() {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, if l == Lang::Ru { "ru\n" } else { "en\n" });
    }
}

/// Определить язык: ключ → `YANDI_LANG` → сохранённый → система.
pub fn detect(flag: Option<&str>) -> Result<Lang, String> {
    if let Some(f) = flag {
        return parse(f).ok_or_else(|| format!("--lang: ru | en (получено «{f}» / got “{f}”)"));
    }
    if let Some(l) = std::env::var("YANDI_LANG").ok().as_deref().and_then(parse) {
        return Ok(l);
    }
    if let Some(l) = saved() {
        return Ok(l);
    }
    let var = |k: &str| std::env::var(k).ok();
    Ok(from_locale(var("LC_ALL").as_deref(), var("LC_MESSAGES").as_deref(), var("LANG").as_deref()))
}

/// Разбор ответа «да/нет» строго по языку; пустой ввод — значение по умолчанию. `None` — не понято (в том числе чужая раскладка).
pub fn parse_yes_no(lang: Lang, input: &str, default_yes: bool) -> Option<bool> {
    let s = input.trim();
    if s.is_empty() {
        return Some(default_yes);
    }
    match lang {
        Lang::Ru => match s.to_lowercase().as_str() {
            "д" | "да" => Some(true),
            "н" | "нет" => Some(false),
            _ => None,
        },
        Lang::En => match s.to_ascii_lowercase().as_str() {
            "y" | "yes" if s.is_ascii() => Some(true),
            "n" | "no" if s.is_ascii() => Some(false),
            _ => None,
        },
    }
}

/// Вид подсказки: `[Д/н]` / `[д/Н]` / `[Y/n]` / `[y/N]` (заглавная — по умолчанию).
pub fn yes_no_hint(lang: Lang, default_yes: bool) -> &'static str {
    match (lang, default_yes) {
        (Lang::Ru, true) => "[Д/н]",
        (Lang::Ru, false) => "[д/Н]",
        (Lang::En, true) => "[Y/n]",
        (Lang::En, false) => "[y/N]",
    }
}

/// Слово подтверждения для опасных действий: вводится целиком.
pub fn confirm_word() -> &'static str {
    t!("ДА", "YES")
}

/// Верно ли введено слово подтверждения (регистр не важен, раскладка — как у выбранного языка).
pub fn is_confirm_word(typed: &str) -> bool {
    let w = confirm_word();
    typed.trim().to_lowercase() == w.to_lowercase()
}

/// Подсказка при непонятном ответе: как правильно ответить на этом языке и что проверить в раскладке.
pub fn yes_no_retry_hint(lang: Lang) -> &'static str {
    match lang {
        Lang::Ru => "Не понял ответ. Введите Д или Н (кириллицей), переключите раскладку на русскую.",
        Lang::En => "Did not understand. Type Y or N (Latin letters); switch the keyboard layout to English.",
    }
}

/// Спросить «да/нет» с повтором при непонятном ответе (до пяти раз). `read` — чтение строки (в тестах подставляется).
pub fn ask_yes_no(
    prompt_ru: &str,
    prompt_en: &str,
    default_yes: bool,
    mut read: impl FnMut(&str) -> Result<String, String>,
) -> Result<bool, String> {
    let lang = get();
    let q = format!("{} {} ", if lang == Lang::Ru { prompt_ru } else { prompt_en }, yes_no_hint(lang, default_yes));
    for _ in 0..5 {
        let a = read(&q)?;
        match parse_yes_no(lang, &a, default_yes) {
            Some(v) => return Ok(v),
            None => eprintln!("{}", yes_no_retry_hint(lang)),
        }
    }
    Err(t!("нет понятного ответа".into(), "no clear answer".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_names_and_locale() {
        assert_eq!(parse("ru"), Some(Lang::Ru));
        assert_eq!(parse(" English "), Some(Lang::En));
        assert_eq!(parse("de"), None);
        assert_eq!(from_locale(None, None, Some("ru_RU.UTF-8")), Lang::Ru);
        assert_eq!(from_locale(Some("en_US.UTF-8"), None, Some("ru_RU.UTF-8")), Lang::En, "LC_ALL главнее LANG");
        assert_eq!(from_locale(Some(""), None, Some("ru_RU.UTF-8")), Lang::Ru, "пустое значение пропускается");
        assert_eq!(from_locale(None, None, None), Lang::En, "без настроек — английский");
        assert_eq!(from_locale(None, None, Some("C")), Lang::En);
    }

    #[test]
    fn yes_no_follows_the_keyboard_layout_of_the_chosen_language_only() {
        // русский: только кириллица
        for y in ["д", "Д", "да", "ДА", "Да"] {
            assert_eq!(parse_yes_no(Lang::Ru, y, false), Some(true), "{y}");
        }
        for n in ["н", "Н", "нет", "НЕТ"] {
            assert_eq!(parse_yes_no(Lang::Ru, n, true), Some(false), "{n}");
        }
        for wrong in ["y", "Y", "yes", "n", "N", "no", "x", "1"] {
            assert_eq!(parse_yes_no(Lang::Ru, wrong, true), None, "латиница не угадывается в русском режиме: {wrong}");
        }
        // английский: только латиница; кириллическая «н» (клавиша Y в русской раскладке) и «д» — не ответ
        for y in ["y", "Y", "yes", "YES", "Yes"] {
            assert_eq!(parse_yes_no(Lang::En, y, false), Some(true), "{y}");
        }
        for n in ["n", "N", "no", "NO"] {
            assert_eq!(parse_yes_no(Lang::En, n, true), Some(false), "{n}");
        }
        for wrong in ["н", "Н", "д", "Д", "да", "нет", "x"] {
            assert_eq!(parse_yes_no(Lang::En, wrong, true), None, "кириллица не угадывается в английском режиме: {wrong}");
        }
        // Enter — значение по умолчанию на обоих языках
        assert_eq!(parse_yes_no(Lang::Ru, "  ", true), Some(true));
        assert_eq!(parse_yes_no(Lang::En, "", false), Some(false));
        assert_eq!(yes_no_hint(Lang::Ru, true), "[Д/н]");
        assert_eq!(yes_no_hint(Lang::En, false), "[y/N]");
    }

    #[test]
    fn a_wrong_layout_is_asked_again_and_the_right_answer_is_taken() {
        set(Lang::En);
        let mut answers = vec!["н".to_string(), "Y".to_string()].into_iter();
        let mut prompts = Vec::new();
        let r = ask_yes_no("Установить?", "Install?", false, |q| {
            prompts.push(q.to_string());
            answers.next().ok_or_else(|| "конец".to_string())
        });
        assert_eq!(r, Ok(true), "кириллическая «н» не принята как «нет»: вопрос повторён");
        assert_eq!(prompts, vec!["Install? [y/N] ".to_string(), "Install? [y/N] ".to_string()]);
        set(Lang::Ru);
        let mut a2 = vec!["y".to_string(), "".to_string()].into_iter();
        let r2 = ask_yes_no("Установить?", "Install?", true, |_| a2.next().ok_or_else(|| "конец".to_string()));
        assert_eq!(r2, Ok(true), "латинская y в русском режиме не принята; пустой ввод — по умолчанию «да»");
        // пять непонятных ответов подряд — отказ с ошибкой, а не молчаливое «нет»
        let r3 = ask_yes_no("Установить?", "Install?", true, |_| Ok("???".into()));
        assert!(r3.is_err());
    }

    #[test]
    fn the_confirmation_word_is_in_the_chosen_language() {
        set(Lang::Ru);
        assert_eq!(confirm_word(), "ДА");
        assert!(is_confirm_word(" да ") && is_confirm_word("ДА"));
        assert!(!is_confirm_word("YES") && !is_confirm_word("y"));
        set(Lang::En);
        assert_eq!(confirm_word(), "YES");
        assert!(is_confirm_word("yes") && is_confirm_word(" YES "));
        assert!(!is_confirm_word("ДА") && !is_confirm_word("да"), "кириллица в английском режиме не засчитывается");
        set(Lang::Ru);
    }
}
