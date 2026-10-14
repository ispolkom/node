//! Политика содержимого сети YANDI: ссылки не передаются, вложения — только из белого списка.
//!
//! * Текст: `http(s)://`, любая `схема://`, `www.` и `имя.зона` (зона из списка настоящих доменных зон) считаются ссылкой.
//!   Ссылка не отправляется; при получении (от старого или чужого клиента) заменяется на «[ссылка удалена]».
//! * Вложения: разрешены только известные безопасные расширения; всё остальное (архивы, исполняемое, скрипты, макросы) — отказ.
//!   Имя проверяется целиком (двойное расширение `отчёт.pdf.exe` не проходит), содержимое — по первым байтам.
//!
//! Одинаковые правила продублированы в приложениях (Dart): `lib/content_policy.dart` мобилки и оболочки.

use regex::Regex;
use std::sync::OnceLock;

/// Чем заменяется ссылка у получателя.
pub const REMOVED: &str = "[ссылка удалена]";

/// Разрешённые расширения вложений (в нижнем регистре, без точки).
pub const ALLOWED_EXT: &[&str] = &[
    // картинки (при отправке перекодируются)
    "png", "jpg", "jpeg", "gif", "webp", "heic", "heif", "avif", "bmp",
    // аудио
    "mp3", "m4a", "aac", "ogg", "oga", "opus", "wav", "flac", "amr", "mid", "midi",
    // видео
    "mp4", "mov", "mkv", "webm", "3gp",
    // документы (без макросов)
    "pdf", "txt", "csv", "docx", "xlsx", "pptx", "odt", "ods", "odp", "xml",
];

/// Явно запрещённые расширения: считаются «ссылкой» и в тексте (`установка.exe`), и как вложение.
pub const BANNED_EXT: &[&str] = &[
    "exe", "bat", "cmd", "com", "msi", "scr", "pif", "dll", "sys", "lnk", "reg", "sh", "bash", "ps1", "psm1", "vbs", "vbe", "wsf", "wsh",
    "js", "jse", "mjs", "jar", "apk", "aab", "ipa", "deb", "rpm", "appimage", "py", "pyc", "rb", "pl", "php", "html", "htm", "xhtml",
    "svg", "hta", "cpl", "chm", "iso", "img", "dmg", "zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz", "cab", "doc", "xls", "ppt",
    "docm", "xlsm", "pptm", "dotm", "xlam", "rtf", "swf", "url",
];

/// Доменные зоны, по которым голое `имя.зона` считается адресом. Все двухбуквенные страновые зоны добавляются кодом.
const TLDS: &[&str] = &[
    "com", "net", "org", "info", "biz", "edu", "gov", "mil", "int", "name", "pro", "mobi", "tel", "asia", "travel", "jobs", "museum",
    "xxx", "app", "dev", "page", "blog", "shop", "store", "online", "site", "website", "space", "tech", "cloud", "live", "life", "world",
    "today", "news", "media", "video", "tube", "stream", "download", "link", "click", "xyz", "top", "club", "vip", "fun", "icu", "cyou",
    "buzz", "win", "bid", "loan", "work", "party", "date", "trade", "cam", "bar", "rest", "pub", "wiki", "cc", "tv", "ws", "gg", "ly",
    "to", "su", "рф", "москва", "онлайн", "сайт", "орг", "ком", "бел", "укр", "дети", "рус", "срб", "католик", "ею", "tk", "ml", "ga",
    "cf", "gq", "zip", "mov", "bank", "casino", "poker", "bet", "porn", "sex", "adult", "ninja", "rocks", "guru", "email", "chat",
    "social", "network", "digital", "systems", "solutions", "services", "agency", "company", "center", "group", "team", "zone", "host",
    "domains", "hosting", "software", "tools", "codes", "run", "ink", "art", "design", "photo", "pics", "pictures", "gallery", "studio",
];

fn is_tld(z: &str) -> bool {
    let z = z.to_lowercase();
    if ALLOWED_EXT.contains(&z.as_str()) && z.len() > 2 {
        return false; // pdf, docx, mp3...: расширение файла, не зона
    }
    if TLDS.contains(&z.as_str()) || BANNED_EXT.contains(&z.as_str()) {
        return true;
    }
    // все двухбуквенные латинские зоны — страновые; кроме расширений файлов, которые мы разрешаем
    z.len() == 2 && z.chars().all(|c| c.is_ascii_lowercase()) && !ALLOWED_EXT.contains(&z.as_str())
}

fn re_scheme() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)[a-z][a-z0-9+.\-]{1,15}://\S*|www\.\S+|\bmailto:\S+").unwrap())
}

fn re_domain() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)(?:[\p{L}\p{N}](?:[\p{L}\p{N}\-]{0,61}[\p{L}\p{N}])?\.)+([\p{L}\p{N}]{2,24}|xn--[a-z0-9\-]{2,40})(?:[/:?#]\S*)?").unwrap()
    })
}

/// Диапазоны ссылок в тексте (байтовые), по порядку, без пересечений.
fn link_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut v: Vec<(usize, usize)> = re_scheme().find_iter(text).map(|m| (m.start(), m.end())).collect();
    for c in re_domain().captures_iter(text) {
        let m = c.get(0).unwrap();
        let zone = c.get(1).unwrap().as_str();
        // граница слева: перед именем не буква и не цифра (иначе это хвост слова)
        let before = text[..m.start()].chars().next_back();
        if before.is_some_and(|ch| ch.is_alphanumeric() || ch == '@') {
            continue;
        }
        // сразу после зоны не продолжается слово
        if !is_tld(zone) {
            continue;
        }
        v.push((m.start(), m.end()));
    }
    v.sort();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (s, e) in v {
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// Имена, которыми нельзя называться: так подписывает сообщения сам узел («Администрация узла»), подделка этого имени — обман.
pub fn is_reserved_name(s: &str) -> bool {
    const KEYS: [&str; 7] = ["администрац", "administrat", "службаузла", "службаподдержки", "техподдержк", "support", "moderatorузла"];
    // две свёртки: латинские двойники → кириллица и кириллица → латинская транслитерация (смешанное написание ловится одной из них)
    let (cy, la) = (fold_name(s), translit(s));
    KEYS.iter().any(|k| cy.contains(&fold_name(k)) || la.contains(&translit(k)))
}

/// Кириллица → латиница по звучанию; прочие буквы — строчными, без пробелов и знаков.
fn translit(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()) {
        let t = match c {
            'а' => "a", 'б' => "b", 'в' => "v", 'г' => "g", 'д' => "d", 'е' | 'ё' => "e", 'ж' => "zh", 'з' => "z",
            'и' | 'й' | 'і' => "i", 'к' => "k", 'л' => "l", 'м' => "m", 'н' => "n", 'о' => "o", 'п' => "p", 'р' => "r",
            'с' => "s", 'т' => "t", 'у' => "u", 'ф' => "f", 'х' => "h", 'ц' => "ts", 'ч' => "ch", 'ш' => "sh",
            'щ' => "sch", 'ъ' | 'ь' => "", 'ы' => "y", 'э' => "e", 'ю' => "yu", 'я' => "ya",
            _ => { out.push(c); continue; }
        };
        out.push_str(t);
    }
    out
}

/// Имя для сравнения: без пробелов, знаков и невидимых символов, строчными, латинские двойники — кириллицей
/// (чтобы «Адми\u{200b}нистрация» и «Аdministrация» не обходили запрет).
fn fold_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .map(|c| match c {
            'a' => 'а',
            'c' => 'с',
            'e' => 'е',
            'o' => 'о',
            'p' => 'р',
            'x' => 'х',
            'y' => 'у',
            'k' => 'к',
            'm' => 'м',
            't' => 'т',
            'h' => 'н',
            'b' => 'в',
            'u' => 'и',
            'i' => 'і',
            _ => c,
        })
        .collect()
}

/// Первая найденная ссылка (для сообщения отправителю), если есть.
pub fn find_link(text: &str) -> Option<String> {
    link_ranges(text).first().map(|&(s, e)| text[s..e].to_string())
}

/// Текст без ссылок: каждая заменена на [`REMOVED`].
pub fn strip_links(text: &str) -> String {
    let ranges = link_ranges(text);
    if ranges.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (s, e) in ranges {
        out.push_str(&text[at..s]);
        out.push_str(REMOVED);
        at = e;
    }
    out.push_str(&text[at..]);
    out
}

/// Можно ли отправить текст: `Err` с понятным объяснением, если в нём ссылка.
pub fn check_text(text: &str) -> Result<(), String> {
    match find_link(text) {
        Some(l) => Err(format!("ссылки в YANDI не передаются: удалите «{}» и отправьте снова", l.chars().take(60).collect::<String>())),
        None => Ok(()),
    }
}

/// Расширение последней части имени в нижнем регистре.
fn ext_of(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let (_, ext) = base.rsplit_once('.')?;
    if ext.is_empty() {
        return None;
    }
    Some(ext.to_lowercase())
}

/// Допустимо ли имя вложения: последнее расширение в белом списке, и ни одна из внутренних частей не из запрещённых.
pub fn attachment_allowed(name: &str) -> Result<(), String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    let Some(ext) = ext_of(base) else {
        return Err("файл без расширения не передаётся".into());
    };
    if !ALLOWED_EXT.contains(&ext.as_str()) {
        return Err(format!("файлы «.{ext}» в YANDI не передаются (разрешены картинки, аудио, видео и документы без макросов)"));
    }
    // двойное расширение: отчёт.pdf.exe не пройдёт и так, но отчёт.exe.pdf — тоже подозрительно
    let parts: Vec<&str> = base.split('.').skip(1).collect();
    for p in &parts[..parts.len().saturating_sub(1)] {
        if BANNED_EXT.contains(&p.to_lowercase().as_str()) {
            return Err(format!("в имени файла «.{p}»: так замаскированные файлы не передаются"));
        }
    }
    Ok(())
}

fn is_text_like(head: &[u8]) -> bool {
    !head.is_empty() && !head.contains(&0) && std::str::from_utf8(head).is_ok() || head.iter().all(|b| *b != 0) && head.len() < 4
}

/// Первая запись ZIP-контейнера должна быть из набора документа: произвольный архив, переименованный в .docx, не проходит.
/// Видна только голова файла: если имя записи не уместилось — решает подпись.
fn office_container_ok(ext: &str, head: &[u8]) -> bool {
    if !head.starts_with(b"PK\x03\x04") {
        return false;
    }
    if head.len() < 30 {
        return true;
    }
    let n = u16::from_le_bytes([head[26], head[27]]) as usize;
    if head.len() < 30 + n {
        return true;
    }
    let name = String::from_utf8_lossy(&head[30..30 + n]).to_lowercase();
    if matches!(ext, "odt" | "ods" | "odp") {
        return name == "mimetype";
    }
    name == "[content_types].xml" || ["_rels/", "docprops/", "word/", "xl/", "ppt/", "customxml/"].iter().any(|p| name.starts_with(p))
}

/// Соответствует ли содержимое (первые байты) заявленному расширению; исполняемое под любым именем отклоняется.
pub fn sniff_ok(name: &str, head: &[u8]) -> Result<(), String> {
    attachment_allowed(name)?;
    let ext = ext_of(name).unwrap_or_default();
    if ext == "amr" && head.starts_with(b"#!AMR") {
        return Ok(()); // «#!AMR» — подпись аудио, а не скрипт
    }
    // исполняемое и скрипты под любым именем
    if head.starts_with(b"MZ") || head.starts_with(b"\x7fELF") || head.starts_with(b"#!") || head.starts_with(b"\xca\xfe\xba\xbe") {
        return Err("внутри исполняемый файл под видом документа: не передаётся".into());
    }
    let starts = |p: &[u8]| head.starts_with(p);
    let has = |p: &[u8], from: usize| head.len() >= from + p.len() && &head[from..from + p.len()] == p;
    let ok = match ext.as_str() {
        "png" => starts(b"\x89PNG\r\n\x1a\n"),
        "jpg" | "jpeg" => starts(b"\xff\xd8\xff"),
        "gif" => starts(b"GIF87a") || starts(b"GIF89a"),
        "webp" => starts(b"RIFF") && has(b"WEBP", 8),
        "bmp" => starts(b"BM"),
        "heic" | "heif" | "avif" | "mp4" | "mov" | "m4a" | "3gp" => has(b"ftyp", 4),
        "pdf" => starts(b"%PDF-"),
        "mp3" => starts(b"ID3") || (head.len() > 2 && head[0] == 0xff && head[1] & 0xe0 == 0xe0),
        "aac" => head.len() > 2 && head[0] == 0xff && head[1] & 0xf0 == 0xf0,
        "ogg" | "oga" | "opus" => starts(b"OggS"),
        "wav" => starts(b"RIFF") && has(b"WAVE", 8),
        "flac" => starts(b"fLaC"),
        "amr" => starts(b"#!AMR"),
        "mid" | "midi" => starts(b"MThd"),
        "mkv" | "webm" => starts(b"\x1a\x45\xdf\xa3"),
        "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" => office_container_ok(&ext, head),
        "txt" | "csv" | "xml" => is_text_like(head),
        _ => false,
    };
    if ok { Ok(()) } else { Err(format!("содержимое не похоже на «.{ext}»: файл не передаётся")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip_head(first: &str) -> Vec<u8> {
        let mut v = b"PK\x03\x04".to_vec();
        v.extend_from_slice(&[0u8; 22]);
        v.extend_from_slice(&(first.len() as u16).to_le_bytes());
        v.extend_from_slice(&[0, 0]);
        v.extend_from_slice(first.as_bytes());
        v.extend_from_slice(&[0u8; 20]);
        v
    }

    #[test]
    fn plain_archive_is_not_an_office_document() {
        assert!(sniff_ok("a.docx", &zip_head("[Content_Types].xml")).is_ok());
        assert!(sniff_ok("a.docx", &zip_head("word/document.xml")).is_ok());
        assert!(sniff_ok("a.docx", &zip_head("setup.exe")).is_err(), "произвольный архив под .docx");
        assert!(sniff_ok("a.odt", &zip_head("mimetype")).is_ok());
        assert!(sniff_ok("a.odt", &zip_head("run.js")).is_err());
    }

    #[test]
    fn links_are_found_in_all_common_shapes() {
        for t in [
            "смотри https://example.com/a?b=1",
            "http://x.y",
            "зайди на www.example.org",
            "пиши на example.com",
            "ЗАЙДИ example.RU тут",
            "сайт сайт.рф",
            "ftp://host/file",
            "mailto:a@b.com",
            "скачай установка.exe",
            "a.b.c.example.net/путь",
            "t.me",
            "bit.ly/abc",
        ] {
            assert!(find_link(t).is_some(), "должна находиться ссылка: {t}");
        }
    }

    #[test]
    fn ordinary_text_is_not_a_link() {
        for t in [
            "т.е. вот так",
            "версия v1.2.3 готова",
            "Привет.Как дела",
            "1.5 литра",
            "и.т.д.",
            "Mr.Smith пришёл",
            "отчёт.pdf во вложении",
            "фото.png и видео.mp4",
            "лист.docx",
            "ёлка.да",
            "число 3.14",
            "e.g. пример",
            "приглашение YANDI-GROUP-1:abc.def",
        ] {
            assert!(find_link(t).is_none(), "не должна находиться ссылка: {t} -> {:?}", find_link(t));
        }
    }

    #[test]
    fn links_are_replaced_at_the_receiver() {
        assert_eq!(strip_links("раз https://a.b/c два www.x.ru три"), "раз [ссылка удалена] два [ссылка удалена] три");
        assert_eq!(strip_links("без ссылок"), "без ссылок");
        assert!(check_text("тут example.com").is_err());
        assert!(check_text("всё чисто").is_ok());
    }

    #[test]
    fn administration_names_are_reserved() {
        assert!(is_reserved_name("Администрация узла"));
        assert!(is_reserved_name("ADMINISTRATOR Yandi"));
        assert!(is_reserved_name("Техподдержка"));
        assert!(!is_reserved_name("Аня"));
        assert!(is_reserved_name("Адми\u{200b}нистрация узла"), "невидимый символ внутри");
        assert!(is_reserved_name("Аdministrация"), "латинский двойник");
        assert!(is_reserved_name("А д м и н и с т р а ц и я"), "пробелы между буквами");
        assert!(is_reserved_name("Служба_узла"));
    }

    #[test]
    fn attachment_names_follow_the_allow_list() {
        for ok in ["фото.PNG", "отчёт.pdf", "голос.m4a", "клип.MP4", "таблица.xlsx", "heic.HEIC"] {
            assert!(attachment_allowed(ok).is_ok(), "{ok}");
        }
        for bad in ["вирус.exe", "скрипт.sh", "архив.zip", "макрос.docm", "старый.doc", "страница.html", "x.tgfr", "без_расширения", "картинка.svg", "отчёт.pdf.exe", "отчёт.exe.pdf", "a.bat"] {
            assert!(attachment_allowed(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn content_must_match_the_extension_and_executables_are_refused() {
        assert!(sniff_ok("a.png", b"\x89PNG\r\n\x1a\nrest").is_ok());
        assert!(sniff_ok("a.pdf", b"%PDF-1.7").is_ok());
        assert!(sniff_ok("a.jpg", b"\xff\xd8\xff\xe0").is_ok());
        assert!(sniff_ok("a.txt", "привет".as_bytes()).is_ok());
        assert!(sniff_ok("a.png", b"MZ\x90\x00").is_err(), "exe под видом картинки");
        assert!(sniff_ok("a.pdf", b"#!/bin/sh\n").is_err(), "скрипт под видом pdf");
        assert!(sniff_ok("a.png", b"%PDF-1.7").is_err(), "содержимое не того типа");
        assert!(sniff_ok("a.txt", b"abc\0def").is_err(), "бинарное под видом текста");
        assert!(sniff_ok("v.amr", b"#!AMR\n").is_ok());
    }
}
