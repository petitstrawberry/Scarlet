#![no_std]
#![no_main]

extern crate scarlet_std as std;

use scarlet_accounts::{Account, authenticate_local};
use scarlet_os::process_scope::ProcessScope;
use std::{format, handle::Handle, println, tty::Terminal, vec::Vec};
use zeroize::Zeroizing;

fn line(limit: usize) -> Result<Zeroizing<Vec<u8>>, &'static str> {
    let mut input = Zeroizing::new(Vec::with_capacity(limit + 1));
    let mut chunk = Zeroizing::new([0u8; 96]);
    loop {
        let n = std::io::stdin()
            .read(&mut *chunk)
            .map_err(|_| "terminal read failed")?;
        if n == 0 {
            return Err("terminal closed");
        }
        let end = chunk[..n].ends_with(b"\n");
        let bytes = &chunk[..n - usize::from(end)];
        // Drain the whole canonical record. Keep one extra byte to reject an
        // overlong name or password without accepting its truncated prefix.
        let take = (limit + 1 - input.len()).min(bytes.len());
        input.extend_from_slice(&bytes[..take]);
        if end {
            return Ok(input);
        }
    }
}

fn authenticate(tty: &Terminal) -> Result<Account, &'static str> {
    loop {
        std::print!("login: ");
        let name = line(32)?;
        tty.set_echo(false)
            .map_err(|_| "cannot hide password input")?;
        std::print!("Password: ");
        let password = line(72);
        tty.set_echo(true)
            .map_err(|_| "cannot restore terminal echo")?;
        println!();
        let password = password?;
        let account = authenticate_local(&name, &password)?;
        if let Some(account) = account {
            return Ok(account);
        }
        println!("Login incorrect");
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn start_shell(account: &Account, scope: ProcessScope) -> Result<(), &'static str> {
    scope.enter().map_err(|_| "cannot join session scope")?;
    drop(scope);
    // SAFETY: the child has relinquished the scope handle. The account DB and
    // password buffers were dropped before fork; stdio are the only handles in use.
    unsafe { scarlet_os::shield::enter_user_session(account.uid, account.gid, &account.groups) }
        .map_err(|_| "cannot install session credentials")?;
    let home = format!("/home/{}", account.name);
    std::fs::change_directory(&home).map_err(|_| "cannot enter home directory")?;
    let user_env = format!("USER={}", account.name);
    let logname = format!("LOGNAME={}", account.name);
    let home_env = format!("HOME={home}");
    let prompt = format!("PS1={}$ ", account.name);
    std::task::execve(
        "/bin/sh",
        &["-sh"],
        &[
            &user_env,
            &logname,
            &home_env,
            &prompt,
            "SHELL=/bin/sh",
            "PATH=/bin:/usr/bin",
        ],
    );
    Err("cannot execute /bin/sh")
}

fn supervise(tty: &Terminal, session: usize) -> Result<(), &'static str> {
    loop {
        tty.set_foreground_group(session)
            .map_err(|_| "cannot set foreground group")?;
        tty.set_keyboard_mode(std::tty::KeyboardMode::Xlate)
            .map_err(|_| "cannot restore keyboard mode")?;
        tty.set_nonblocking(false)
            .map_err(|_| "cannot restore blocking input")?;
        tty.set_canonical(true)
            .map_err(|_| "cannot enable line input")?;
        tty.set_crnl_input_enabled(true)
            .map_err(|_| "cannot enable newline translation")?;
        tty.set_debug_enabled(false)
            .map_err(|_| "cannot disable input tracing")?;
        tty.set_echo(true)
            .map_err(|_| "cannot enable terminal echo")?;
        tty.flush_input()
            .map_err(|_| "cannot clear previous session input")?;
        let account = authenticate(tty)?;
        let scope = ProcessScope::create().map_err(|_| "cannot create session scope")?;
        match std::task::fork() {
            -1 => return Err("cannot fork shell"),
            0 => {
                if let Err(error) = start_shell(&account, scope) {
                    println!("login: {}", error);
                }
                std::task::exit(1);
            }
            pid => {
                let (finished, _) = std::task::waitpid(pid, 0);
                scope
                    .terminate()
                    .map_err(|_| "cannot close session scope")?;
                while scope
                    .status()
                    .map_err(|_| "cannot inspect session scope")?
                    .live
                    != 0
                {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                if finished != pid {
                    return Err("cannot wait for shell");
                }
            }
        }
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    // SAFETY: borrow stdin for terminal operations without taking ownership.
    let Ok(stdin) = (unsafe { Handle::from_raw(0) }) else {
        return 1;
    };
    let stdin = core::mem::ManuallyDrop::new(stdin);
    let tty = Terminal::from_handle(&stdin);
    let pid = std::task::getpid();
    let Ok(session) = std::task::session_id(None).and_then(|sid| {
        if sid == pid {
            Ok(sid)
        } else {
            std::task::create_session()
        }
    }) else {
        println!("login: cannot create terminal session");
        return 1;
    };
    if tty.acquire_as_controlling(false).is_err() {
        println!("login: cannot acquire controlling terminal");
        return 1;
    }
    let result = supervise(&tty, session as usize);
    let _ = tty.detach_controlling();
    if let Err(error) = result {
        println!("login: {}", error);
    }
    1
}
