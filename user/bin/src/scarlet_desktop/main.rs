//! Scarlet Desktop session launcher.
//!
//! This binary is responsible for starting the desktop components as separate
//! SWS clients (background, taskbar, etc.).

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::println;
use std::task::{EXECVE_FORCE_ABI_REBUILD, execve_with_flags, exit, fork, waitpid};

fn spawn_component(name: &str, args: &[&str]) -> i32 {
    match fork() {
        0 => {
            let candidates = [
                "/system/scarlet/bin",
                "/scarlet/system/scarlet/bin",
                "/old_root/system/scarlet/bin",
            ];

            for base in &candidates {
                let mut path_buf = std::string::String::new();
                path_buf.push_str(base);
                path_buf.push('/');
                path_buf.push_str(name);

                let argv0 = path_buf.as_str();
                let mut argv: std::vec::Vec<&str> = std::vec::Vec::new();
                argv.push(argv0);
                argv.extend_from_slice(args);

                // If exec succeeds, it never returns.
                let rc = execve_with_flags(argv0, &argv, &[], EXECVE_FORCE_ABI_REBUILD);
                if rc == 0 {
                    break;
                }
            }

            println!("[scarlet_desktop] Failed to exec '{}'", name);
            exit(127);
        }
        -1 => {
            println!("[scarlet_desktop] fork failed for '{}'", name);
            -1
        }
        pid => pid,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[scarlet_desktop] Starting desktop session");

    let bg_pid = spawn_component("scarlet_desktop_background", &[]);
    if bg_pid > 0 {
        println!("[scarlet_desktop] background pid={}", bg_pid);
    }

    let taskbar_pid = spawn_component("scarlet_desktop_taskbar", &[]);
    if taskbar_pid > 0 {
        println!("[scarlet_desktop] taskbar pid={}", taskbar_pid);
    }

    // Session manager: wait for one of the components to exit.
    // (Keeps the session alive and reaps children.)
    loop {
        let (pid, status) = waitpid(-1, 0);
        if pid < 0 {
            continue;
        }
        println!(
            "[scarlet_desktop] child exited pid={} status={}",
            pid, status
        );

        // If a core component exits, terminate the session.
        if pid == bg_pid || pid == taskbar_pid {
            println!("[scarlet_desktop] core component exited; terminating session");
            return 0;
        }
    }
}
