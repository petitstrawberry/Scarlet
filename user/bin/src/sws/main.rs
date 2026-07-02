//! Scarlet Window Server (SWS)
//!
//! A compositing window server for Scarlet OS

#![no_std]
#![no_main]

extern crate scarlet_std as std;

mod compositor;
mod cursor;
mod input;
mod ipc;
mod window;

use compositor::Compositor;
use core::time::Duration;
use sbus_client as sbus;
use std::io::Write;
use std::println;
use std::socket::Socket;
use std::task::{SCHED_UTIL_SCALE, set_sched_util_min};

const STEMD_SERVICE_READY_CMD: u8 = 0x06;
const STEMD_NOTIFY_RETRIES: usize = 100;
const STEMD_NOTIFY_DELAY_MS: u64 = 50;

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

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("=== Scarlet Window Server (SWS) ===");
    println!("Initializing compositor...");

    // Initialize compositor
    let mut compositor = match Compositor::new() {
        Ok(comp) => comp,
        Err(e) => {
            println!("Failed to initialize compositor: {}", e);
            return 1;
        }
    };

    // Initialize display
    if let Err(e) = compositor.init_display() {
        println!("Failed to initialize display: {}", e);
        return 1;
    }

    // Register with sbus
    println!("Registering with sbus...");
    match sbus::Connection::connect() {
        Ok(mut conn) => {
            if let Err(e) = conn.register_service("org.scarlet-os.sws") {
                println!("Failed to register with sbus: {:?}", e);
            } else {
                println!("Successfully registered with sbus as org.scarlet-os.sws");
            }
        }
        Err(e) => {
            println!("Failed to connect to sbus: {:?}", e);
            println!("Continuing without sbus registration");
        }
    }

    println!("Compositor ready. Starting main loop...");
    notify_service_ready("sws");

    if set_sched_util_min(SCHED_UTIL_SCALE).is_err() {
        println!("[sws] Failed to set compositor scheduler utilization hint");
    }

    // Run main loop
    if let Err(e) = compositor.run() {
        println!("Compositor error: {}", e);
        return 1;
    }

    0
}
