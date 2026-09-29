#![no_std]
#![no_main]

extern crate scarlet_std as std;
use std::{format, println, string::ToString, vec::Vec};
use std::{handle::Handle, tty::Terminal};

#[unsafe(no_mangle)]
fn main() -> i32 {
    // Opening a TTY for stdin is not enough: descendants need both the
    // session identity and controlling-terminal association for tcsetpgrp.
    // The login supervisor owns the session while its shell runs.
    let stdin = match unsafe { Handle::from_raw(0) } {
        Ok(handle) => core::mem::ManuallyDrop::new(handle),
        Err(_) => {
            println!("login: missing terminal input");
            return 1;
        }
    };
    let terminal = Terminal::from_handle(&stdin);
    let pid = std::task::getpid();
    let session = match std::task::session_id(None) {
        Ok(session) if session == pid => session,
        _ => match std::task::create_session() {
            Ok(session) => session,
            Err(_) => {
                println!("login: failed to create terminal session");
                return 1;
            }
        },
    };
    if terminal.acquire_as_controlling(false).is_err() {
        println!("login: failed to acquire controlling terminal");
        return 1;
    }
    if terminal.set_foreground_group(session as usize).is_err() {
        let _ = terminal.detach_controlling();
        println!("login: failed to set terminal foreground group");
        return 1;
    }

    // TODO: Implement user authentication.
    std::env::set_var("USER", "root");
    std::env::set_var("HOME", "/root");
    std::env::set_var("SHELL", "/bin/sh");
    println!(
        "Login successful for user: {}",
        std::env::var("USER").unwrap_or("unknown".to_string())
    );

    let mut env = Vec::new();

    for (key, value) in std::env::vars() {
        env.push(format!("{}={}", key, value));
    }

    // Convert Vec<String> to Vec<&str> for execve
    let env: Vec<&str> = env.iter().map(|s| s.as_str()).collect();

    // Start the shell process
    match std::task::fork() {
        0 => {
            let shell_path = std::env::var("SHELL").unwrap_or("/bin/sh".to_string());
            let shell_name = format!("-{}", shell_path.rsplit('/').next().unwrap_or("sh"));
            // Child process: Execute the shell program
            if std::task::execve(&shell_path, &[&shell_name], &env) != 0 {
                println!("Failed to execve /bin/sh");
                return -1; // Exit with error}
            }
        }
        -1 => {
            println!("Failed to fork");
            let _ = terminal.detach_controlling();
            return -1; // Exit with error
        }
        pid => {
            let res = std::task::waitpid(pid, 0);
            println!(
                "Child process (PID={}) exited with status: {}",
                res.0, res.1
            );
            if res.1 != 0 {
                println!("Child process exited with error");
            }
        }
    }

    // Release session ownership so a subsequent login can acquire the TTY.
    let _ = terminal.detach_controlling();
    0
}
