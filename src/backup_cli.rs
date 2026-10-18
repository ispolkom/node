//! `yandi backup ...` — работа с файлом резервной копии без запуска узла.
//!
//!   yandi backup inspect FILE                 что за копия (номер, время, узел); код не нужен
//!   yandi backup verify  FILE                 открыть кодом и перечислить содержимое; ничего не записывается
//!   yandi backup restore FILE [--force]       восстановить связи на этой машине (узел не должен работать)
//!
//!   параметры: --code-stdin (код/мастер-пароль строкой со стандартного ввода: для скриптов и проверок), --yes (без вопроса «продолжить»),
//!   --port N (9000), --keys-dir/--data-dir/--home-dir D (другие каталоги, например для пробного восстановления),
//!   --machine-id ID (для проверок)
//!
//! Код восстановления — тот, что показан при первой настройке узла (подходит и мастер-пароль, на который сделана обёртка).
//! Он вводится без эха и нигде не сохраняется. После записи файлов для этой машины создаётся новый ключ устройства
//! (`yandi-keys recover` внутри), поэтому узел стартует без повторного ввода кода.

use crate::node_backup::{self, Layout, RestoreOptions};
use std::io::BufRead;
use std::path::PathBuf;

struct Args {
    cmd: String,
    file: PathBuf,
    force: bool,
    yes: bool,
    code_stdin: bool,
    layout: Layout,
    machine_id: Option<String>,
}

fn parse(argv: Vec<String>) -> Result<Args, String> {
    let mut it = argv.into_iter();
    let cmd = it.next().ok_or("нужна команда: inspect | verify | restore")?;
    let mut file: Option<PathBuf> = None;
    let (mut force, mut yes, mut code_stdin) = (false, false, false);
    let mut port: u16 = 9000;
    let (mut keys, mut data, mut home): (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>) = (None, None, None);
    let mut machine_id = None;
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().ok_or(format!("после {name} нужно значение"));
        match a.as_str() {
            "--force" => force = true,
            "--yes" => yes = true,
            "--code-stdin" => code_stdin = true,
            "--port" => port = val("--port")?.parse().map_err(|_| "порт — число".to_string())?,
            "--keys-dir" => keys = Some(val("--keys-dir")?.into()),
            "--data-dir" => data = Some(val("--data-dir")?.into()),
            "--home-dir" => home = Some(val("--home-dir")?.into()),
            "--machine-id" => machine_id = Some(val("--machine-id")?),
            s if s.starts_with("--") => return Err(format!("неизвестный параметр {s}")),
            _ => file = Some(a.into()),
        }
    }
    let live = Layout::live(port);
    Ok(Args {
        cmd,
        file: file.ok_or("нужен путь к файлу .yandibak")?,
        force,
        yes,
        code_stdin,
        layout: Layout { keys: keys.unwrap_or(live.keys), data: data.unwrap_or(live.data), home: home.unwrap_or(live.home), port },
        machine_id,
    })
}

fn read_line() -> Result<String, String> {
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).map_err(|e| e.to_string())?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

/// Строка без эха (нужен терминал) или одна строка со стандартного ввода.
fn ask_secret(prompt: &str, from_stdin: bool) -> Result<String, String> {
    if from_stdin {
        return read_line();
    }
    eprint!("{prompt}");
    use std::io::Write;
    let _ = std::io::stderr().flush();
    unsafe {
        if libc::isatty(0) != 1 {
            return Err("стандартный ввод — не терминал; используйте --code-stdin".into());
        }
        let mut old: libc::termios = std::mem::zeroed();
        libc::tcgetattr(0, &mut old);
        let mut quiet = old;
        quiet.c_lflag &= !libc::ECHO;
        libc::tcsetattr(0, libc::TCSANOW, &quiet);
        let line = read_line();
        libc::tcsetattr(0, libc::TCSANOW, &old);
        eprintln!();
        line
    }
}

fn describe(h: &node_backup::Header) -> String {
    let (y, mo, d, hh, mi) = node_backup::civil_utc(h.created_ms / 1000);
    format!("копия №{} от {y:04}-{mo:02}-{d:02} {hh:02}:{mi:02} UTC; узел {}", h.counter, h.node_id)
}

/// Работает ли узел с этим каталогом данных (сокет оболочки отвечает): восстановление поверх живого узла запрещено.
fn node_is_running(l: &Layout) -> bool {
    std::os::unix::net::UnixStream::connect(l.data.join("run").join("yandi-ui.sock")).is_ok()
}

pub fn run(argv: Vec<String>) -> i32 {
    let a = match parse(argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("yandi backup: {e}");
            return 2;
        }
    };
    let bytes = match std::fs::read(&a.file) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("yandi backup: {}: {e}", a.file.display());
            return 2;
        }
    };
    let header = match node_backup::read_header(&bytes) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("yandi backup: {e}");
            return 2;
        }
    };
    println!("{}", describe(&header));
    if a.cmd == "inspect" {
        println!("в копии только связи; переписка, файлы, звонки и журналы групп не входят");
        return 0;
    }
    if a.cmd != "verify" && a.cmd != "restore" {
        eprintln!("yandi backup: неизвестная команда {}", a.cmd);
        return 2;
    }
    let code = match ask_secret("Сид-фраза (12 или 24 слова), код восстановления или мастер-пароль: ", a.code_stdin) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("yandi backup: {e}");
            return 2;
        }
    };
    let opened = match node_backup::open_with_code(&bytes, &code) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("yandi backup: {e}");
            return 1;
        }
    };
    let entries = match opened.entries() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("yandi backup: {e}");
            return 1;
        }
    };
    println!("копия открыта, файлов: {}; не входит: {}", entries.len(), opened.manifest.excluded.join(", "));
    if a.cmd == "verify" {
        for e in &entries {
            println!("  {:?}/{} ({} байт)", e.area, e.path, e.data.len());
        }
        println!("проверка пройдена: файл цел, код подходит. Ничего не записано.");
        return 0;
    }
    if node_is_running(&a.layout) {
        eprintln!("yandi backup: узел с этим каталогом данных сейчас работает. Остановите его (sudo systemctl stop yandi-node) и повторите.");
        return 1;
    }
    if !a.yes {
        eprintln!("Будут записаны ключи и связи узла {} в {} и {}.", opened.header.node_id, a.layout.keys.display(), a.layout.data.display());
        eprint!("Продолжить? (введите yes): ");
        if read_line().map(|l| l.trim() != "yes").unwrap_or(true) {
            eprintln!("отменено, ничего не записано");
            return 1;
        }
    }
    // Копия с закрытой средой: пароль прежней установки не переносится, для этой машины задаётся новый (как в кошельках).
    let has_vault = entries.iter().any(|e| e.path == "closed/envelope.json");
    // новый пароль нужен и без закрытой среды: он заменяет пароль входа (иначе перенесённый старый остался бы действовать)
    let new_password = {
        let first = match ask_secret("Новый пароль для этой машины (не короче 8 знаков, на тестовом узле 6): ", a.code_stdin) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("yandi backup: {e}");
                return 2;
            }
        };
        #[cfg(unix)]
        if let Err(e) = crate::web::auth::check_new_password(&first) {
            eprintln!("yandi backup: {e}; ничего не записано");
            return 2;
        }
        #[cfg(not(unix))]
        if first.chars().count() < 6 || first.len() > 1024 {
            eprintln!("yandi backup: пароль не короче 6 знаков и не длиннее 1024 байт; ничего не записано");
            return 2;
        }
        if !a.code_stdin {
            let again = ask_secret("Повторите пароль: ", false).unwrap_or_default();
            if again != first {
                eprintln!("yandi backup: пароли не совпали; ничего не записано");
                return 2;
            }
        }
        Some(first)
    };
    let report = match node_backup::restore(&opened, &a.layout, &RestoreOptions { force: a.force }) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("yandi backup: {e}");
            return 1;
        }
    };
    println!("записано файлов: {}; групп восстановлено: {}", report.written.len(), report.groups);
    for old in &report.kept_old {
        println!("прежний файл сохранён: {}", old.display());
    }
    for u in &report.unclean {
        eprintln!("ВНИМАНИЕ, не очищено от секретов старого формата: {u}");
    }
    // ключ устройства для ЭТОЙ машины: корень открывается кодом, для него делается новая машинная обёртка
    let machine: Box<dyn key_root::MachineContext> = match &a.machine_id {
        Some(id) => Box::new(key_root::FixedMachine(id.clone())),
        None => Box::new(key_root::SystemMachine),
    };
    let dir = match key_root::KeyDir::open(&a.layout.keys) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("yandi backup: каталог ключей: {e}");
            return 1;
        }
    };
    let device = key_root::HardwareDeviceKey::new(dir.file("device.seal"), dir.file("device.key"));
    let policy = key_root::KdfPolicy::production();
    let secret = key_root::recovery_secret(&code);
    let rec = key_root::Recovery { dir: &dir, port: a.layout.port, machine: machine.as_ref(), device: &device, password: &secret, policy: &policy };
    match rec.run() {
        Ok(r) => {
            println!("личность узла {} восстановлена; для этой машины создан новый ключ устройства.", r.node_id);
            if let Some(pw) = &new_password {
                if has_vault {
                    match node_backup::finalize_vault(&a.layout, &code, pw) {
                        Ok(_) => println!("закрытая среда открыта сид-фразой; пароль прежней установки больше не действует, действует новый."),
                        Err(e) => {
                            eprintln!("yandi backup: файлы и ключи записаны, но закрытая среда не настроена: {e}. Повторите команду с --force и той же сид-фразой.");
                            return 1;
                        }
                    }
                }
                // новый пароль заменяет и пароль входа (перенесённый старый нигде не действует), с закрытой средой и без неё
                match key_root::login::reset_login_min(&dir, &code, pw, &policy, crate::web::auth::min_new_password()) {
                    Ok(_) => println!("пароль входа заменён новым; прежний больше не действует."),
                    Err(e) => {
                        eprintln!("yandi backup: файлы и ключи записаны, но пароль входа не заменён: {e}. Повторите команду с --force и той же сид-фразой.");
                        return 1;
                    }
                }
            }
            println!();
            println!("Дальше: запустите узел (sudo systemctl start yandi-node) и войдите новым паролем.");
            println!("Телефоны и известные узлы подключатся сами. Группы возвращены без сообщений на узле (у участников они остались на телефонах).");
            0
        }
        Err(e) => {
            eprintln!("yandi backup: файлы записаны, но ключ устройства создать не удалось: {e}. Выполните `yandi-keys recover`.");
            1
        }
    }
}
