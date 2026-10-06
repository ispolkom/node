//! yandi-keys — look at, migrate and recover the node's key directory.
//!
//!   yandi-keys status
//!   yandi-keys migrate           legacy → recoverable root; YOU choose the master password (typed twice); --generate-code makes a recovery code instead
//!   yandi-keys new-recovery-code replace the recovery secret by a fresh recovery code (the device opens the key; identity and chats untouched)
//!   yandi-keys recover           new machine / lost device key: type the recovery code
//!   yandi-keys login-check       (used by the assistant's web page) is the line on standard input the web login password? exit 0 yes, 1 no
//!   yandi-keys login-reset       (same) standard input: recovery secret, new login password. Exit 0 done, 1 refused (short message on stderr)
//!   yandi-keys setup             (same) standard input: login password and its repeat, master password and its repeat; creates the keys
//!   yandi-keys core-key          print the key the Core is given (base64), for `python -m agent.db.sql.protect`; only into a pipe, never a terminal
//!
//!   options: --dir D (default ~/.yandi_keys)  --port N (9000)  --machine-id ID
//!            --generate-code  (migrate: make and show a recovery code instead of choosing a password)
//!            --show           (show what you type; by default a secret is typed with no echo)
//!            --password-stdin (read every answer as a line from standard input; for scripts and tests)
//!
//! It prints no key, with one exception: `core-key` writes the derived Core key to a pipe, for the tool that seals the personal memory.
//! The recovery code is printed once, on purpose, by `migrate` and `new-recovery-code`; it is never stored.
use base64::Engine;
use key_root::{
    derive_domain, recovery_code::looks_like_code, status, unlock_root, DeviceKeyProvider,
    FileDeviceKey, FixedMachine, KdfParams, KdfPolicy, KeyDir, KeyRootError, MachineContext,
    Migration, NewRecoveryCode, Recovery, RecoveryCode, SystemMachine, DOMAIN_CORE,
};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use zeroize::Zeroizing;

struct Args {
    command: String,
    dir: PathBuf,
    port: u16,
    stdin: bool,
    generate_code: bool,
    show: bool,
    machine_id: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let command = it
        .next()
        .ok_or("usage: yandi-keys status|migrate|new-recovery-code|recover [options]")?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set")?;
    let mut args = Args {
        command,
        dir: home.join(".yandi_keys"),
        port: 9000,
        stdin: false,
        generate_code: false,
        show: false,
        machine_id: None,
    };
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--dir" => args.dir = PathBuf::from(it.next().ok_or("--dir needs a value")?),
            "--port" => {
                args.port = it
                    .next()
                    .ok_or("--port needs a value")?
                    .parse()
                    .map_err(|_| "--port must be a number")?
            }
            "--machine-id" => {
                args.machine_id = Some(it.next().ok_or("--machine-id needs a value")?)
            }
            "--password-stdin" | "--stdin" => args.stdin = true,
            "--generate-code" => args.generate_code = true,
            "--show" => args.show = true,
            "--own-password" | "--hidden" => {} // the defaults now; accepted so that older instructions still work
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(args)
}

fn read_line_stdin() -> Result<Zeroizing<String>, String> {
    let mut line = Zeroizing::new(String::new());
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|_| "could not read the answer")?;
    while line.ends_with('\n') || line.ends_with('\r') {
        line.pop();
    }
    Ok(line)
}

/// Ask a question. `hidden`: the answer is not echoed (a terminal is required); otherwise it is shown as typed.
fn ask(prompt: &str, from_stdin: bool, hidden: bool) -> Result<Zeroizing<String>, String> {
    if from_stdin {
        return read_line_stdin();
    }
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    if !hidden {
        return read_line_stdin();
    }
    unsafe {
        if libc::isatty(0) != 1 {
            return Err("standard input is not a terminal; use --password-stdin".into());
        }
        let mut old: libc::termios = std::mem::zeroed();
        libc::tcgetattr(0, &mut old);
        let mut quiet = old;
        quiet.c_lflag &= !libc::ECHO;
        libc::tcsetattr(0, libc::TCSANOW, &quiet);
        let line = read_line_stdin();
        libc::tcsetattr(0, libc::TCSANOW, &old);
        eprintln!();
        line
    }
}

fn fail(e: &KeyRootError) -> i32 {
    eprintln!("yandi-keys: {} — {}", e.category(), e);
    2
}

fn show_code(code: &RecoveryCode) {
    println!();
    println!("    ┌───────────────────────────────────────────────────────┐");
    println!("    │  RECOVERY CODE:  {}  │", code.display());
    println!("    └───────────────────────────────────────────────────────┘");
    println!();
    println!("Write it down on paper NOW, exactly as shown. It is the only way to get your identity back on a new machine, and it is");
    println!("shown once: it is not stored anywhere and cannot be recovered. Do not photograph it, do not keep it next to the key files.");
    println!();
}

/// Show the code, then require it to be typed back (visible) before anything is written: proof that it was copied correctly.
fn confirmed_code(args: &Args) -> Result<Option<RecoveryCode>, String> {
    let code = RecoveryCode::generate();
    show_code(&code);
    for attempt in 1..=3 {
        let typed = ask(
            "Type the code back to confirm you have written it down: ",
            args.stdin,
            false,
        )?;
        match RecoveryCode::parse(&typed) {
            Ok(t) if t.same_as(&code) => return Ok(Some(code)),
            Ok(_) => eprintln!(
                "That is a valid code, but not the one shown. Check what you wrote ({attempt}/3)."
            ),
            Err(e) => eprintln!("{e} ({attempt}/3)."),
        }
    }
    eprintln!("yandi-keys: the code was not confirmed; nothing was changed");
    Ok(None)
}

fn run() -> Result<i32, String> {
    let args = parse_args()?;
    let machine: Box<dyn MachineContext> = match &args.machine_id {
        Some(id) => Box::new(FixedMachine(id.clone())),
        None => Box::new(SystemMachine),
    };
    let dir = match KeyDir::open(&args.dir) {
        Ok(d) => d,
        Err(e) => return Ok(fail(&e)),
    };
    let device = FileDeviceKey::new(dir.file("device.key"));
    let policy = KdfPolicy::production();
    match args.command.as_str() {
        "status" => {
            let s = status(&dir, args.port);
            println!("key directory : {}", dir.path().display());
            println!(
                "auth.json     : {}",
                match &s.auth {
                    Ok(Some(f)) => format!("{f:?}"),
                    Ok(None) => "absent".into(),
                    Err(e) => format!("unreadable ({})", e.category()),
                }
            );
            println!(
                "identity ({}) : {}",
                args.port,
                match &s.identity {
                    Ok(Some(f)) => format!("{f:?}"),
                    Ok(None) => "absent".into(),
                    Err(e) => format!("unreadable ({})", e.category()),
                }
            );
            if !s.other_identity_files.is_empty() {
                println!(
                    "other identity files: {}",
                    s.other_identity_files.join(", ")
                );
            }
            println!(
                "device key    : {}",
                match device.load() {
                    Ok(Some(_)) => "present".to_owned(),
                    Ok(None) => "absent".to_owned(),
                    Err(e) => format!("unreadable ({})", e.category()),
                }
            );
            println!(
                "device key protection when created here: {}",
                device.protection().describe()
            );
            Ok(0)
        }
        "migrate" => {
            let env_pw = std::env::var("YANDI_KEY_PASSWORD").ok();
            let secret: Zeroizing<String> = if !args.generate_code {
                let pw = ask(
                    "New master password, your own recovery secret (at least 12 characters): ",
                    args.stdin,
                    !args.show,
                )?;
                let again = ask("Repeat it: ", args.stdin, !args.show)?;
                if *pw != *again {
                    eprintln!("yandi-keys: the two passwords differ; nothing was changed");
                    return Ok(2);
                }
                pw
            } else {
                match confirmed_code(&args)? {
                    Some(code) => Zeroizing::new(code.secret().to_owned()),
                    None => return Ok(2),
                }
            };
            let m = Migration {
                dir: &dir,
                port: args.port,
                machine: machine.as_ref(),
                env_password: env_pw.as_deref(),
                device: &device,
                recovery_password: &secret,
                params: KdfParams::RECOMMENDED,
                policy: &policy,
            };
            match m.run() {
                Ok(r) => {
                    println!("migrated. node id {} is unchanged.", r.node_id);
                    println!(
                        "recovery wrapper written (Argon2id, {} MiB); device key {}.",
                        KdfParams::RECOMMENDED.memory_kib / 1024,
                        if r.device_key_created {
                            "created"
                        } else {
                            "reused"
                        }
                    );
                    println!("device key protection: {}", r.device_protection.describe());
                    for b in &r.backups {
                        println!("backup kept: {}", b.display());
                    }
                    println!("The backups are the OLD files, protected only by the public machine id. Keep them until you have restarted the node and checked it,");
                    println!("then move them somewhere safe or delete them yourself; this tool never deletes them.");
                    Ok(0)
                }
                Err(e) => Ok(fail(&e)),
            }
        }
        "new-recovery-code" => {
            let op = NewRecoveryCode {
                dir: &dir,
                machine: machine.as_ref(),
                device: &device,
                params: KdfParams::RECOMMENDED,
                policy: &policy,
            };
            let pending = match op.prepare() {
                Ok(p) => p,
                Err(e) => return Ok(fail(&e)),
            };
            show_code(pending.code());
            for attempt in 1..=3 {
                let typed = ask(
                    "Type the code back to confirm you have written it down: ",
                    args.stdin,
                    false,
                )?;
                match op.commit(&pending, &typed) {
                    Ok(backup) => {
                        println!("the recovery code was replaced. The previous recovery password (or code) NO LONGER WORKS.");
                        println!("the node id, the key and the chats are unchanged. previous auth.json kept as {}", backup.display());
                        return Ok(0);
                    }
                    Err(KeyRootError::Invalid(why)) => eprintln!("{why} ({attempt}/3)."),
                    Err(e) => return Ok(fail(&e)),
                }
            }
            eprintln!("yandi-keys: the code was not confirmed; nothing was changed");
            Ok(2)
        }
        "recover" => {
            let typed = ask(
                "Recovery secret (your master password, or a recovery code): ",
                args.stdin,
                !args.show,
            )?;
            if looks_like_code(&typed) {
                if let Err(e) = RecoveryCode::parse(&typed) {
                    eprintln!("yandi-keys: {e}. Nothing was changed.");
                    return Ok(2);
                }
            }
            let secret = key_root::recovery_secret(&typed);
            let r = Recovery {
                dir: &dir,
                port: args.port,
                machine: machine.as_ref(),
                device: &device,
                password: &secret,
                policy: &policy,
            };
            match r.run() {
                Ok(r) => {
                    println!("recovered. node id {} is the same as before.", r.node_id);
                    println!("a new device key was made for this machine ({}); the next start needs no code.", r.device_protection.describe());
                    for b in &r.backups {
                        println!("backup kept: {}", b.display());
                    }
                    Ok(0)
                }
                Err(e) => Ok(fail(&e)),
            }
        }
        "login-check" => {
            let password = read_line_stdin()?;
            match key_root::login::verify_login_in(&dir, &password) {
                Ok(true) => Ok(0),
                Ok(false) => {
                    eprintln!("yandi-keys: login_wrong");
                    Ok(1)
                }
                Err(message) => {
                    eprintln!("yandi-keys: {message}");
                    Ok(2)
                }
            }
        }
        "login-reset" => {
            let code = read_line_stdin()?;
            let new_password = read_line_stdin()?;
            match key_root::login::reset_login(&dir, &code, &new_password, &policy) {
                Ok(()) => Ok(0),
                Err(message) => {
                    eprintln!("yandi-keys: {message}");
                    Ok(1)
                }
            }
        }
        "setup" => {
            let login = read_line_stdin()?;
            let login_repeat = read_line_stdin()?;
            let master = read_line_stdin()?;
            let master_repeat = read_line_stdin()?;
            let made =
                key_root::login::check_setup_inputs(&login, &login_repeat, &master, &master_repeat)
                    .and_then(|_| {
                        key_root::login::create_keys(
                            &dir,
                            machine.as_ref(),
                            &device,
                            &login,
                            &master,
                            KdfParams::RECOMMENDED,
                            &policy,
                        )
                    });
            match made {
                Ok(_root) => Ok(0),
                Err(message) => {
                    eprintln!("yandi-keys: {message}");
                    Ok(1)
                }
            }
        }
        "core-key" => {
            // The key itself goes out here, so only into a pipe: a terminal keeps scrollback, screen recordings and shoulders.
            if unsafe { libc::isatty(1) } == 1 {
                eprintln!("yandi-keys: core-key prints a secret; pipe it to the program that needs it, never to a terminal");
                return Ok(2);
            }
            match unlock_root(&dir, machine.as_ref(), &device) {
                Ok((root, _)) => {
                    let key = derive_domain(&root, DOMAIN_CORE);
                    let text = Zeroizing::new(
                        base64::engine::general_purpose::STANDARD.encode(key.as_slice()),
                    );
                    println!("{}", text.as_str());
                    Ok(0)
                }
                Err(e) => Ok(fail(&e)),
            }
        }
        other => Err(format!("unknown command {other}")),
    }
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(msg) => {
            eprintln!("yandi-keys: {msg}");
            std::process::exit(2);
        }
    }
}
