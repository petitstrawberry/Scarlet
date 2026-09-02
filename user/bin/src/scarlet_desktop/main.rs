//! Scarlet Desktop session launcher.
//!
//! This binary supervises the workspace shell and desktop settings service.

#![no_std]
#![no_main]

extern crate scarlet_std as std;
extern crate sws_client;

use core::time::Duration;
use std::println;
use std::task::{EXECVE_FORCE_ABI_REBUILD, execve_with_flags, exit, fork, waitpid};
use std::thread;

const SWS_READY_RETRIES: usize = 100;
const SWS_READY_RETRY_DELAY_MS: u64 = 50;
const COMPONENT_RESPAWN_DELAY_MS: u64 = 100;

fn wait_for_sws_ready() -> bool {
    for attempt in 0..SWS_READY_RETRIES {
        if let Ok(conn) = sws_client::Connection::connect("/tmp/sws.sock")
            && conn.get_screen_size().is_ok()
        {
            println!(
                "[scarlet-desktop] SWS ready after {} attempt(s)",
                attempt + 1
            );
            return true;
        }

        thread::sleep(Duration::from_millis(SWS_READY_RETRY_DELAY_MS));
    }

    println!(
        "[scarlet-desktop] SWS was not ready after {} attempts; continuing anyway",
        SWS_READY_RETRIES
    );
    false
}

fn spawn_component(name: &str, args: &[&str]) -> i32 {
    match fork() {
        0 => {
            let candidates = [
                "/bin",
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

            println!("[scarlet-desktop] Failed to exec '{}'", name);
            exit(127);
        }
        -1 => {
            println!("[scarlet-desktop] fork failed for '{}'", name);
            -1
        }
        pid => pid,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[scarlet-desktop] Starting desktop session");
    wait_for_sws_ready();

    let mut settingsd_pid = spawn_component("desktop-settings", &[]);
    if settingsd_pid > 0 {
        println!("[scarlet-desktop] settingsd pid={}", settingsd_pid);
    }

    let mut shell_pid = spawn_component("scarlet-shell", &[]);
    if shell_pid > 0 {
        println!("[scarlet-desktop] shell pid={}", shell_pid);
    }

    // Session manager: supervise core components and respawn them if they exit.
    loop {
        let (pid, status) = waitpid(-1, 0);
        if pid < 0 {
            continue;
        }
        println!(
            "[scarlet-desktop] child exited pid={} status={}",
            pid, status
        );

        if pid == shell_pid {
            println!("[scarlet-desktop] shell exited; respawning");
            thread::sleep(Duration::from_millis(COMPONENT_RESPAWN_DELAY_MS));
            wait_for_sws_ready();
            shell_pid = spawn_component("scarlet-shell", &[]);
            if shell_pid > 0 {
                println!("[scarlet-desktop] shell respawned pid={}", shell_pid);
            }
            continue;
        }

        if pid == settingsd_pid {
            println!("[scarlet-desktop] settingsd exited; respawning");
            thread::sleep(Duration::from_millis(COMPONENT_RESPAWN_DELAY_MS));
            settingsd_pid = spawn_component("desktop-settings", &[]);
            if settingsd_pid > 0 {
                println!(
                    "[scarlet-desktop] settingsd respawned pid={}",
                    settingsd_pid
                );
            }
            continue;
        }
    }
}
