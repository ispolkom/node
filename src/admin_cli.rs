//! `yandi setup` и `yandi admin` — управление запущенным узлом из консоли (сервер без оболочки).
//!
//! Это тонкий клиент того же локального сокета `yandi-ui.sock`, которым пользуется оболочка: никакой своей логики первой настройки
//! или админских действий здесь нет, вызываются те же обработчики узла. Доступ к сокету получает только владелец каталога данных
//! (проверка uid у сокета), админские маршруты по-прежнему требуют сеанс администратора (пароль, 15 минут, токен только в памяти
//! процесса).
//!
//!   yandi setup [--words 12|24] [--seed-file ФАЙЛ] [--password-stdin]   первая настройка (пароль, фраза восстановления, закрытая среда)
//!   yandi admin                                                           меню
//!   yandi admin status | pairing [owner|user] [--host H] | devices | device-remove ID | phone-action ПИР ДЕЙСТВИЕ
//!                proxies [list|add|remove ID|test|settings] | vault status|lock|unlock|change-password|panic-erase
//!                node stop|start | alerts | update | raw МЕТОД ПУТЬ [json]
//!
//! Общие параметры: `--socket ПУТЬ`, `--admin-password-stdin` (пароль администратора первой строкой стандартного ввода).
//! Секреты вводятся без эха и нигде не печатаются; фраза восстановления показывается только в терминале (иначе нужен `--seed-file`).
use crate::t;
use serde_json::{json, Value};
use std::io::{BufRead, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_RESPONSE: usize = 8 * 1024 * 1024;

/// Каталог по умолчанию — тот же, что у узла и оболочки.
pub fn default_socket() -> PathBuf {
    crate::util::data_dir::data_dir().join("run").join("yandi-ui.sock")
}

// ───────────────────────── HTTP по Unix-сокету ─────────────────────────

/// Разобрать ответ HTTP/1.1 строго: тело обязано соответствовать `Content-Length` (только цифры) либо единственной кодировке `chunked`
/// с завершающим нулевым куском и разобранными заголовками-трейлерами. Неоднозначное обрамление (оба заголовка, иные кодировки,
/// мусор после конца) — ошибка, а не «пустой успех».
pub fn parse_response(raw: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or(t!("ответ узла оборван", "node reply is truncated"))?;
    let head = std::str::from_utf8(&raw[..split]).map_err(|_| t!("ответ узла не текст", "node reply is not text"))?;
    let mut lines = head.split("\r\n");
    let status = lines.next().ok_or(t!("пустой ответ", "empty reply"))?;
    let code: u16 = status.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or(t!("нет кода ответа", "no reply status code"))?;
    let mut encodings: Option<Vec<String>> = None;
    let mut length: Option<usize> = None;
    for l in lines {
        let lower = l.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("transfer-encoding:") {
            if encodings.replace(v.split(',').map(|t| t.trim().to_string()).collect()).is_some() {
                return Err(t!("повторный заголовок кодировки", "duplicate encoding header").into());
            }
        } else if let Some(v) = lower.strip_prefix("content-length:") {
            let v = v.trim();
            if v.is_empty() || v.len() > 15 || !v.bytes().all(|b| b.is_ascii_digit()) {
                return Err(t!("неверная длина тела", "invalid body length").into());
            }
            let n: usize = v.parse().map_err(|_| t!("неверная длина тела", "invalid body length"))?;
            if length.replace(n).is_some_and(|prev| prev != n) {
                return Err(t!("противоречивая длина тела", "conflicting body length").into());
            }
        }
    }
    let body = &raw[split + 4..];
    let chunked = match encodings.as_deref() {
        None => false,
        Some([only]) if only == "chunked" => true,
        Some(_) => return Err(t!("неподдерживаемая кодировка ответа", "unsupported reply encoding").into()),
    };
    if chunked && length.is_some() {
        return Err(t!("неоднозначное обрамление: и кодировка, и длина", "ambiguous framing: both encoding and length").into());
    }
    if !chunked {
        return match length {
            Some(n) if body.len() == n => Ok((code, body.to_vec())),
            Some(_) => Err(t!("тело ответа не совпадает с заявленной длиной (ответ оборван)", "reply body does not match the declared length (truncated)").into()),
            None => Ok((code, body.to_vec())),
        };
    }
    let mut out = Vec::new();
    let mut rest = body;
    loop {
        let eol = rest.windows(2).position(|w| w == b"\r\n").ok_or(t!("оборван ответ по частям", "chunked reply is truncated"))?;
        let size_text = std::str::from_utf8(&rest[..eol]).map_err(|_| t!("размер части не текст", "chunk size is not text"))?;
        let size_hex = size_text.split(';').next().unwrap_or("").trim();
        if size_hex.is_empty() || size_hex.len() > 15 || !size_hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(t!("неверный размер части", "invalid chunk size").into());
        }
        let size = usize::from_str_radix(size_hex, 16).map_err(|_| t!("неверный размер части", "invalid chunk size"))?;
        rest = &rest[eol + 2..];
        if size == 0 {
            // трейлеры: строки «имя: значение» до пустой строки; после неё ничего не должно быть
            loop {
                let eol = rest.windows(2).position(|w| w == b"\r\n").ok_or(t!("ответ по частям не завершён", "chunked reply is not finished"))?;
                let line = &rest[..eol];
                rest = &rest[eol + 2..];
                if line.is_empty() {
                    return if rest.is_empty() { Ok((code, out)) } else { Err(t!("лишние данные после конца ответа", "extra data after the end of the reply").into()) };
                }
                if !line.contains(&b':') {
                    return Err(t!("неверный заголовок в конце ответа", "invalid trailer at the end of the reply").into());
                }
            }
        }
        let end = size.checked_add(2).filter(|e| *e <= rest.len()).ok_or(t!("оборван ответ по частям", "chunked reply is truncated"))?;
        if &rest[size..end] != b"\r\n" {
            return Err(t!("часть ответа не завершена CRLF", "reply chunk is not terminated by CRLF").into());
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[end..];
    }
}

/// Подключиться к сокету. Сначала напрямую; путь, не помещающийся в `sun_path` (около 108 знаков), открывается через дескриптор каталога
/// (`/proc/self/fd/N/имя`), если такой псевдоним короче.
pub fn connect(socket: &Path) -> std::io::Result<UnixStream> {
    match UnixStream::connect(socket) {
        Ok(s) => return Ok(s),
        Err(e) if socket.as_os_str().len() < 100 => return Err(e),
        Err(_) => {}
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let (Some(dir), Some(name)) = (socket.parent(), socket.file_name()) else {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
        };
        let d = std::fs::File::open(dir)?;
        let mut alias = format!("/proc/self/fd/{}/", d.as_raw_fd()).into_bytes();
        alias.extend_from_slice(name.as_bytes());
        return UnixStream::connect(std::path::PathBuf::from(std::ffi::OsString::from_vec(alias)));
    }
    #[cfg(not(target_os = "linux"))]
    UnixStream::connect(socket)
}

/// Один запрос к узлу. Тело и ответ — JSON.
pub fn roundtrip(socket: &Path, method: &str, path: &str, body: Option<&Value>, admin_token: Option<&str>, timeout: Duration) -> Result<(u16, Value), String> {
    let mut s = connect(socket).map_err(|_| t!("узел не отвечает: он запущен под вашим пользователем? (сокет yandi-ui.sock)", "the node does not answer: is it running under your user? (socket yandi-ui.sock)").to_string())?;
    s.set_read_timeout(Some(timeout)).ok();
    s.set_write_timeout(Some(timeout)).ok();
    let payload = body.map(|b| b.to_string());
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nAccept: application/json\r\nConnection: close\r\n");
    if let Some(t) = admin_token {
        req.push_str(&format!("x-yandi-admin: {t}\r\n"));
    }
    if let Some(p) = &payload {
        req.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", p.len()));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).map_err(|_| t!("не удалось отправить запрос узлу", "could not send the request to the node").to_string())?;
    if let Some(p) = &payload {
        s.write_all(p.as_bytes()).map_err(|_| t!("не удалось отправить запрос узлу", "could not send the request to the node").to_string())?;
    }
    let mut raw = Vec::new();
    s.take(MAX_RESPONSE as u64 + 1).read_to_end(&mut raw).map_err(|_| t!("узел не ответил вовремя", "the node did not answer in time").to_string())?;
    if raw.len() > MAX_RESPONSE {
        return Err(t!("ответ узла слишком велик", "the node reply is too large").into());
    }
    let (code, bytes) = parse_response(&raw)?;
    let value = if bytes.is_empty() {
        // пустое тело успешного ответа допустимо только у 204; иначе узел нарушил протокол (ответ — всегда JSON)
        if (200..300).contains(&code) && code != 204 {
            return Err(t!("ответ узла пуст: ожидался JSON", "empty node reply: JSON expected").into());
        }
        Value::Null
    } else {
        match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            // успешный ответ обязан быть JSON; текст ошибки без JSON допустим (его покажет error_text по коду)
            Err(_) if (200..300).contains(&code) => return Err(t!("ответ узла не соответствует протоколу (не JSON)", "the node reply violates the protocol (not JSON)").into()),
            Err(_) => Value::Null,
        }
    };
    Ok((code, value))
}

// ───────────────────────── ввод секретов ─────────────────────────

/// Откуда брать секреты: терминал без эха или строки стандартного ввода (для скриптов).
pub struct Secrets {
    pub from_stdin: bool,
    /// Готовый секрет (только для проверок).
    pub preset: Option<String>,
}

impl Secrets {
    fn line() -> Result<String, String> {
        let mut line = String::new();
        let n = std::io::stdin().lock().read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err(t!("конец ввода: ожидалась ещё строка", "end of input: another line was expected").into());
        }
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }

    pub fn ask(&self, prompt: &str) -> Result<String, String> {
        if let Some(p) = &self.preset {
            return Ok(p.clone());
        }
        if self.from_stdin {
            return Self::line();
        }
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        unsafe {
            if libc::isatty(0) != 1 {
                return Err(t!("стандартный ввод — не терминал: запустите команду в терминале или передайте секрет через --…-stdin", "standard input is not a terminal: run the command in a terminal or pass the secret with --…-stdin").into());
            }
            let mut old: libc::termios = std::mem::zeroed();
            libc::tcgetattr(0, &mut old);
            let mut quiet = old;
            quiet.c_lflag &= !libc::ECHO;
            libc::tcsetattr(0, libc::TCSANOW, &quiet);
            let line = Self::line();
            libc::tcsetattr(0, libc::TCSANOW, &old);
            eprintln!();
            line
        }
    }

    /// Обычный (видимый) вопрос: подтверждение «да», номер пункта меню.
    pub fn ask_visible(prompt: &str) -> Result<String, String> {
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        Self::line()
    }
}

fn stdout_is_tty() -> bool {
    unsafe { libc::isatty(1) == 1 }
}

// ───────────────────────── сеанс с узлом ─────────────────────────

pub struct Session {
    pub socket: PathBuf,
    pub secrets: Secrets,
    pub token: Option<String>,
    pub timeout: Duration,
}

impl Session {
    pub fn new(socket: PathBuf, admin_stdin: bool) -> Self {
        Self { socket, secrets: Secrets { from_stdin: admin_stdin, preset: None }, token: None, timeout: Duration::from_secs(30) }
    }

    fn raw(&self, method: &str, path: &str, body: Option<&Value>) -> Result<(u16, Value), String> {
        roundtrip(&self.socket, method, path, body, self.token.as_deref(), self.timeout)
    }

    /// Запрос; на `admin_required` один раз запрашивает пароль администратора и повторяет.
    pub fn call(&mut self, method: &str, path: &str, body: Option<&Value>) -> Result<(u16, Value), String> {
        let (code, value) = self.raw(method, path, body)?;
        if code == 403 && value["error"] == "admin_required" {
            self.login_admin()?;
            return self.raw(method, path, body);
        }
        Ok((code, value))
    }

    /// Вызов, где любой не-2xx — ошибка с текстом узла.
    pub fn ok(&mut self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let (code, value) = self.call(method, path, body)?;
        if (200..300).contains(&code) {
            Ok(value)
        } else {
            Err(error_text(code, &value))
        }
    }

    /// Сеанс администратора заранее: опасные команды сначала спрашивают пароль, и только потом слово подтверждения.
    pub fn ensure_admin(&mut self) -> Result<(), String> {
        if self.token.is_some() {
            return Ok(());
        }
        self.login_admin()
    }

    fn login_admin(&mut self) -> Result<(), String> {
        let pw = self.secrets.ask(t!("Пароль администратора: ", "Administrator password: "))?;
        let (code, v) = self.raw("POST", "/api/admin/session", Some(&json!({ "password": pw })))?;
        if code == 200 {
            self.token = v["token"].as_str().map(str::to_string);
            if self.token.is_some() {
                return Ok(());
            }
        }
        Err(error_text(code, &v))
    }
}

/// Текст ошибки узла для человека (без тела ответа целиком).
pub fn error_text(code: u16, v: &Value) -> String {
    let msg = v["error"].as_str().or_else(|| v["message"].as_str()).unwrap_or("");
    match (code, msg) {
        (401, _) => t!("неверный пароль или доступ запрещён", "wrong password or access denied").to_string(),
        (429, _) => t!("слишком много попыток: подождите и повторите", "too many attempts: wait and try again").to_string(),
        (_, m) if !m.is_empty() => t!(format!("узел ответил {code}: {}", m.chars().take(300).collect::<String>()), format!("the node answered {code}: {}", m.chars().take(300).collect::<String>())),
        _ => t!(format!("узел ответил {code}"), format!("the node answered {code}")),
    }
}

// ───────────────────────── вывод ─────────────────────────

fn qr_to_terminal(text: &str) -> Option<String> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    Some(code.render::<qrcode::render::unicode::Dense1x2>().quiet_zone(true).build())
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

fn field(v: &Value, k: &str) -> String {
    match &v[k] {
        Value::String(s) => s.clone(),
        Value::Null => "—".into(),
        other => other.to_string(),
    }
}

// ───────────────────────── команды admin ─────────────────────────

fn print_pairing(v: &Value) {
    println!("{}", t!(format!("Код сопряжения телефона (действует {} с):", v["expires_in_secs"].as_u64().unwrap_or(300)), format!("Phone pairing code (valid for {} s):", v["expires_in_secs"].as_u64().unwrap_or(300))));
    println!("{}{}:{}", t!("  адрес узла : ", "  node address: "), field(v, "host"), field(v, "port"));
    println!("{}{}", t!("  отпечаток  : ", "  fingerprint : "), field(v, "fingerprint"));
    if let Some(qr) = v["qr_text"].as_str() {
        if stdout_is_tty() {
            if let Some(art) = qr_to_terminal(qr) {
                println!("{art}");
            }
        }
        println!("{}\n{qr}", t!("Текст кода (если камера не считывает QR):", "Code text (if the camera cannot read the QR):"));
    }
}

fn confirm_word(prompt: &str, word: &str) -> Result<bool, String> {
    let typed = Secrets::ask_visible(&format!("{prompt}\n{}", t!(format!("Для подтверждения введите «{word}»: "), format!("To confirm, type “{word}”: "))))?;
    // регистр не важен; слово строго на выбранном языке (кириллица в английском режиме и наоборот не засчитывается)
    Ok(typed.trim().to_lowercase() == word.to_lowercase())
}

/// Ключ подписи списка входных узлов хранится только в закрытой среде узла (файла нет): здесь он лишь просится подписать список.
fn cmd_bootstrap(s: &mut Session, args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str).unwrap_or("pubkey") {
        "pubkey" => {
            let v = s.ok("GET", "/api/admin/bootstrap/pubkey", None)?;
            match v["pubkey"].as_str() {
                Some(k) => {
                    println!("{k}");
                    eprintln!("{}", if v["pinned"] == true { t!("Этот ключ закреплён в программе (bootstrap/signer.pub).", "This key is pinned in the program (bootstrap/signer.pub).") } else { t!("Ключ ещё не закреплён: запишите его в bootstrap/signer.pub и пересоберите узел и приложение.", "The key is not pinned yet: write it to bootstrap/signer.pub and rebuild the node and the app.") });
                }
                None => return Err(t!("закрытая среда заперта: откройте её паролем (yandi admin vault unlock)", "the closed environment is locked: unlock it with the password (yandi admin vault unlock)").into()),
            }
        }
        "sign" => {
            let file = args.get(1).ok_or(t!("bootstrap sign ФАЙЛ [--days N] [--network ИМЯ]", "bootstrap sign FILE [--days N] [--network NAME]"))?;
            let mut days: Option<u64> = None;
            let mut network: Option<String> = None;
            let mut it = args[2..].iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--days" => days = Some(it.next().and_then(|d| d.parse().ok()).ok_or(t!("--days N", "--days N"))?),
                    "--network" => network = Some(it.next().ok_or(t!("--network ИМЯ", "--network NAME"))?.clone()),
                    other => return Err(t!(format!("неизвестный параметр {other}"), format!("unknown option {other}"))),
                }
            }
            let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
            let list: Value = serde_json::from_str(&text).map_err(|_| t!("файл списка — не JSON", "the list file is not JSON").to_string())?;
            s.ensure_admin()?;
            let r = s.ok("POST", "/api/admin/bootstrap/sign", Some(&json!({ "list": list, "days": days, "network": network })))?;
            let signed = serde_json::to_string_pretty(&r["list"]).map_err(|e| e.to_string())? + "\n";
            // атомарно: сначала рядом, потом переименование — оборванная запись не испортит прежний список
            let tmp = format!("{file}.new");
            std::fs::write(&tmp, signed).map_err(|e| format!("{tmp}: {e}"))?;
            std::fs::rename(&tmp, file).map_err(|e| format!("{file}: {e}"))?;
            println!("{}", t!(format!("{file}: подписан, выпуск {}, ключ {}.", field(&r["list"], "sequence"), field(&r["list"], "signer")), format!("{file}: signed, release {}, key {}.", field(&r["list"], "sequence"), field(&r["list"], "signer"))));
            if r["pinned"] != true {
                eprintln!("{}", t!("ВНИМАНИЕ: этот ключ не закреплён в программе (bootstrap/signer.pub): узлы и приложения такой список не примут, пока ключ не вшит.", "WARNING: this key is not pinned in the program (bootstrap/signer.pub): nodes and apps will not accept such a list until the key is built in."));
            }
        }
        other => return Err(t!(format!("bootstrap: неизвестное действие «{other}» (pubkey | sign)"), format!("bootstrap: unknown action “{other}” (pubkey | sign)"))),
    }
    Ok(())
}

fn cmd_proxies(s: &mut Session, args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str).unwrap_or("list") {
        "list" => {
            let v = s.ok("GET", "/api/upstream-proxies", None)?;
            println!("{}", t!(format!("Пул: {}; строго: {}; клиентов на прокси: {}", field(&v, "enabled"), field(&v, "strict"), field(&v, "max_clients")), format!("Pool: {}; strict: {}; clients per proxy: {}", field(&v, "enabled"), field(&v, "strict"), field(&v, "max_clients"))));
            for p in v["proxies"].as_array().into_iter().flatten() {
                println!("  {}  {}  {}:{}", field(p, "id"), field(p, "kind"), field(p, "host"), field(p, "port"));
            }
        }
        "add" => {
            let kind = args.get(1).map(String::as_str).unwrap_or("socks5");
            if !matches!(kind, "socks5" | "http_connect") {
                return Err(t!("вид прокси: socks5 или http_connect", "proxy kind: socks5 or http_connect").into());
            }
            s.ensure_admin()?; // пароль администратора — первой строкой стандартного ввода, до списка прокси
            eprintln!("{}", t!("Введите прокси по одному в строке (host:port или host:port:пользователь:пароль), пустая строка — конец:", "Enter proxies one per line (host:port or host:port:user:password), an empty line ends the list:"));
            let mut text = String::new();
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let line = line.map_err(|e| e.to_string())?;
                if line.trim().is_empty() {
                    break;
                }
                text.push_str(&line);
                text.push('\n');
            }
            let r = s.ok("POST", "/api/upstream-proxies", Some(&json!({ "text": text, "kind": kind })))?;
            for e in r["errors"].as_array().into_iter().flatten() {
                eprintln!("{}{}", t!("  не принято: ", "  rejected: "), e.as_str().unwrap_or("?"));
            }
            println!("{}", t!("Готово.", "Done."));
        }
        "remove" => {
            let id = args.get(1).ok_or(t!("нужен идентификатор прокси (yandi admin proxies list)", "proxy id needed (yandi admin proxies list)"))?;
            if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                return Err(t!("неверный идентификатор", "invalid id").into());
            }
            s.ok("DELETE", &format!("/api/upstream-proxies/{id}"), None)?;
            println!("{}", t!("Удалено.", "Removed."));
        }
        "test" => {
            let kind = args.get(1).map(String::as_str).unwrap_or("socks5");
            s.ensure_admin()?;
            let mut text = String::new();
            eprintln!("{}", t!("Проверить прокси (строки как при добавлении), пустая строка — конец:", "Proxies to test (lines as when adding), an empty line ends the list:"));
            for line in std::io::stdin().lock().lines() {
                let line = line.map_err(|e| e.to_string())?;
                if line.trim().is_empty() {
                    break;
                }
                text.push_str(&line);
                text.push('\n');
            }
            let r = s.ok("POST", "/api/upstream-proxies/test", Some(&json!({ "text": text, "kind": kind })))?;
            for x in r["results"].as_array().into_iter().flatten() {
                println!("  {}", if x["ok"] == true { t!("работает", "works") } else { t!("не отвечает", "does not answer") });
            }
        }
        "settings" => {
            let mut body = serde_json::Map::new();
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--enabled" | "--strict" => {
                        let on = match it.next().map(String::as_str) {
                            Some("on") => true,
                            Some("off") => false,
                            _ => return Err(t!(format!("{a}: нужно on или off"), format!("{a}: on or off"))),
                        };
                        body.insert(a.trim_start_matches("--").into(), json!(on));
                    }
                    "--max-clients" => {
                        let n: u32 = it.next().and_then(|n| n.parse().ok()).ok_or("--max-clients N")?;
                        body.insert("max_clients".into(), json!(n));
                    }
                    other => return Err(t!(format!("неизвестный параметр {other}"), format!("unknown option {other}"))),
                }
            }
            if body.is_empty() {
                return Err(t!("укажите --enabled on|off, --strict on|off или --max-clients N", "give --enabled on|off, --strict on|off or --max-clients N").into());
            }
            s.ok("POST", "/api/upstream-proxies/settings", Some(&Value::Object(body)))?;
            println!("{}", t!("Сохранено.", "Saved."));
        }
        other => return Err(t!(format!("proxies: неизвестное действие «{other}» (list | add | remove | test | settings)"), format!("proxies: unknown action “{other}” (list | add | remove | test | settings)"))),
    }
    Ok(())
}

fn role_label(role: &str) -> &str {
    match role {
        "owner" => t!("владелец", "owner"),
        "user" => t!("пользователь", "user"),
        "invited" => t!("приглашённый", "invited"),
        "guest" => t!("гость", "guest"),
        other => other,
    }
}

fn check_device_id(id: &str) -> Result<(), String> {
    if id.len() < 8 || id.len() > 64 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(t!("номер устройства — от 8 до 64 шестнадцатеричных знаков (yandi admin devices)", "device id: 8 to 64 hexadecimal characters (yandi admin devices)").into());
    }
    Ok(())
}

/// Политика приглашений: показать или изменить (включено, в сутки, живых кодов).
fn cmd_invites(s: &mut Session, args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str).unwrap_or("show") {
        "show" => {
            let p = s.ok("GET", "/api/mobile/invite-policy", None)?;
            println!("{}", t!(format!("Приглашения устройств: {}; в сутки от одного человека: {}; действующих кодов: {}", if p["enabled"] == true { "включены" } else { "выключены" }, field(&p, "per_day"), field(&p, "max_live")), format!("Device invitations: {}; per person per day: {}; live codes: {}", if p["enabled"] == true { "on" } else { "off" }, field(&p, "per_day"), field(&p, "max_live"))));
        }
        "set" => {
            // разбор до любого запроса: пустая или неверная команда ничего не трогает
            let mut changes: Vec<(&str, Value)> = Vec::new();
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--enabled" => changes.push((
                        "enabled",
                        json!(match it.next().map(String::as_str) {
                            Some("on") => true,
                            Some("off") => false,
                            _ => return Err(t!("--enabled: нужно on или off", "--enabled: on or off").into()),
                        }),
                    )),
                    "--per-day" | "--max-live" => {
                        let n: u32 = it.next().and_then(|n| n.parse().ok()).ok_or_else(|| format!("{a} N"))?;
                        changes.push((if a == "--per-day" { "per_day" } else { "max_live" }, json!(n)));
                    }
                    other => return Err(t!(format!("неизвестный параметр {other}"), format!("unknown option {other}"))),
                }
            }
            if changes.is_empty() {
                return Err(t!("укажите --enabled on|off, --per-day N или --max-live N", "give --enabled on|off, --per-day N or --max-live N").into());
            }
            let mut p = s.ok("GET", "/api/mobile/invite-policy", None)?;
            for (k, v) in changes {
                p[k] = v;
            }
            s.ok("POST", "/api/mobile/invite-policy", Some(&p))?;
            println!("{}", t!("Сохранено.", "Saved."));
        }
        other => return Err(t!(format!("invites: неизвестное действие «{other}» (show | set)"), format!("invites: unknown action “{other}” (show | set)"))),
    }
    Ok(())
}

fn cmd_vault(s: &mut Session, args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str).unwrap_or("status") {
        "status" => println!("{}", pretty(&s.ok("GET", "/api/vault/status", None)?)),
        "lock" => {
            s.ok("POST", "/api/vault/lock", Some(&json!({})))?;
            println!("{}", t!("Закрытая среда заперта.", "The closed environment is locked."));
        }
        "unlock" => {
            let pw = s.secrets.ask(t!("Пароль владельца: ", "Owner password: "))?;
            s.ok("POST", "/api/vault/unlock", Some(&json!({ "password": pw })))?;
            println!("{}", t!("Закрытая среда открыта.", "The closed environment is unlocked."));
        }
        "change-password" => {
            let old = s.secrets.ask(t!("Текущий пароль: ", "Current password: "))?;
            let new = s.secrets.ask(t!("Новый пароль: ", "New password: "))?;
            let again = s.secrets.ask(t!("Новый пароль ещё раз: ", "New password again: "))?;
            if new != again {
                return Err(t!("пароли не совпадают", "passwords do not match").into());
            }
            s.ok("POST", "/api/vault/change_password", Some(&json!({ "old_password": old, "new_password": new })))?;
            println!("{}", t!("Пароль изменён.", "Password changed."));
        }
        "panic-erase" => {
            s.ensure_admin()?;
            eprintln!("{}", t!("ВНИМАНИЕ: закрытая среда этого узла будет стёрта безвозвратно. Личность восстанавливается по сид-фразе и файлу копии.", "WARNING: the closed environment of this node will be erased irreversibly. The identity is restored from the seed phrase and the backup file."));
            if !confirm_word(t!("Стереть закрытую среду?", "Erase the closed environment?"), t!("СТЕРЕТЬ", "ERASE"))? {
                return Err(t!("не подтверждено: ничего не стёрто", "not confirmed: nothing was erased").into());
            }
            s.ok("POST", "/api/vault/panic_erase", Some(&json!({ "confirm": "СТЕРЕТЬ" })))?;
            println!("{}", t!("Закрытая среда стёрта.", "The closed environment has been erased."));
        }
        other => return Err(t!(format!("vault: неизвестное действие «{other}» (status | lock | unlock | change-password | panic-erase)"), format!("vault: unknown action “{other}” (status | lock | unlock | change-password | panic-erase)"))),
    }
    Ok(())
}

/// Выполнить одну команду admin. `Ok(())` — успех; текст ошибки печатает вызывающий.
pub fn dispatch(s: &mut Session, args: &[String]) -> Result<(), String> {
    let cmd = args.first().map(String::as_str).ok_or(t!("нужна команда", "a command is needed"))?;
    let rest = &args[1..];
    match cmd {
        "status" => println!("{}", pretty(&s.ok("GET", "/api/status", None)?)),
        "pairing" => {
            let mut role = "owner".to_string();
            let mut host: Option<String> = None;
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "owner" | "user" => role = a.clone(),
                    "--host" => host = Some(it.next().ok_or(t!("--host АДРЕС", "--host ADDRESS"))?.clone()),
                    other => return Err(t!(format!("неизвестный параметр {other}"), format!("unknown option {other}"))),
                }
            }
            let mut path = format!("/api/mobile/pairing?role={role}");
            if let Some(h) = host {
                if h.is_empty() || !h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']')) {
                    return Err(t!("адрес: допустимы буквы, цифры, точка, дефис, двоеточие", "address: letters, digits, dot, hyphen and colon only").into());
                }
                path.push_str(&format!("&host={h}"));
            }
            print_pairing(&s.ok("GET", &path, None)?);
        }
        "devices" => {
            let v = s.ok("GET", "/api/mobile/devices", None)?;
            if v["devices"].as_array().map_or(true, |a| a.is_empty()) {
                println!("{}", t!("Сопряжённых устройств нет. Код сопряжения: yandi admin pairing", "No paired devices. Pairing code: yandi admin pairing"));
            }
            for d in v["devices"].as_array().into_iter().flatten() {
                // ник, который человек задал себе сам, главнее технического имени устройства
                let nick = d["nick"].as_str().map(str::trim).filter(|n| !n.is_empty());
                let who = match nick {
                    Some(n) => format!("{n} [{}]", field(d, "name")),
                    None => field(d, "name"),
                };
                let inviter = d["invited_by_nick"].as_str().map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).or_else(|| d["invited_by_name"].as_str().map(str::to_string));
                let by = match (&inviter, d["invited_by"].is_string()) {
                    (Some(n), _) => t!(format!("  (пригласил: {n})"), format!("  (invited by: {n})")),
                    (None, true) => t!("  (пригласивший удалён)", "  (the inviter was removed)").to_string(),
                    _ => String::new(),
                };
                let wait = if d["role"] == "invited" { t!(", ждёт подтверждения", ", awaiting confirmation") } else { "" };
                println!("  {}  {}  {}{wait}{by}", field(d, "peer_id"), who, role_label(&field(d, "role")));
            }
        }
        "device-remove" => {
            let id = rest.first().ok_or(t!("нужен peer_id устройства (yandi admin devices)", "device peer_id needed (yandi admin devices)"))?;
            if id.len() != 64 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(t!("peer_id — 64 шестнадцатеричных знака", "peer_id is 64 hexadecimal characters").into());
            }
            s.ensure_admin()?;
            if !confirm_word(t!("Отвязать устройство?", "Unpair the device?"), crate::i18n::confirm_word())? {
                return Err(t!("не подтверждено", "not confirmed").into());
            }
            let path = format!("/api/mobile/devices/{id}");
            let (code, v) = s.call("DELETE", &path, None)?;
            if code == 403 && v["error"] != "admin_required" {
                // устройство владеет группами: их удаление требует пароль узла ещё раз
                let pw = s.secrets.ask(t!("Пароль узла (устройство владеет группами): ", "Node password (the device owns groups): "))?;
                let (code, v) = s.call("DELETE", &path, Some(&json!({ "password": pw })))?;
                if !(200..300).contains(&code) {
                    return Err(error_text(code, &v));
                }
            } else if !(200..300).contains(&code) {
                return Err(error_text(code, &v));
            }
            println!("{}", t!("Устройство отвязано.", "Device unpaired."));
        }
        "device-promote" => {
            let id = rest.first().ok_or("device-promote ID (yandi admin devices)")?;
            check_device_id(id)?;
            s.ensure_admin()?;
            s.ok("POST", &format!("/api/mobile/devices/{id}/promote"), Some(&json!({})))?;
            println!("{}", t!("Устройство повышено до пользователя: теперь оно может приглашать само.", "Device confirmed as a user: it can now invite others itself."));
        }
        "device-revoke-branch" => {
            let id = rest.first().ok_or("device-revoke-branch ID (yandi admin devices)")?;
            check_device_id(id)?;
            s.ensure_admin()?;
            eprintln!("{}", t!("Будет отозвано это устройство и ВСЕ, кого оно пригласило (и кого пригласили они).", "This device and EVERYONE it invited (and whom they invited) will be revoked."));
            if !confirm_word(t!("Отозвать ветку?", "Revoke the branch?"), crate::i18n::confirm_word())? {
                return Err(t!("не подтверждено", "not confirmed").into());
            }
            let path = format!("/api/mobile/devices/{id}/revoke-branch");
            let (code, v) = s.call("POST", &path, Some(&json!({})))?;
            let r = if code == 403 && v["error"] == "password_required" {
                // в ветке есть владельцы групп: их удаление требует пароль узла ещё раз
                let pw = s.secrets.ask(t!("Пароль узла (в ветке есть владельцы групп): ", "Node password (the branch has group owners): "))?;
                let (code, v) = s.call("POST", &path, Some(&json!({ "password": pw })))?;
                if !(200..300).contains(&code) {
                    return Err(error_text(code, &v));
                }
                v
            } else if !(200..300).contains(&code) {
                return Err(error_text(code, &v));
            } else {
                v
            };
            println!("{}", t!(format!("Отозвано устройств: {}.", field(&r, "removed")), format!("Devices revoked: {}.", field(&r, "removed"))));
        }
        "invites" => cmd_invites(s, rest)?,
        "bootstrap" => cmd_bootstrap(s, rest)?,
        "phone-action" => {
            let peer = rest.first().ok_or(t!("phone-action ПИР ДЕЙСТВИЕ", "phone-action PEER ACTION"))?;
            let action = rest.get(1).ok_or(t!("phone-action ПИР ДЕЙСТВИЕ", "phone-action PEER ACTION"))?;
            s.ok("POST", "/api/mobile/phone-action", Some(&json!({ "peer": peer, "action": action })))?;
            println!("{}", t!("Выполнено.", "Done."));
        }
        "proxies" => cmd_proxies(s, rest)?,
        "vault" => cmd_vault(s, rest)?,
        "node" => match rest.first().map(String::as_str) {
            Some(a @ ("stop" | "start")) => {
                s.ok("POST", &format!("/api/node/{a}"), Some(&json!({})))?;
                println!("{}", t!("Готово.", "Done."));
            }
            _ => return Err("node stop | node start".into()),
        },
        "alerts" => println!("{}", pretty(&s.ok("GET", "/api/alerts", None)?)),
        "update" => println!("{}", pretty(&s.ok("GET", "/api/update/status", None)?)),
        "raw" => {
            let method = rest.first().ok_or(t!("raw МЕТОД ПУТЬ [json]", "raw METHOD PATH [json]"))?.to_ascii_uppercase();
            let path = rest.get(1).ok_or(t!("raw МЕТОД ПУТЬ [json]", "raw METHOD PATH [json]"))?;
            if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE") || !path.starts_with("/api/") || path.contains(['\r', '\n', ' ']) {
                return Err(t!("raw: метод GET/POST/PUT/PATCH/DELETE и путь вида /api/…", "raw: method GET/POST/PUT/PATCH/DELETE and a path like /api/…").into());
            }
            let body: Option<Value> = match rest.get(2) {
                Some(j) => Some(serde_json::from_str(j).map_err(|_| t!("тело — не JSON", "the body is not JSON").to_string())?),
                None => None,
            };
            let (code, v) = s.call(&method, path, body.as_ref())?;
            println!("{code}\n{}", pretty(&v));
        }
        other => return Err(t!(format!("неизвестная команда «{other}»; yandi admin --help"), format!("unknown command “{other}”; yandi admin --help"))),
    }
    Ok(())
}

const HELP_RU: &str = "yandi admin — управление запущенным узлом из консоли

  yandi admin                      меню
  yandi admin status               состояние узла
  yandi admin pairing [owner|user] [--host АДРЕС]
                                   код и QR сопряжения телефона
  yandi admin devices              сопряжённые устройства
  yandi admin device-remove ID     отвязать устройство
  yandi admin device-promote ID    подтвердить приглашённое устройство: оно станет пользователем и сможет приглашать само
  yandi admin device-revoke-branch ID   отозвать устройство и всех, кого оно пригласило
  yandi admin invites show | set --enabled on|off --per-day N --max-live N
  yandi admin bootstrap pubkey | sign ФАЙЛ [--days N]   ключ подписи списка узлов — только в закрытой среде, файла нет
  yandi admin phone-action ПИР ДЕЙСТВИЕ
  yandi admin proxies list|add [вид]|remove ID|test [вид]|settings --enabled on|off --strict on|off --max-clients N
  yandi admin vault status|lock|unlock|change-password|panic-erase
  yandi admin node stop|start
  yandi admin alerts | update
  yandi admin raw МЕТОД /api/путь [json]
Параметры: --socket ПУТЬ, --admin-password-stdin, --lang ru|en
Админские действия просят пароль администратора (сеанс 15 минут). Опасные (стирание, отвязка) просят ввести слово подтверждения.";

const HELP_EN: &str = "yandi admin — manage the running node from the console

  yandi admin                      menu
  yandi admin status               node status
  yandi admin pairing [owner|user] [--host ADDRESS]
                                   phone pairing code and QR
  yandi admin devices              paired devices
  yandi admin device-remove ID     unpair a device
  yandi admin device-promote ID    confirm an invited device: it becomes a user and can invite others itself
  yandi admin device-revoke-branch ID   revoke a device and everyone it invited
  yandi admin invites show | set --enabled on|off --per-day N --max-live N
  yandi admin bootstrap pubkey | sign FILE [--days N]   the node-list signing key lives only in the closed environment, no file
  yandi admin phone-action PEER ACTION
  yandi admin proxies list|add [kind]|remove ID|test [kind]|settings --enabled on|off --strict on|off --max-clients N
  yandi admin vault status|lock|unlock|change-password|panic-erase
  yandi admin node stop|start
  yandi admin alerts | update
  yandi admin raw METHOD /api/path [json]
Options: --socket PATH, --admin-password-stdin, --lang ru|en
Administrator actions ask for the administrator password (a 15 minute session). Dangerous ones (erase, unpair) ask you to type a confirmation word.";

fn help() -> &'static str {
    t!(HELP_RU, HELP_EN)
}

struct Common {
    socket: PathBuf,
    admin_stdin: bool,
    rest: Vec<String>,
}

fn common(argv: Vec<String>) -> Result<Common, String> {
    let mut socket = default_socket();
    let mut admin_stdin = false;
    let mut rest = Vec::new();
    let mut it = argv.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--socket" => socket = it.next().ok_or(t!("после --socket нужен путь", "--socket needs a path"))?.into(),
            "--admin-password-stdin" => admin_stdin = true,
            _ => rest.push(a),
        }
    }
    Ok(Common { socket, admin_stdin, rest })
}

fn menu(s: &mut Session) -> i32 {
    let items: Vec<(&str, &str)> = vec![
        (t!("Состояние узла", "Node status"), "status"),
        (t!("Сопряжение телефона (код и QR)", "Phone pairing (code and QR)"), "pairing"),
        (t!("Сопряжённые устройства", "Paired devices"), "devices"),
        (t!("Прокси узла", "Node proxies"), "proxies"),
        (t!("Закрытая среда", "Closed environment"), "vault status"),
        (t!("Извещения", "Alerts"), "alerts"),
        (t!("Обновления", "Updates"), "update"),
    ];
    loop {
        eprintln!("{}", t!("\nYANDI — управление узлом", "\nYANDI — node management"));
        for (i, (name, _)) in items.iter().enumerate() {
            eprintln!("  {}. {name}", i + 1);
        }
        eprintln!("{}", t!("  9. Опасные действия (стирание, отвязка, смена пароля)\n  0. Выход", "  9. Dangerous actions (erase, unpair, change password)\n  0. Exit"));
        let choice = match Secrets::ask_visible(t!("Пункт: ", "Item: ")) {
            Ok(c) => c,
            Err(_) => return 1,
        };
        let args: Vec<String> = match choice.trim() {
            "0" | "q" | "" => return 0,
            "9" => {
                eprintln!("{}", t!("  a. Отвязать устройство\n  b. Стереть закрытую среду\n  c. Сменить пароль владельца\n  d. Остановить узел", "  a. Unpair a device\n  b. Erase the closed environment\n  c. Change the owner password\n  d. Stop the node"));
                match Secrets::ask_visible(t!("Действие: ", "Action: ")).unwrap_or_default().trim() {
                    "a" => {
                        let id = Secrets::ask_visible(t!("peer_id устройства: ", "Device peer_id: ")).unwrap_or_default();
                        vec!["device-remove".into(), id.trim().to_string()]
                    }
                    "b" => vec!["vault".into(), "panic-erase".into()],
                    "c" => vec!["vault".into(), "change-password".into()],
                    "d" => vec!["node".into(), "stop".into()],
                    _ => continue,
                }
            }
            n => match n.parse::<usize>().ok().and_then(|n| items.get(n.wrapping_sub(1))) {
                Some((_, cmd)) => cmd.split(' ').map(String::from).collect(),
                None => continue,
            },
        };
        if let Err(e) = dispatch(s, &args) {
            eprintln!("{}{e}", t!("Ошибка: ", "Error: "));
        }
    }
}

/// Выбрать язык консоли: ключ `--lang` (убирается из аргументов), `YANDI_LANG`, сохранённый выбор, язык системы. В `yandi setup` при запуске
/// в терминале без явного выбора спрашивает язык и запоминает ответ.
fn init_lang(argv: Vec<String>, ask: bool) -> Result<Vec<String>, String> {
    use crate::i18n::{self, Lang};
    let mut rest = Vec::new();
    let mut flag: Option<String> = None;
    let mut it = argv.into_iter();
    while let Some(a) = it.next() {
        if a == "--lang" {
            flag = Some(it.next().ok_or("--lang: ru | en")?);
        } else if let Some(v) = a.strip_prefix("--lang=") {
            flag = Some(v.to_string());
        } else {
            rest.push(a);
        }
    }
    let explicit = flag.is_some() || std::env::var("YANDI_LANG").ok().as_deref().and_then(i18n::parse).is_some();
    let lang = i18n::detect(flag.as_deref())?;
    i18n::set(lang);
    let help = rest.iter().any(|a| matches!(a.as_str(), "--help" | "-h" | "help"));
    if ask && !help {
        let tty = unsafe { libc::isatty(0) == 1 && libc::isatty(2) == 1 };
        let mut chosen = lang;
        if !explicit && tty {
            let q = format!("Выберите язык / Select language [ru/en] ({}): ", if lang == Lang::Ru { "ru" } else { "en" });
            for _ in 0..3 {
                let a = Secrets::ask_visible(&q)?;
                if a.trim().is_empty() {
                    break;
                }
                if let Some(l) = i18n::parse(&a) {
                    chosen = l;
                    break;
                }
                eprintln!("ru | en");
            }
            i18n::set(chosen);
        }
        i18n::save(chosen);
    }
    Ok(rest)
}

pub fn run_admin(argv: Vec<String>) -> i32 {
    let argv = match init_lang(argv, false) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("yandi admin: {e}");
            return 2;
        }
    };
    if argv.first().is_some_and(|a| matches!(a.as_str(), "--help" | "-h" | "help")) {
        println!("{}", help());
        return 0;
    }
    let c = match common(argv) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("yandi admin: {e}");
            return 2;
        }
    };
    let mut s = Session::new(c.socket, c.admin_stdin);
    if c.rest.is_empty() {
        return menu(&mut s);
    }
    match dispatch(&mut s, &c.rest) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("yandi admin: {e}");
            1
        }
    }
}

// ───────────────────────── первая настройка ─────────────────────────

/// Пароль не короче порога, который называет сам узел.
fn check_password_pair(a: &str, b: &str, min: usize) -> Result<(), String> {
    if a.chars().count() < min {
        return Err(t!(format!("пароль — не короче {min} знаков"), format!("the password must be at least {min} characters")));
    }
    if a != b {
        return Err(t!("пароли не совпадают", "passwords do not match").into());
    }
    Ok(())
}

/// Фраза как её ждёт узел: слова через пробел, регистр и порядок важны.
pub fn normalize_phrase(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn write_private_new(path: &Path, content: &str) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(content.as_bytes()).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())
}

const SETUP_HELP_RU: &str = "yandi setup — первая настройка узла из консоли

  yandi setup [--words 12|24] [--seed-file ФАЙЛ] [--password-stdin] [--socket ПУТЬ] [--lang ru|en]

Узел должен быть уже запущен (служба yandi-node) и ещё не настроен. Мастер спрашивает пароль, создаёт ключи и закрытую среду, показывает
фразу восстановления (12 или 24 слова). Запишите её на бумаге: по ней и по файлу копии личность восстанавливается на новой машине.
Фраза показывается только в терминале; без терминала нужен --seed-file (новый файл с правами 0600 — удалите его после переписывания).
--password-stdin: пароль входа первой строкой стандартного ввода (для скриптов).
--lang ru|en: язык консоли (без ключа спрашивается в начале; выбор запоминается).";

const SETUP_HELP_EN: &str = "yandi setup — first-time node setup from the console

  yandi setup [--words 12|24] [--seed-file FILE] [--password-stdin] [--socket PATH] [--lang ru|en]

The node must already be running (the yandi-node service) and not set up yet. The wizard asks for a password, creates the keys and the closed environment,
and shows the recovery phrase (12 or 24 words). Write it down on paper: with it and the backup file the identity is restored on a new machine.
The phrase is shown only in a terminal; without a terminal use --seed-file (a new file with mode 0600 — delete it after copying).
--password-stdin: the login password as the first line of standard input (for scripts).
--lang ru|en: console language (asked at the start if not given; the choice is remembered).";

fn setup_help() -> &'static str {
    t!(SETUP_HELP_RU, SETUP_HELP_EN)
}

pub fn run_setup(argv: Vec<String>) -> i32 {
    let argv = match init_lang(argv, true) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("yandi setup: {e}");
            return 2;
        }
    };
    if argv.first().is_some_and(|a| matches!(a.as_str(), "--help" | "-h" | "help")) {
        println!("{}", setup_help());
        return 0;
    }
    let mut socket = default_socket();
    let mut words: u32 = 12;
    let mut seed_file: Option<PathBuf> = None;
    let mut password_stdin = false;
    let mut it = argv.into_iter();
    while let Some(a) = it.next() {
        let r: Result<(), String> = (|| {
            match a.as_str() {
                "--socket" => socket = it.next().ok_or(t!("после --socket нужен путь", "--socket needs a path"))?.into(),
                "--words" => words = it.next().and_then(|n| n.parse().ok()).filter(|n| *n == 12 || *n == 24).ok_or(t!("--words 12 или 24", "--words 12 or 24"))?,
                "--seed-file" => seed_file = Some(it.next().ok_or(t!("после --seed-file нужен путь", "--seed-file needs a path"))?.into()),
                "--password-stdin" => password_stdin = true,
                other => return Err(t!(format!("неизвестный параметр {other}"), format!("unknown option {other}"))),
            }
            Ok(())
        })();
        if let Err(e) = r {
            eprintln!("yandi setup: {e}");
            return 2;
        }
    }
    match setup(&socket, words, seed_file.as_deref(), password_stdin) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("yandi setup: {e}");
            1
        }
    }
}

fn setup(socket: &Path, words: u32, seed_file: Option<&Path>, password_stdin: bool) -> Result<(), String> {
    let t = Duration::from_secs(120);
    let (code, st) = roundtrip(socket, "GET", "/api/auth/state", None, None, t)?;
    if code != 200 {
        return Err(error_text(code, &st));
    }
    if st["setup_required"] != true {
        return Err(t!("узел уже настроен (для восстановления из копии: yandi backup restore, затем запуск службы)", "the node is already set up (to restore from a backup: yandi backup restore, then start the service)").into());
    }
    let tty = unsafe { libc::isatty(0) == 1 && libc::isatty(2) == 1 };
    if !tty && seed_file.is_none() {
        return Err(t!("нет терминала: фразу восстановления некуда безопасно показать; запустите в терминале или добавьте --seed-file ФАЙЛ", "no terminal: there is nowhere to show the recovery phrase safely; run in a terminal or add --seed-file FILE").into());
    }
    let secrets = Secrets { from_stdin: password_stdin, preset: None };
    // 1. фраза от узла
    let (code, seed) = roundtrip(socket, "POST", "/api/auth/seed/new", Some(&json!({ "words": words })), None, t)?;
    if code != 200 {
        return Err(error_text(code, &seed));
    }
    let list: Vec<String> = seed["words"].as_array().ok_or(t!("узел не вернул слова", "the node did not return the words"))?.iter().filter_map(|w| w.as_str().map(String::from)).collect();
    if list.len() as u32 != words {
        return Err(t!("узел вернул неверное число слов", "the node returned a wrong number of words").into());
    }
    let min = seed["min_password"].as_u64().unwrap_or(8) as usize;
    let phrase = list.join(" ");
    // 2. пароли
    let pw = secrets.ask(&t!(format!("Пароль владельца (не короче {min} знаков): "), format!("Owner password (at least {min} characters): ")))?;
    let again = if password_stdin { pw.clone() } else { secrets.ask(t!("Пароль ещё раз: ", "Password again: "))? };
    check_password_pair(&pw, &again, min)?;
    let admin = if password_stdin {
        String::new()
    } else {
        let a = Secrets::ask_visible(t!("Отдельный пароль администратора? (пусто — тот же, иначе введите следом): ", "A separate administrator password? (empty — the same, otherwise type it now): ")).unwrap_or_default();
        if a.trim().is_empty() {
            String::new()
        } else {
            let p1 = secrets.ask(t!("Пароль администратора: ", "Administrator password: "))?;
            let p2 = secrets.ask(t!("Ещё раз: ", "Again: "))?;
            check_password_pair(&p1, &p2, min)?;
            p1
        }
    };
    // 3. показать фразу и убедиться, что её записали
    if let Some(path) = seed_file {
        write_private_new(path, &format!("{phrase}\n"))?;
        eprintln!("{}", t!(format!("Фраза записана в {} (права 0600). Перепишите её и удалите файл.", path.display()), format!("The phrase is written to {} (mode 0600). Copy it down and delete the file.", path.display())));
    } else {
        eprintln!("{}", t!(format!("\nФраза восстановления ({words} слов). Запишите её на бумаге, порядок важен:\n"), format!("\nRecovery phrase ({words} words). Write it down on paper, the order matters:\n")));
        for (i, w) in list.iter().enumerate() {
            eprintln!("  {:>2}. {w}", i + 1);
        }
        eprintln!();
        let typed = Secrets::ask_visible(t!("Для проверки введите фразу целиком: ", "To verify, type the whole phrase: "))?;
        if normalize_phrase(&typed) != phrase {
            return Err(t!("фраза введена неверно: ничего не создано, запустите мастер снова", "the phrase was typed wrongly: nothing was created, run the wizard again").into());
        }
    }
    // 4. создание ключей, ожидание запуска узла, закрытая среда
    let mut body = json!({
        "login_password": pw, "login_password_repeat": pw,
        "master_password": phrase, "master_password_repeat": phrase,
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
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    loop {
        std::thread::sleep(Duration::from_secs(1));
        if let Ok((200, st)) = roundtrip(socket, "GET", "/api/auth/state", None, None, Duration::from_secs(10)) {
            if st["setup_required"] == false {
                break;
            }
        }
        if std::time::Instant::now() > deadline {
            return Err(t!("узел не запустился за 120 секунд: проверьте журнал службы (journalctl -u yandi-node)", "the node did not start within 120 seconds: check the service log (journalctl -u yandi-node)").into());
        }
    }
    let (code, v) = roundtrip(socket, "POST", "/api/vault/setup", Some(&json!({ "password": pw, "phrase": phrase })), None, Duration::from_secs(300))?;
    if code != 200 {
        return Err(t!(format!("ключи созданы, но закрытая среда не создана: {}", error_text(code, &v)), format!("the keys are created, but the closed environment is not: {}", error_text(code, &v))));
    }
    eprintln!("{}", t!("Закрытая среда создана. Готово.", "The closed environment is created. Done."));
    eprintln!("{}", t!("Дальше: yandi admin pairing — код сопряжения телефона; yandi admin — меню управления.", "Next: yandi admin pairing — phone pairing code; yandi admin — the management menu."));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn the_language_flag_is_taken_out_of_the_arguments_and_switches_every_message() {
        use crate::i18n::{self, Lang};
        let rest = init_lang(vec!["status".into(), "--lang".into(), "en".into()], false).unwrap();
        assert_eq!(rest, vec!["status".to_string()], "ключ языка не доходит до команд");
        assert_eq!(i18n::get(), Lang::En);
        assert!(help().contains("manage the running node"));
        assert!(setup_help().contains("first-time node setup"));
        assert_eq!(error_text(401, &Value::Null), "wrong password or access denied");
        assert_eq!(error_text(429, &Value::Null), "too many attempts: wait and try again");
        assert!(error_text(500, &json!({"error": "boom"})).starts_with("the node answered 500"));
        assert_eq!(role_label("invited"), "invited");
        // та же консоль по-русски
        let rest = init_lang(vec!["--lang=ru".into(), "devices".into()], false).unwrap();
        assert_eq!(rest, vec!["devices".to_string()]);
        assert_eq!(i18n::get(), Lang::Ru);
        assert!(help().contains("управление запущенным узлом"));
        assert_eq!(error_text(401, &Value::Null), "неверный пароль или доступ запрещён");
        assert_eq!(role_label("invited"), "приглашённый");
        // неизвестный язык — ошибка на двух языках, а не молчаливый выбор
        assert!(init_lang(vec!["--lang".into(), "de".into()], false).is_err());
        assert!(init_lang(vec!["--lang".into()], false).is_err());
    }

    #[test]
    fn protocol_errors_and_prompts_follow_the_language_but_the_erase_word_on_the_wire_does_not() {
        use crate::i18n::{self, Lang};
        i18n::set(Lang::En);
        let e = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nab").unwrap_err();
        assert!(e.contains("does not match the declared length"), "{e}");
        assert_eq!(t!("СТЕРЕТЬ", "ERASE"), "ERASE", "слово подтверждения по-английски");
        i18n::set(Lang::Ru);
        let e = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nab").unwrap_err();
        assert!(e.contains("не совпадает с заявленной длиной"), "{e}");
        // по сети слово остаётся прежним (его ждёт узел), независимо от языка
        let src = include_str!("admin_cli.rs");
        assert!(src.contains(r#"json!({ "confirm": "СТЕРЕТЬ" })"#));
    }


    fn serve_once(dir: &Path, name: &str, reply: Vec<u8>) -> (PathBuf, std::thread::JoinHandle<String>) {
        let sock = dir.join(name);
        let l = UnixListener::bind(&sock).unwrap();
        let h = std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let mut buf = vec![0u8; 8192];
            let mut got = Vec::new();
            loop {
                let n = c.read(&mut buf).unwrap();
                got.extend_from_slice(&buf[..n]);
                // конец запроса: заголовки и тело по Content-Length
                if let Some(p) = got.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&got[..p]).to_ascii_lowercase();
                    let len = head.lines().find_map(|l| l.strip_prefix("content-length: ")).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
                    if got.len() >= p + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            c.write_all(&reply).unwrap();
            String::from_utf8_lossy(&got).to_string()
        });
        (sock, h)
    }

    #[test]
    fn plain_and_chunked_responses_are_parsed() {
        let (c, b) = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap();
        assert_eq!((c, b.as_slice()), (200, b"{}".as_slice()));
        let chunked = b"HTTP/1.1 403 Forbidden\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n{\"a\":\r\n3\r\n1}\n\r\n0\r\n\r\n";
        let (c, b) = parse_response(chunked).unwrap();
        assert_eq!(c, 403);
        assert_eq!(serde_json::from_slice::<Value>(&b).unwrap()["a"], 1);
        assert!(parse_response(b"HTTP/1.1 200 OK\r\n").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n9\r\nshort").is_err());
    }

    #[test]
    fn a_request_carries_the_token_and_the_json_body() {
        let dir = tempfile::tempdir().unwrap();
        let (sock, h) = serve_once(dir.path(), "a.sock", b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n{\"ok\":true}".to_vec());
        let (code, v) = roundtrip(&sock, "POST", "/api/x", Some(&json!({"k": 1})), Some("tok123"), Duration::from_secs(5)).unwrap();
        assert_eq!((code, v["ok"] == true), (200, true));
        let req = h.join().unwrap();
        assert!(req.starts_with("POST /api/x HTTP/1.1\r\n"));
        assert!(req.contains("x-yandi-admin: tok123\r\n"));
        assert!(req.contains("Content-Length: 7\r\n") && req.ends_with("{\"k\":1}"));
    }

    #[test]
    fn a_missing_socket_is_a_clear_error() {
        let e = roundtrip(Path::new("/nonexistent/yandi-ui.sock"), "GET", "/api/status", None, None, Duration::from_secs(1)).unwrap_err();
        assert!(e.contains("узел не отвечает"));
    }

    #[test]
    fn error_texts_do_not_echo_whole_bodies() {
        assert_eq!(error_text(401, &json!({})), "неверный пароль или доступ запрещён");
        let long = "x".repeat(2000);
        let t = error_text(400, &json!({ "error": long }));
        assert!(t.len() < 400, "{}", t.len());
        assert_eq!(error_text(500, &Value::Null), "узел ответил 500");
    }

    #[test]
    fn phrase_normalization_and_password_checks() {
        assert_eq!(normalize_phrase("  alpha   beta\tgamma\n"), "alpha beta gamma");
        assert!(check_password_pair("short", "short", 8).is_err());
        assert!(check_password_pair("longenough", "different1", 8).is_err());
        assert!(check_password_pair("longenough", "longenough", 8).is_ok());
    }

    #[test]
    fn the_seed_file_is_new_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("seed.txt");
        write_private_new(&p, "a b c\n").unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(write_private_new(&p, "other").is_err(), "существующий файл не перезаписывается");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "a b c\n");
    }

    #[test]
    fn raw_and_pairing_reject_dangerous_arguments_before_any_request() {
        let mut s = Session::new(PathBuf::from("/nonexistent/s.sock"), true);
        let bad = |s: &mut Session, a: &[&str]| dispatch(s, &a.iter().map(|x| x.to_string()).collect::<Vec<_>>()).unwrap_err();
        assert!(bad(&mut s, &["raw", "GET", "/etc/passwd"]).contains("/api/"));
        assert!(bad(&mut s, &["raw", "TRACE", "/api/x"]).contains("метод"));
        assert!(bad(&mut s, &["raw", "GET", "/api/x y"]).contains("/api/"));
        assert!(bad(&mut s, &["pairing", "--host", "a b"]).contains("адрес"));
        assert!(bad(&mut s, &["pairing", "--host", "x\r\nHost: evil"]).contains("адрес"));
        assert!(bad(&mut s, &["device-remove", "zz"]).contains("peer_id"));
        assert!(bad(&mut s, &["proxies", "remove", "../x"]).contains("идентификатор"));
        assert!(bad(&mut s, &["proxies", "add", "ftp"]).contains("вид прокси"));
        assert!(bad(&mut s, &["nonsense"]).contains("неизвестная"));
    }

    fn http(code: &str, body: &str) -> String {
        format!("HTTP/1.1 {code}\r\nContent-Length: {}\r\n\r\n{body}", body.len())
    }

    #[test]
    fn the_admin_password_is_asked_and_the_call_is_retried_with_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("m.sock");
        let l = UnixListener::bind(&sock).unwrap();
        let h = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for reply in [
                http("403 Forbidden", r#"{"error":"admin_required"}"#),
                http("200 OK", r#"{"ok":true,"token":"secrettk"}"#),
                http("200 OK", r#"{"done":1}"#),
            ] {
                let (mut c, _) = l.accept().unwrap();
                let mut buf = vec![0u8; 8192];
                let n = c.read(&mut buf).unwrap();
                let mut got = buf[..n].to_vec();
                if let Some(p) = got.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&got[..p]).to_ascii_lowercase();
                    let len = head.lines().find_map(|l| l.strip_prefix("content-length: ")).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
                    while got.len() < p + 4 + len {
                        let m = c.read(&mut buf).unwrap();
                        got.extend_from_slice(&buf[..m]);
                    }
                }
                seen.push(String::from_utf8_lossy(&got).to_string());
                c.write_all(reply.as_bytes()).unwrap();
            }
            seen
        });
        let mut s = Session::new(sock, false);
        s.secrets.preset = Some("adminpw".into());
        let (code, v) = s.call("GET", "/api/alerts", None).unwrap();
        assert_eq!((code, v["done"].as_i64()), (200, Some(1)));
        assert_eq!(s.token.as_deref(), Some("secrettk"));
        let seen = h.join().unwrap();
        assert!(!seen[0].contains("x-yandi-admin"), "первый запрос без токена");
        assert!(seen[1].starts_with("POST /api/admin/session") && seen[1].contains(r#""password":"adminpw""#));
        assert!(seen[2].starts_with("GET /api/alerts") && seen[2].contains("x-yandi-admin: secrettk"));
    }

    #[test]
    fn truncated_or_inconsistent_responses_are_errors_not_empty_successes() {
        // тело короче заявленного (обрыв) и длиннее
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"ok\":").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}xx").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 3\r\n\r\n{}").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: x\r\n\r\n{}").is_err());
        // по частям: нет CRLF после данных, нет завершения, огромный размер
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}XX0\r\n\r\n").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffffffffffffffff\r\n{}\r\n").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n").is_ok());
    }

    #[test]
    fn a_non_json_success_is_a_protocol_error() {
        let dir = tempfile::tempdir().unwrap();
        let (sock, h) = serve_once(dir.path(), "j.sock", http("200 OK", "not json").into_bytes());
        let e = roundtrip(&sock, "GET", "/api/x", None, None, Duration::from_secs(5)).unwrap_err();
        h.join().unwrap();
        assert!(e.contains("не JSON"), "{e}");
        let (sock, h) = serve_once(dir.path(), "k.sock", http("500 Internal Server Error", "oops").into_bytes());
        let (code, v) = roundtrip(&sock, "GET", "/api/x", None, None, Duration::from_secs(5)).unwrap();
        h.join().unwrap();
        assert_eq!((code, v), (500, Value::Null));
    }

    #[test]
    fn proxy_settings_accept_only_on_or_off() {
        let mut s = Session::new(PathBuf::from("/nonexistent/s.sock"), true);
        let run = |s: &mut Session, a: &[&str]| dispatch(s, &a.iter().map(|x| x.to_string()).collect::<Vec<_>>()).unwrap_err();
        for a in [&["proxies", "settings", "--strict"][..], &["proxies", "settings", "--strict", "onn"], &["proxies", "settings", "--enabled", "--strict", "on"], &["proxies", "settings", "--max-clients", "x"]] {
            let e = run(&mut s, a);
            assert!(e.contains("on или off") || e.contains("max-clients"), "{a:?}: {e}");
        }
    }

    #[test]
    fn a_long_basename_in_a_short_directory_still_connects() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        // предел sun_path: 108 на Linux, 104 на macOS — берём длину ниже предела, но выше порога запасного пути на Linux
        let total = if cfg!(target_os = "linux") { 104 } else { 96 };
        let name = "s".repeat(total - dir.path().as_os_str().len() - 1);
        let p = dir.path().join(&name);
        let l = UnixListener::bind(&p).unwrap();
        assert!(p.as_os_str().len() <= total, "{}", p.as_os_str().len());
        let h = std::thread::spawn(move || l.accept().is_ok());
        assert!(connect(&p).is_ok(), "путь допустимой длины должен подключаться напрямую");
        assert!(h.join().unwrap());
    }

    #[test]
    fn help_is_recognized_only_as_the_first_argument() {
        // значение параметра с именем help не отменяет команду (до разбора сокета дело не доходит из-за неверного параметра)
        assert_eq!(run_setup(vec!["--seed-file".into(), "help".into(), "--bogus".into()]), 2);
        assert_eq!(run_admin(vec!["--bogus-flag".into(), "help".into(), "x".into()]), 1);
    }

    #[test]
    fn ambiguous_framing_and_empty_successes_are_refused() {
        // обрыв и хвосты после нулевого куска
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\nX: unfinished\r\n").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\nGARBAGE\r\n").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\nX: ok\r\n\r\n").is_ok());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\nbad line\r\n\r\n").is_err());
        // кодировки и длина
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: notchunked\r\n\r\n0\r\n\r\n").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\nContent-Length: 999\r\n\r\n0\r\n\r\n").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 2\r\n\r\n2\r\n{}\r\n0\r\n\r\n").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: +2\r\n\r\n{}").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 0x2\r\n\r\n{}").is_err());
        // пустой успех — ошибка протокола, кроме 204; пустая ошибка допустима
        let dir = tempfile::tempdir().unwrap();
        let (sock, h) = serve_once(dir.path(), "e.sock", b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec());
        let e = roundtrip(&sock, "POST", "/api/node/stop", None, None, Duration::from_secs(5)).unwrap_err();
        h.join().unwrap();
        assert!(e.contains("пуст"), "{e}");
        let (sock, h) = serve_once(dir.path(), "f.sock", b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec());
        assert_eq!(roundtrip(&sock, "POST", "/api/x", None, None, Duration::from_secs(5)).unwrap(), (204, Value::Null));
        h.join().unwrap();
        let (sock, h) = serve_once(dir.path(), "g.sock", b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n".to_vec());
        assert_eq!(roundtrip(&sock, "GET", "/api/x", None, None, Duration::from_secs(5)).unwrap(), (500, Value::Null));
        h.join().unwrap();
    }

    #[test]
    fn invitation_commands_validate_before_any_request() {
        let mut s = Session::new(PathBuf::from("/nonexistent/s.sock"), true);
        let bad = |s: &mut Session, a: &[&str]| dispatch(s, &a.iter().map(|x| x.to_string()).collect::<Vec<_>>()).unwrap_err();
        assert!(bad(&mut s, &["device-promote", "zz"]).contains("номер устройства"));
        assert!(bad(&mut s, &["device-promote"]).contains("device-promote ID"));
        assert!(bad(&mut s, &["device-revoke-branch", "../x"]).contains("номер устройства"));
        assert!(bad(&mut s, &["invites", "set"]).contains("--enabled"), "без параметров ничего не меняем");
        assert!(bad(&mut s, &["invites", "bogus"]).contains("неизвестное"));
        assert_eq!(role_label("invited"), "приглашённый");
    }
}
