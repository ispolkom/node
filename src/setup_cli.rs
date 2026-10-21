//! `yandi setup`: новая настройка узла или восстановление по сид-фразе (с необязательным контрольным словом) из консоли, на двух языках.
//!
//! Сид-фраза полностью определяет узел: тот же сид (и слово) на другой машине даёт ту же личность, то же доверие и тот же ключ подписи списка узлов.
//! Файл связей `.yandibak` необязателен: из него возвращаются сопряжённые телефоны, контакты и группы. Перед записью чего-либо при восстановлении
//! показывается номер получившегося узла, чтобы опечатка в слове не создала молча чужой узел.
//!
//! Порядок секретов на стандартном вводе (`--password-stdin`): фраза (только `--restore`), контрольное слово (только `--control-word-stdin`),
//! пароль входа. Всё остальное спрашивается в терминале или задаётся ключами.
use crate::admin_cli::{check_password_pair, error_text, roundtrip, write_private_new, Secrets, Session};
use crate::t;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

pub struct Opts {
    pub socket: PathBuf,
    /// 12 / 18 / 24; `None` — спросить (по умолчанию 24)
    pub words: Option<u32>,
    pub seed_file: Option<PathBuf>,
    pub password_stdin: bool,
    pub control_word_stdin: bool,
    pub restore: bool,
    /// только проверить фразу и слово: показать номер узла, ничего не делая (узел не нужен)
    pub check: bool,
    /// папка для копий связей (итоговый путь …/yandi-node)
    pub backup_dir: Option<String>,
    pub no_backup_dir: bool,
    /// файл копии `.yandibak` или папка `…/yandi-node` (восстановление)
    pub backup_file: Option<String>,
    /// не спрашивать подтверждений (сценарии): номер узла и риск копий на системном диске принимаются
    pub yes: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            socket: crate::admin_cli::default_socket(),
            words: None,
            seed_file: None,
            password_stdin: false,
            control_word_stdin: false,
            restore: false,
            check: false,
            backup_dir: None,
            no_backup_dir: false,
            backup_file: None,
            yes: false,
        }
    }
}

fn tty() -> bool {
    unsafe { libc::isatty(0) == 1 && libc::isatty(2) == 1 }
}

/// Номер узла для глаз: группами по четыре знака.
pub fn show_id(id: &str) -> String {
    id.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(" ")
}

/// Текст ошибки фразы на языке консоли.
pub fn seed_error_text(e: key_root::SeedError) -> String {
    use key_root::SeedError::*;
    match e {
        WrongCount(n) => t!(format!("Нужно 12, 18 или 24 слова, введено {n}"), format!("12, 18 or 24 words are needed, {n} given")),
        UnknownWord(i) => t!(format!("Слово №{i} не из списка"), format!("Word #{i} is not in the list")),
        Checksum => t!("Фраза не проходит проверку: опечатка или слова переставлены", "The phrase fails its check: a typo or swapped words").to_string(),
        ControlWordSpaces => t!("Контрольное слово — одно слово, без пробелов", "The control word is ONE word, without spaces").to_string(),
        ControlWordShort => t!(
            format!("Контрольное слово не короче {} знаков", key_root::CONTROL_WORD_MIN),
            format!("The control word must be at least {} characters", key_root::CONTROL_WORD_MIN)
        ),
        ControlWordLong => t!(
            format!("Контрольное слово не длиннее {} знаков", key_root::CONTROL_WORD_MAX),
            format!("The control word must be at most {} characters", key_root::CONTROL_WORD_MAX)
        ),
    }
}

/// Слово, которое легко угадать: предупреждение, не запрет.
pub fn weak_control_word(w: &str) -> bool {
    let lower = w.to_lowercase();
    const COMMON: &[&str] = &[
        "password", "passw0rd", "qwertyuiop", "qwerty123", "12345678", "123456789", "1234567890", "iloveyou", "admin1234", "letmein12", "welcome1", "abc12345",
        "1q2w3e4r", "1qaz2wsx", "qazwsxedc", "football", "baseball", "sunshine", "princess", "trustno1", "пароль12", "пароль123", "парольпароль", "йцукенгшщз",
        "йцукен123", "привет123", "секретное", "контрольное", "контрольноеслово", "controlword",
    ];
    let unique: std::collections::HashSet<char> = lower.chars().collect();
    COMMON.contains(&lower.as_str()) || unique.len() <= 3 || lower.chars().all(|c| c.is_ascii_digit())
}

fn yn(ru: &str, en: &str, default_yes: bool) -> Result<bool, String> {
    if !tty() {
        return Ok(default_yes);
    }
    crate::i18n::ask_yes_no(ru, en, default_yes, |q| Secrets::ask_visible(q))
}

/// Фраза и слово одной строкой для узла.
fn joined(phrase: &str, word: &str) -> String {
    key_root::join_phrase(phrase, word)
}

/// Номер узла по фразе и слову (локально, без узла).
pub fn node_number(phrase: &str, word: &str) -> Result<String, String> {
    let seed = key_root::Seed::parse(&joined(phrase, word)).map_err(seed_error_text)?;
    let id = crate::core::identity::NodeIdentity::node_id_for_root(&seed.root());
    Ok(hex::encode(&id.0[..8]))
}

/// Контрольное слово при новой настройке: необязательное, дважды, с предупреждениями.
fn ask_new_control_word(o: &Opts) -> Result<String, String> {
    let secrets = Secrets { from_stdin: true, preset: None };
    if o.control_word_stdin {
        let w = secrets.ask("")?;
        if w.trim().is_empty() {
            return Ok(String::new());
        }
        key_root::normalize_control_word(&w).map_err(seed_error_text)?;
        return Ok(w);
    }
    if !tty() {
        return Ok(String::new());
    }
    eprintln!(
        "\n{}",
        t!(
            "Контрольное слово (необязательно): ОДНО слово, которое придумаете и запомните сами. Без него сид-фраза НЕ восстановит узел — даже если листок найдут.\nХраните слово отдельно от сид-фразы. Забыли слово — узел потерян безвозвратно. Enter — не задавать.",
            "Control word (optional): ONE word you invent and remember yourself. Without it the seed phrase will NOT restore the node — even if the paper is found.\nKeep the word apart from the seed phrase. If you forget the word, the node is lost for good. Enter — skip."
        )
    );
    let secrets = Secrets { from_stdin: false, preset: None };
    loop {
        let w = secrets.ask(t!("Контрольное слово: ", "Control word: "))?;
        if w.trim().is_empty() {
            return Ok(String::new());
        }
        if let Err(e) = key_root::normalize_control_word(&w) {
            eprintln!("{}", seed_error_text(e));
            continue;
        }
        if weak_control_word(&w) && !yn("Слово слабое: его легко угадать. Лучше 10 и больше случайных знаков. Оставить это слово?", "The word is weak: easy to guess. 10+ random characters are better. Keep this word?", false)? {
            continue;
        }
        let again = secrets.ask(t!("Ещё раз: ", "Again: "))?;
        if again != w {
            eprintln!("{}", t!("Слова не совпали, начнём заново.", "The words differ, let's start over."));
            continue;
        }
        return Ok(w);
    }
}

/// Контрольное слово при восстановлении: спрашиваем, было ли оно.
fn ask_restore_control_word(o: &Opts) -> Result<String, String> {
    if o.control_word_stdin {
        return Ok(Secrets { from_stdin: true, preset: None }.ask("")?);
    }
    if !tty() {
        return Ok(String::new());
    }
    if !yn("Было ли контрольное слово при настройке?", "Was there a control word at setup?", false)? {
        return Ok(String::new());
    }
    Secrets { from_stdin: false, preset: None }.ask(t!("Контрольное слово: ", "Control word: "))
}

fn print_id_block(id: &str) {
    eprintln!("{}", t!(format!("Номер узла: {}", show_id(id)), format!("Node number: {}", show_id(id))));
}

/// Проверить фразу и слово, ничего не делая: показать номер узла.
fn check_flow(o: &Opts) -> Result<(), String> {
    let secrets = Secrets { from_stdin: o.password_stdin, preset: None };
    let phrase = crate::admin_cli::normalize_phrase(&secrets.ask(t!("Сид-фраза: ", "Seed phrase: "))?);
    key_root::Seed::parse(&phrase).map_err(seed_error_text)?;
    let word = ask_restore_control_word(o)?;
    let id = node_number(&phrase, &word)?;
    print_id_block(&id);
    eprintln!("{}", t!("Сверьте номер с записанным. Он не секрет, но на листок с сид-фразой его не пишут.", "Compare the number with the one you wrote down. It is not a secret, but it does not go on the seed paper."));
    Ok(())
}

pub fn run(o: Opts) -> Result<(), String> {
    if o.check {
        return check_flow(&o);
    }
    let t_out = Duration::from_secs(120);
    let (code, st) = roundtrip(&o.socket, "GET", "/api/auth/state", None, None, t_out)?;
    if code != 200 {
        return Err(error_text(code, &st));
    }
    if st["setup_required"] != true {
        return Err(t!("узел уже настроен (для восстановления из копии без новой настройки: yandi backup restore, затем запуск службы)", "the node is already set up (to restore from a backup file without a new setup: yandi backup restore, then start the service)").into());
    }
    let mut restore = o.restore;
    if !restore && tty() && o.words.is_none() && o.seed_file.is_none() && !o.password_stdin {
        eprintln!("{}", t!("1 — новая настройка\n2 — восстановление по сид-фразе (и контрольному слову)", "1 — new setup\n2 — restore with a seed phrase (and a control word)"));
        let a = Secrets::ask_visible(t!("Что делаем? [1/2] (1): ", "What now? [1/2] (1): "))?;
        restore = a.trim() == "2";
    }
    if restore { restore_flow(&o) } else { new_flow(&o) }
}

fn ask_passwords(o: &Opts, secrets: &Secrets, min: usize, with_admin: bool) -> Result<(String, String), String> {
    let pw = secrets.ask(&t!(format!("Пароль владельца (не короче {min} знаков): "), format!("Owner password (at least {min} characters): ")))?;
    let again = if o.password_stdin { pw.clone() } else { secrets.ask(t!("Пароль ещё раз: ", "Password again: "))? };
    check_password_pair(&pw, &again, min)?;
    let admin = if o.password_stdin || !with_admin {
        String::new()
    } else {
        let a = Secrets::ask_visible(t!("Отдельный пароль администратора? (пусто — тот же, иначе введите следом): ", "A separate administrator password? (empty — the same, otherwise type it next): "))?;
        if a.trim().is_empty() {
            String::new()
        } else {
            let p1 = secrets.ask(t!("Пароль администратора: ", "Administrator password: "))?;
            let p2 = secrets.ask(t!("Ещё раз: ", "Again: "))?;
            check_password_pair(&p1, &p2, min)?;
            p1
        }
    };
    Ok((pw, admin))
}

fn new_flow(o: &Opts) -> Result<(), String> {
    let t_out = Duration::from_secs(120);
    let socket = &o.socket;
    let tty_ok = tty();
    if !tty_ok && o.seed_file.is_none() {
        return Err(t!("нет терминала: фразу восстановления некуда безопасно показать; запустите в терминале или добавьте --seed-file", "no terminal: nowhere safe to show the recovery phrase; run in a terminal or add --seed-file").into());
    }
    let secrets = Secrets { from_stdin: o.password_stdin, preset: None };
    // 1. длина фразы
    let words = match o.words {
        Some(w) => w,
        None if tty_ok => {
            eprintln!("{}", t!("Длина сид-фразы: 12 слов (128 бит), 18 (192 бита) или 24 (256 бит, рекомендуется).", "Seed phrase length: 12 words (128 bits), 18 (192 bits) or 24 (256 bits, recommended)."));
            loop {
                let a = Secrets::ask_visible(t!("Слов: 12 / 18 / 24 [24]: ", "Words: 12 / 18 / 24 [24]: "))?;
                match a.trim() {
                    "" | "24" => break 24,
                    "12" => break 12,
                    "18" => break 18,
                    _ => eprintln!("12 | 18 | 24"),
                }
            }
        }
        None => 24,
    };
    // 2. фраза от узла
    let (code, seed) = roundtrip(socket, "POST", "/api/auth/seed/new", Some(&json!({ "words": words })), None, t_out)?;
    if code != 200 {
        return Err(error_text(code, &seed));
    }
    let list: Vec<String> = seed["words"].as_array().ok_or(t!("узел не вернул слова", "the node did not return the words"))?.iter().filter_map(|w| w.as_str().map(String::from)).collect();
    if list.len() as u32 != words {
        return Err(t!("узел вернул неверное число слов", "the node returned a wrong number of words").into());
    }
    let min = seed["min_password"].as_u64().unwrap_or(8) as usize;
    let phrase = list.join(" ");
    // 3. пароли и контрольное слово
    let (pw, admin) = ask_passwords(o, &secrets, min, true)?;
    let word = ask_new_control_word(o)?;
    // 4. показать фразу и убедиться, что её записали
    if let Some(path) = &o.seed_file {
        write_private_new(path, &format!("{phrase}\n"))?;
        eprintln!("{}", t!(format!("Фраза записана в {} (права 0600). Перепишите её и удалите файл.", path.display()), format!("The phrase is written to {} (mode 0600). Copy it and delete the file.", path.display())));
    } else {
        eprintln!("{}", t!(format!("\nСид-фраза ({words} слов). Запишите её на бумаге, порядок важен:\n"), format!("\nSeed phrase ({words} words). Write it on paper, the order matters:\n")));
        for (i, w) in list.iter().enumerate() {
            eprintln!("  {:>2}. {w}", i + 1);
        }
        eprintln!();
        let typed = Secrets::ask_visible(t!("Для проверки введите фразу целиком: ", "To verify, type the whole phrase: "))?;
        if crate::admin_cli::normalize_phrase(&typed) != phrase {
            return Err(t!("фраза введена неверно: ничего не создано, запустите мастер снова", "the phrase was typed wrongly: nothing was created, run the wizard again").into());
        }
    }
    let id = node_number(&phrase, &word)?;
    let master = joined(&phrase, &word);
    // 5. создание ключей, ожидание запуска узла, закрытая среда
    let mut body = json!({
        "login_password": pw, "login_password_repeat": pw,
        "master_password": master, "master_password_repeat": master,
    });
    if !admin.is_empty() {
        body["admin_password"] = json!(admin);
        body["admin_password_repeat"] = json!(admin);
    }
    let (code, v) = roundtrip(socket, "POST", "/api/auth/setup", Some(&body), None, Duration::from_secs(300))?;
    if code != 200 {
        return Err(error_text(code, &v));
    }
    eprintln!("{}", t!("Ключи созданы. Узел запускается…", "Keys created. The node is starting…"));
    wait_started(socket)?;
    let (code, v) = roundtrip(socket, "POST", "/api/vault/setup", Some(&json!({ "password": pw, "phrase": master })), None, Duration::from_secs(300))?;
    if code != 200 {
        return Err(t!(format!("ключи созданы, но закрытая среда не создана: {}", error_text(code, &v)), format!("the keys are created, but the closed environment is not: {}", error_text(code, &v))).into());
    }
    eprintln!("{}", t!("Закрытая среда создана.", "The closed environment is created."));
    print_id_block(&id);
    eprintln!("{}", t!(
        "Номер узла не секрет. Запишите его отдельно от листка с сид-фразой: при восстановлении по нему видно, что фраза и слово введены верно.",
        "The node number is not a secret. Write it down apart from the seed paper: at restore it shows that the phrase and word were entered correctly."
    ));
    if word.is_empty() {
        eprintln!("{}", t!("Контрольного слова нет: любой, кто увидит листок, получит ваш узел.", "No control word: anyone who sees the paper gets your node."));
    } else {
        eprintln!("{}", t!("Контрольное слово: запомните или храните отдельно от сид-фразы. Без него узел не восстановить.", "Control word: remember it or keep it apart from the seed phrase. The node cannot be restored without it."));
    }
    backup_step(o, None, if admin.is_empty() { &pw } else { &admin })?;
    finish_message();
    Ok(())
}

fn wait_started(socket: &std::path::Path) -> Result<(), String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    loop {
        std::thread::sleep(Duration::from_secs(1));
        if let Ok((200, st)) = roundtrip(socket, "GET", "/api/auth/state", None, None, Duration::from_secs(10)) {
            if st["setup_required"] == false {
                return Ok(());
            }
        }
        if std::time::Instant::now() > deadline {
            return Err(t!("узел не запустился за 120 секунд: проверьте журнал службы (journalctl -u yandi-node@ПОЛЬЗОВАТЕЛЬ)", "the node did not start within 120 seconds: check the service log (journalctl -u yandi-node@USER)").into());
        }
    }
}

fn finish_message() {
    eprintln!("{}", t!("Готово.", "Done."));
    eprintln!("{}", t!(
        "Дальше: yandi admin pairing — код сопряжения телефона; yandi admin — меню управления; yandi admin backup status — копии связей.",
        "Next: yandi admin pairing — phone pairing code; yandi admin — the management menu; yandi admin backup status — backups of the links."
    ));
}

fn restore_flow(o: &Opts) -> Result<(), String> {
    let socket = &o.socket;
    let secrets = Secrets { from_stdin: o.password_stdin, preset: None };
    // 1. фраза и слово
    let typed = secrets.ask(t!("Сид-фраза (12, 18 или 24 слова): ", "Seed phrase (12, 18 or 24 words): "))?;
    let phrase = crate::admin_cli::normalize_phrase(&typed);
    let seed = key_root::Seed::parse(&phrase).map_err(seed_error_text)?;
    eprintln!("{}", t!(format!("Принято слов: {}.", seed.word_count()), format!("Words accepted: {}.", seed.word_count())));
    let word = ask_restore_control_word(o)?;
    // 2. номер узла до любой записи
    let id = node_number(&phrase, &word)?;
    print_id_block(&id);
    if !o.yes && tty() {
        eprintln!("{}", t!("Сверьте номер с записанным при настройке (он есть и в панели узла).", "Compare the number with the one noted at setup (it is also in the node panel)."));
        if !yn("Номер совпадает?", "Does the number match?", false)? {
            return Err(t!("номер не совпал: проверьте слова фразы и контрольное слово; ничего не записано", "the number does not match: check the phrase words and the control word; nothing was written").into());
        }
    }
    // 3. файл или папка копии связей (необязательно)
    let mut backup_path = o.backup_file.clone();
    if backup_path.is_none() && tty() && yn("Есть файл (.yandibak) или папка yandi-node с копиями связей?", "Do you have a backup file (.yandibak) or a yandi-node folder with backups of the links?", false)? {
        backup_path = Some(Secrets::ask_visible(t!("Полный путь к файлу или папке: ", "Full path to the file or folder: "))?.trim().to_string());
    }
    let min = {
        let (_c, seed_resp) = roundtrip(socket, "POST", "/api/auth/seed/new", Some(&json!({ "words": 24 })), None, Duration::from_secs(30))?;
        seed_resp["min_password"].as_u64().unwrap_or(8) as usize
    };
    // 4. новый пароль (старый не переносится)
    let (pw, admin) = ask_passwords(o, &secrets, min, backup_path.is_none())?;
    let master = joined(&phrase, &word);
    let mut from_folder: Option<String> = None;
    if let Some(path) = backup_path.filter(|p| !p.is_empty()) {
        let (code, v) = roundtrip(socket, "POST", "/api/auth/restore/inspect", Some(&json!({ "path": path })), None, Duration::from_secs(60))?;
        if code != 200 {
            return Err(error_text(code, &v));
        }
        eprintln!("{}", v["text"].as_str().unwrap_or(""));
        if v["node_id"].as_str().is_some_and(|n| n != id) {
            return Err(t!(
                "копия принадлежит другому узлу: проверьте слова фразы и контрольное слово; ничего не записано",
                "the backup belongs to another node: check the phrase words and the control word; nothing was written"
            ).into());
        }
        let (code, v) = roundtrip(socket, "POST", "/api/auth/restore", Some(&json!({ "path": path, "phrase": master, "password": pw })), None, Duration::from_secs(600))?;
        if code != 200 {
            return Err(error_text(code, &v));
        }
        for w in v["warnings"].as_array().into_iter().flatten().filter_map(|w| w.as_str()) {
            eprintln!("! {w}");
        }
        eprintln!("{}", t!("Узел восстановлен из копии: сопряжённые телефоны, контакты и группы на месте. Узел запускается…", "The node is restored from the backup: paired phones, contacts and groups are back. The node is starting…"));
        wait_started(socket)?;
        // папка …/yandi-node: копии логично продолжать писать туда же
        let p = std::path::Path::new(&path);
        if p.is_dir() && p.file_name().is_some_and(|n| n == crate::node_backup::FOLDER_NAME) {
            from_folder = p.parent().map(|x| x.display().to_string());
        }
    } else {
        let mut body = json!({
            "login_password": pw, "login_password_repeat": pw,
            "master_password": master, "master_password_repeat": master,
        });
        if !admin.is_empty() {
            body["admin_password"] = json!(admin);
            body["admin_password_repeat"] = json!(admin);
        }
        let (code, v) = roundtrip(socket, "POST", "/api/auth/setup", Some(&body), None, Duration::from_secs(300))?;
        if code != 200 {
            return Err(error_text(code, &v));
        }
        eprintln!("{}", t!("Ключи восстановлены. Узел запускается…", "Keys restored. The node is starting…"));
        wait_started(socket)?;
        let (code, v) = roundtrip(socket, "POST", "/api/vault/setup", Some(&json!({ "password": pw, "phrase": master })), None, Duration::from_secs(300))?;
        if code != 200 {
            return Err(t!(format!("ключи восстановлены, но закрытая среда не создана: {}", error_text(code, &v)), format!("the keys are restored, but the closed environment is not: {}", error_text(code, &v))).into());
        }
        eprintln!("{}", t!(
            "Личность узла, доверие к нему и ключ подписи списка узлов возвращены. Телефоны, контакты и группы без файла копии не возвращаются: телефоны сопрягите заново (yandi admin pairing).",
            "The node identity, the trust in it and the entry-list signing key are back. Without a backup file phones, contacts and groups do not come back: pair the phones again (yandi admin pairing)."
        ));
    }
    print_id_block(&id);
    backup_step(o, from_folder, if admin.is_empty() { &pw } else { &admin })?;
    finish_message();
    Ok(())
}

// ───────────────────────── место для копий связей ─────────────────────────

fn fmt_free(bytes: Option<u64>) -> String {
    match bytes {
        Some(b) if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        Some(b) => format!("{} MB", b >> 20),
        None => "?".into(),
    }
}

/// Один вызов «задать место копий»; `Ok(true)` — принято, `Ok(false)` — выбрать другое.
pub(crate) fn apply_place(s: &mut Session, dir: &str, auto_yes: bool) -> Result<bool, String> {
    let mut confirm = false;
    loop {
        let (code, v) = s.call("POST", "/api/backup/config", Some(&json!({ "dir": dir, "confirm_system": confirm })))?;
        if code == 200 {
            let shown = v["dir"].as_str().unwrap_or(dir);
            eprintln!("{}", t!(
                format!("Копии связей будут сохраняться в {shown}: при изменениях (контакты, группы, пользователи) в течение 10 минут; хранится {} последних, старые перезаписываются.", v["keep"]),
                format!("Backups of the links will be saved to {shown}: when things change (contacts, groups, users) within 10 minutes; the last {} are kept, older ones are overwritten.", v["keep"])
            ));
            return Ok(true);
        }
        match v["code"].as_str() {
            Some("system_disk") => {
                eprintln!("{}", t!(
                    "Это место на системном диске. Храните копии вне системного диска (флешка, другой диск, облако): при потере диска пропадут и копии.",
                    "This place is on the system disk. Keep backups off the system disk (a flash drive, another disk, the cloud): if the disk is lost the backups are lost too."
                ));
                if auto_yes || yn("Всё равно сохранять сюда?", "Save here anyway?", false)? {
                    confirm = true;
                    continue;
                }
                return Ok(false);
            }
            Some("no_write") => {
                let why = match v["reason"].as_str() {
                    Some("permission") => t!("доступ запрещён (диск или папка принадлежат другому пользователю)", "access denied (the disk or folder belongs to another user)").to_string(),
                    Some("read_only") => t!("диск только для чтения", "the disk is read-only").to_string(),
                    Some("not_found") => t!("такого места нет или это не папка", "no such place, or it is not a folder").to_string(),
                    Some("no_space") => t!("на диске нет свободного места", "no free space on the disk").to_string(),
                    _ => v["detail"].as_str().unwrap_or("").to_string(),
                };
                eprintln!("{}", t!(format!("У меня нет прав для записи в это место: {why}"), format!("I have no permission to write to this place: {why}")));
                return Ok(false);
            }
            Some("inside_node") => {
                eprintln!("{}", t!("Копия внутри каталогов самого узла не защищает от потери диска: выберите другое место.", "A backup inside the node's own folders does not protect against losing the disk: choose another place."));
                return Ok(false);
            }
            Some("not_absolute") => {
                eprintln!("{}", t!("Место задаётся полным путём без «..».", "The place must be a full path without \"..\"."));
                return Ok(false);
            }
            _ => return Err(error_text(code, &v)),
        }
    }
}

fn remind_no_backup() {
    eprintln!("{}", t!(
        "Копии связей не настроены (телефоны, контакты и группы без копии при потере диска пропадут). Настроить позже: yandi admin backup set-dir ПУТЬ",
        "Backups of the links are not set up (without a backup, phones, contacts and groups are lost with the disk). Set up later: yandi admin backup set-dir PATH"
    ));
}

/// Шаг мастера «где хранить копии»: после сида и пароля.
pub fn backup_step(o: &Opts, suggest: Option<String>, admin_password: &str) -> Result<(), String> {
    // копии — админский раздел узла: сеанс администратора открывается паролем, который человек только что задал
    let mut sess = Session::new(o.socket.clone(), false);
    sess.secrets.preset = Some(admin_password.to_string());
    if o.no_backup_dir {
        remind_no_backup();
        return Ok(());
    }
    if let Some(dir) = o.backup_dir.clone().or(suggest) {
        if apply_place(&mut sess, &dir, o.yes)? {
            return Ok(());
        }
        if o.backup_dir.is_some() {
            return Err(t!("место для копий не принято", "the backup place was not accepted").into());
        }
    }
    if !tty() {
        remind_no_backup();
        return Ok(());
    }
    eprintln!("\n{}", t!(
        "Где хранить копии связей (сопряжённые телефоны, контакты, группы)? Узел создаст в выбранном месте папку yandi-node, итоговый путь: …/yandi-node.\nХраните копии вне системного диска: флешка, другой диск, папка облака.",
        "Where to keep backups of the links (paired phones, contacts, groups)? The node creates a yandi-node folder in the chosen place, the final path: …/yandi-node.\nKeep backups off the system disk: a flash drive, another disk, a cloud folder."
    ));
    loop {
        let (code, v) = sess.call("GET", "/api/backup/places", None)?;
        let places: Vec<Value> = if code == 200 { v["places"].as_array().cloned().unwrap_or_default() } else { Vec::new() };
        for (i, p) in places.iter().enumerate() {
            let ok = if p["writable"] == true { "" } else { t!("  (нет прав на запись)", "  (not writable)") };
            eprintln!("  {}. {}  [{}, {}]{ok}", i + 1, p["path"].as_str().unwrap_or("?"), p["fs"].as_str().unwrap_or("?"), t!(format!("свободно {}", fmt_free(p["free_bytes"].as_u64())), format!("free {}", fmt_free(p["free_bytes"].as_u64()))));
        }
        eprintln!("{}", t!("  0. ввести путь вручную\n  Enter — пропустить (напомню позже)", "  0. type a path\n  Enter — skip (I will remind you later)"));
        let a = Secrets::ask_visible(t!("Выбор: ", "Choice: "))?;
        let a = a.trim();
        if a.is_empty() {
            remind_no_backup();
            return Ok(());
        }
        let dir = if a == "0" {
            Secrets::ask_visible(t!("Путь (например /media/iam/FLASH): ", "Path (for example /media/iam/FLASH): "))?.trim().to_string()
        } else if let Some(p) = a.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|n| places.get(n)) {
            p["path"].as_str().unwrap_or("").to_string()
        } else {
            eprintln!("{}", t!("Не понял выбор.", "I did not understand the choice."));
            continue;
        };
        if dir.is_empty() {
            continue;
        }
        if apply_place(&mut sess, &dir, false)? {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORDS: &str = "legal winner thank year wave sausage worth useful legal winner thank yellow";

    #[test]
    fn the_node_number_depends_on_the_words_and_the_control_word_only() {
        let a = node_number(WORDS, "").unwrap();
        assert_eq!(a.len(), 16);
        assert_eq!(a, node_number(&WORDS.to_uppercase(), "").unwrap());
        let b = node_number(WORDS, "Воробей-2026").unwrap();
        assert_ne!(a, b);
        assert_eq!(b, node_number(WORDS, "Воробей-2026").unwrap());
        assert!(node_number(WORDS, "коротко").is_err());
        assert!(node_number("winner legal thank year wave sausage worth useful legal winner thank yellow", "").is_err());
    }

    #[test]
    fn ids_are_shown_in_groups_of_four_and_seed_errors_speak_both_languages() {
        assert_eq!(show_id("e96d01bcf6d85540"), "e96d 01bc f6d8 5540");
        crate::i18n::set(crate::i18n::Lang::En);
        assert!(seed_error_text(key_root::SeedError::WrongCount(5)).contains("12, 18 or 24 words"));
        assert!(seed_error_text(key_root::SeedError::ControlWordShort).contains("at least 8"));
        crate::i18n::set(crate::i18n::Lang::Ru);
        assert!(seed_error_text(key_root::SeedError::WrongCount(5)).contains("Нужно 12, 18 или 24"));
        assert!(seed_error_text(key_root::SeedError::UnknownWord(3)).contains("№3"));
    }

    #[test]
    fn weak_words_are_flagged_and_random_ones_are_not() {
        for w in ["password", "12345678", "aaaaaaaa", "Пароль123", "ababababab"] {
            assert!(weak_control_word(w), "{w}");
        }
        for w in ["Kx9-Pq2vLm", "Воробей-2026", "tR7#nW0zQe"] {
            assert!(!weak_control_word(w), "{w}");
        }
    }
}
