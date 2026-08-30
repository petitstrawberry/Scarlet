//! Scarlet Window Server (SWS)
//!
//! A compositing window server for Scarlet OS

mod compositor;
mod config;
mod cursor;
mod cursor_theme;
mod damage;
mod gpu_compositor;
mod input;
mod ipc;
mod pointer_lock;
mod remote;
#[path = "../sgfx_ir_support.rs"]
mod sgfx_ir_support;
mod trace;
mod window;

use compositor::Compositor;
use core::time::Duration;
use sbus_client as sbus;
use scarlet_os::scheduler;
use scarlet_os::socket::Socket;
use scarlet_sys::SCHED_UTIL_SCALE;
use std::io::Write;
use std::println;
use std::process::ExitCode;

const STEMD_SERVICE_READY_CMD: u8 = 0x06;
const STEMD_NOTIFY_RETRIES: usize = 100;
const STEMD_NOTIFY_DELAY_MS: u64 = 50;
const SBUS_REGISTRATION_TIMEOUT_MS: u64 = 1_000;
const SWS_COMPOSITOR_UTIL_MIN: u32 = SCHED_UTIL_SCALE * 7 / 8;

fn notify_service_ready(service_name: &str) {
    for attempt in 0..STEMD_NOTIFY_RETRIES {
        if let Ok(mut stream) = Socket::new()
            && stream.connect("/tmp/stemd.sock").is_ok()
        {
            let mut payload = std::vec::Vec::new();
            payload.push(STEMD_SERVICE_READY_CMD);
            payload.extend_from_slice(&(service_name.len() as u32).to_le_bytes());
            payload.extend_from_slice(service_name.as_bytes());

            if stream.write(&payload).is_ok() {
                println!(
                    "[sws] Reported ready to stemd after {} attempt(s)",
                    attempt + 1
                );
                return;
            }
        }

        std::thread::sleep(Duration::from_millis(STEMD_NOTIFY_DELAY_MS));
    }

    println!("[sws] Failed to report ready to stemd after retries");
}

fn main() -> ExitCode {
    println!("=== Scarlet Window Server (SWS) ===");
    println!("Initializing compositor...");

    // Initialize compositor
    let mut compositor = match Compositor::new() {
        Ok(comp) => comp,
        Err(e) => {
            println!("Failed to initialize compositor: {}", e);
            return ExitCode::FAILURE;
        }
    };

    // Initialize display
    if let Err(e) = compositor.init_display() {
        println!("Failed to initialize display: {}", e);
        return ExitCode::FAILURE;
    }

    // Sbus integration is optional, so its acknowledgment wait must remain
    // well inside stemd's service-readiness deadline. Keep a successful
    // connection alive for the compositor lifetime; dropping it immediately
    // would unregister the service again.
    println!("Registering with sbus...");
    let _sbus_connection = match sbus::Connection::connect() {
        Ok(mut conn) => {
            match conn.register_service_timeout("org.scarlet-os.sws", SBUS_REGISTRATION_TIMEOUT_MS)
            {
                Ok(()) => {
                    println!("Successfully registered with sbus as org.scarlet-os.sws");
                    Some(conn)
                }
                Err(e) => {
                    println!("Failed to register with sbus: {:?}", e);
                    None
                }
            }
        }
        Err(e) => {
            println!("Failed to connect to sbus: {:?}", e);
            println!("Continuing without sbus registration");
            None
        }
    };

    println!("Compositor ready. Starting main loop...");
    notify_service_ready("sws");
    trace::start_watchdog();

    let scheduler_hint_result = scheduler::configured().and_then(|mut configured| {
        configured.set_fair_util_min(SWS_COMPOSITOR_UTIL_MIN)?;
        configured.apply()
    });
    if scheduler_hint_result.is_err() {
        println!("[sws] Failed to set compositor scheduler utilization hint");
    }

    // Run main loop
    if let Err(e) = compositor.run() {
        println!("Compositor error: {}", e);
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
