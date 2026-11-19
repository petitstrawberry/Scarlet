//! Local-seat session supervisor. The seat owns the private stdio
//! channel. Each input byte produces a 37-byte reply: active flag followed
//! by the greeter's 36-byte display state. No password bytes leave this process.
#![no_std]
#![no_main]

extern crate scarlet_std as std;
use core::time::Duration;
use scarlet_accounts::{Account, authenticate_local};
use scarlet_os::process_scope::ProcessScope;
use std::{format, fs::File, handle::Handle, task};
use zeroize::{Zeroize, Zeroizing};

struct Desktop {
    owner: Account,
    scope: ProcessScope,
    pid: i32,
}

impl Desktop {
    fn start(owner: Account, argv: &[&str]) -> Result<Self, ()> {
        let scope = ProcessScope::create().map_err(|_| ())?;
        let pid = task::fork();
        if pid < 0 {
            return Err(());
        }
        if pid == 0 {
            // Stdio was the private supervisor channel. Replace all three
            // descriptors before entering the user scope or executing desktop code.
            let Ok(null) = File::open_with_flags("/dev/null", 2) else {
                task::exit(126)
            };
            let null = null.into_handle();
            for fd in 0..3 {
                let Ok(handle) = (unsafe { Handle::from_raw(fd) }) else {
                    task::exit(126)
                };
                let _ = handle.close();
                match null.duplicate() {
                    Ok(handle) if handle.as_raw() == fd => core::mem::forget(handle),
                    _ => task::exit(126),
                }
            }
            drop(null);
            // SAFETY: this exclusive child owns no other handles and exits on failure.
            if unsafe { owner.enter(scope) }.is_err() {
                task::exit(126);
            }
            let home = owner.home();
            if std::fs::change_directory(&home).is_err() {
                task::exit(126);
            }
            let env = [
                format!("USER={}", owner.name),
                format!("LOGNAME={}", owner.name),
                format!("HOME={home}"),
            ];

            task::execve(
                argv[0],
                argv,
                &[
                    &env[0],
                    &env[1],
                    &env[2],
                    "PATH=/bin:/usr/bin",
                    "SHELL=/bin/sh",
                ],
            );
            task::exit(127);
        }
        let mut desktop = Self { owner, scope, pid };
        for _ in 0..200 {
            if desktop.exited() {
                break;
            }
            if desktop.scope.status().is_ok_and(|status| status.live > 0) {
                return Ok(desktop);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(())
    }

    fn exited(&mut self) -> bool {
        if self.pid > 0 && task::waitpid(self.pid, task::WAIT_NOHANG).0 == 0 {
            return false;
        }
        self.pid = -1;
        true
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        // No later user may occupy the seat until the old descendants are gone.
        while self.scope.terminate().is_err() || !self.scope.status().is_ok_and(|s| s.live == 0) {
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.pid > 0 {
            task::waitpid(self.pid, 0);
        }
    }
}

#[derive(Default)]
struct Seat {
    desktop: Option<Desktop>,
    authenticated: Option<Account>,
    active: bool,
    choosing: bool,
    multiple: bool,
    password_field: bool,
    failed: bool,
    unavailable: bool,
    name_overflow: usize,
    password_overflow: usize,
    name: std::vec::Vec<u8>,
    password: Zeroizing<std::vec::Vec<u8>>,
}

impl Seat {
    fn reset(&mut self) {
        self.desktop.take();
        *self = Self::default();
    }

    fn choose_account(&mut self, step: i8) {
        let names = scarlet_accounts::login_names();
        let index = names.iter().position(|s| s.as_bytes() == self.name);
        self.clear_entry();
        self.unavailable = names.is_empty();
        self.choosing = !self.unavailable;
        self.multiple = names.len() > 1;
        if self.choosing {
            let index = index.map_or(0, |i| {
                (i as isize + step as isize).rem_euclid(names.len() as isize) as usize
            });
            self.name.extend_from_slice(names[index].as_bytes());
        }
    }

    fn clear_entry(&mut self) {
        self.name.clear();
        self.name_overflow = 0;
        self.password.zeroize();
        self.password_overflow = 0;
        self.password_field = false;
        self.failed = false;
        self.choosing = false;
    }

    fn submit(&mut self) {
        if !self.password_field && self.desktop.is_none() {
            self.password_field = !self.name.is_empty() && self.name_overflow == 0;
            return;
        }
        let account = if self.name_overflow == 0 && self.password_overflow == 0 {
            authenticate_local(&self.name, &self.password)
                .ok()
                .flatten()
        } else {
            None
        };
        self.password_overflow = 0;
        self.password.zeroize();
        match (account, self.desktop.as_ref()) {
            (Some(user), Some(session))
                if session.owner.uid == user.uid && session.owner.name == user.name =>
            {
                self.active = true
            }
            (Some(user), None) => {
                self.authenticated = Some(user);
            }
            _ => {}
        }
        self.failed = !self.active && self.authenticated.is_none();
        if self.failed {
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    fn event(&mut self, byte: u8, argv: &[&str]) {
        if self.desktop.as_mut().is_some_and(Desktop::exited) {
            self.reset();
        }
        if self.desktop.is_none()
            && self.authenticated.is_none()
            && (byte == 19 || (self.unavailable && byte == b'\n'))
        {
            self.choose_account(0);
            return;
        }
        if self.desktop.is_none() && (byte == 0 || byte == b'\n') {
            let was_unavailable = self.unavailable;
            self.unavailable = !scarlet_accounts::login_available();
            if self.unavailable || was_unavailable {
                self.clear_entry();
                return;
            }
        }
        if self.unavailable {
            return;
        }
        if let Some(user) = self.authenticated.take() {
            if byte == 16 {
                self.desktop = Desktop::start(user, argv).ok();
                self.active = self.desktop.is_some();
                self.failed = !self.active;
            }
            return;
        }
        // NUL polls state. SO/SI request lock/logout for an active session.
        if self.active {
            match byte {
                14 => {
                    self.active = false;
                    self.password_field = true;
                }
                15 => self.reset(),
                _ => {}
            }
            return;
        }
        if self.choosing {
            match byte {
                20 | 21 | 9 => {
                    self.choose_account(if byte == 20 { -1 } else { 1 });
                    return;
                }
                b'\n' => {
                    self.choosing = false;
                    self.password_field = true;
                    return;
                }
                22 | 32..=126 => self.clear_entry(),
                _ => return,
            }
        }
        match byte {
            b'\n' => self.submit(),
            // Trusted seat focus controls. Selecting a field never submits.
            // Keep separate overflow counters so switching cannot admit a
            // silently truncated name or password.
            17 if self.desktop.is_none() => self.password_field = false,
            18 if self.desktop.is_none() && !self.name.is_empty() && self.name_overflow == 0 => {
                self.password_field = true
            }
            9 if self.desktop.is_none() => {
                self.password_field =
                    !self.password_field && !self.name.is_empty() && self.name_overflow == 0;
            }
            27 if self.desktop.is_none() => {
                self.password_field = false;
                self.password_overflow = 0;
                self.failed = false;
                self.password.zeroize();
            }
            8 if self.password_field && self.password_overflow > 0 => self.password_overflow -= 1,
            8 if !self.password_field && self.name_overflow > 0 => self.name_overflow -= 1,
            8 if self.password_field => {
                if let Some(last) = self.password.last_mut() {
                    last.zeroize();
                }
                self.password.pop();
            }
            8 => {
                self.name.pop();
            }
            32..=255 if self.password_field => {
                if self.password.capacity() == 0 {
                    self.password.reserve_exact(72);
                }
                if self.password.len() < 72 {
                    self.password.push(byte);
                } else {
                    self.password_overflow = self.password_overflow.saturating_add(1);
                }
            }
            b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => {
                if self.name.len() < 32 {
                    self.name.push(byte);
                } else {
                    self.name_overflow = self.name_overflow.saturating_add(1);
                }
            }
            _ => {}
        }
    }

    fn response(&self) -> [u8; 37] {
        let mut reply = [0; 37];
        reply[0] = if self.authenticated.is_some() {
            2
        } else {
            u8::from(self.active)
        };
        reply[1] = if self.desktop.is_some() {
            2
        } else if self.choosing {
            if self.multiple { 4 } else { 3 }
        } else {
            u8::from(self.password_field)
        };
        reply[2] = if self.unavailable {
            2
        } else {
            u8::from(self.failed)
        };
        reply[3] = self.password.len().min(72) as u8;
        reply[4] = self.name.len() as u8;
        reply[5..5 + self.name.len()].copy_from_slice(&self.name);
        reply
    }
}

fn exec_anonymous(program: &str, argv: &[&str], env: &[&str]) -> i32 {
    // SAFETY: this exclusive launcher owns only the stdio descriptors. The
    // transition clears all grants and discards any extra inherited handles.
    if unsafe { scarlet_os::shield::enter_user_session(u32::MAX, u32::MAX, &[]) }.is_ok() {
        std::task::execve(program, argv, env);
    }
    1
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args = std::env::args_vec();
    if args.len() == 2 && args[1] == "--login-panel" {
        let panel = "/bin/soft-keyboard";
        return exec_anonymous(panel, &[panel, "--login-seat"], &["SCARLET_UI_BACKEND=cpu"]);
    }
    let Some((program, rest)) = args.get(2..).and_then(|args| args.split_first()) else {
        return 2;
    };
    if !program.starts_with('/') {
        return 2;
    }
    match args[1].as_str() {
        "--render-greeter" if rest.is_empty() => {
            return exec_anonymous(program, &[program], &[]);
        }
        "--session" => {}
        _ => return 2,
    }
    let argv: std::vec::Vec<&str> = args[2..].iter().map(|s| s.as_str()).collect();
    let mut seat = Seat::default();
    let mut input = Zeroizing::new([0u8; 1]);
    while let Ok(1) = std::io::stdin().read(&mut *input) {
        seat.event(input[0], &argv);
        input.zeroize();
        if std::io::stdout().write_all(&seat.response()).is_err() {
            return 1;
        }
    }
    0
}
