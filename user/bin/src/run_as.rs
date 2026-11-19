//! Install a system service's configured identity in its exclusive exec child.
#![no_std]
#![no_main]
extern crate scarlet_std as std;
use std::{env, format, task, vec::Vec};
#[unsafe(no_mangle)]
fn main() -> i32 {
    let args = env::args_vec();
    let [_, name, program, ..] = args.as_slice() else {
        return 2;
    };
    if !program.starts_with('/') {
        return 2;
    }
    let Some(account) = scarlet_accounts::service_account(name.as_bytes()) else {
        return 126;
    };
    let home = account.home();
    // SAFETY: startup is single threaded. Account lookup released its handles.
    if unsafe { scarlet_os::shield::enter_user_session(account.uid, account.gid, &account.groups) }
        .is_err()
    {
        return 126;
    }
    if std::fs::change_directory(&home).is_err() {
        return 126;
    }
    let argv: Vec<&str> = args[2..].iter().map(|s| s.as_str()).collect();
    task::execve(
        program,
        &argv,
        &[
            &format!("USER={name}"),
            &format!("LOGNAME={name}"),
            &format!("HOME={home}"),
            "PATH=/bin:/usr/bin",
            "SHELL=/bin/sh",
        ],
    );
    127
}
