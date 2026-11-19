//! Local-seat session supervisor. The compositor owns the private stdio
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
    password_field: bool,
    failed: bool,
    unavailable: bool,
    overflow: usize,
    name: std::vec::Vec<u8>,
    password: Zeroizing<std::vec::Vec<u8>>,
}

impl Seat {
    fn reset(&mut self) {
        self.desktop.take();
        *self = Self::default();
    }

    fn submit(&mut self) {
        if !self.password_field && self.desktop.is_none() {
            self.password_field = !self.name.is_empty() && self.overflow == 0;
            return;
        }
        let account = if self.overflow == 0 {
            authenticate_local(&self.name, &self.password)
                .ok()
                .flatten()
        } else {
            None
        };
        self.overflow = 0;
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
        if self.desktop.is_none() && (byte == 0 || byte == b'\n') {
            let was_unavailable = self.unavailable;
            self.unavailable = !scarlet_accounts::login_available();
            if self.unavailable || was_unavailable {
                self.name.clear();
                self.password.zeroize();
                self.overflow = 0;
                self.password_field = false;
                self.failed = false;
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
        match byte {
            b'\n' => self.submit(),
            27 if self.desktop.is_none() => {
                self.password_field = false;
                self.overflow = 0;
                self.failed = false;
                self.password.zeroize();
            }
            8 if self.overflow > 0 => self.overflow -= 1,
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
                    self.overflow = self.overflow.saturating_add(1);
                }
            }
            b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => {
                if self.name.len() < 32 {
                    self.name.push(byte);
                } else {
                    self.overflow = self.overflow.saturating_add(1);
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

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args = std::env::args_vec();
    let Some((program, rest)) = args.get(2..).and_then(|args| args.split_first()) else {
        return 2;
    };
    if !program.starts_with('/') {
        return 2;
    }
    match args[1].as_str() {
        "--render-greeter" if rest.is_empty() => {
            // No configured login may use the anonymous renderer identity.
            if unsafe { scarlet_os::shield::enter_user_session(u32::MAX, u32::MAX, &[]) }.is_err() {
                return 1;
            }
            std::task::execve(program, &[program], &[]);
            return 1;
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
