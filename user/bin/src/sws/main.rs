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
use sbus_client as sbus;
use std::println;

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

    // Run main loop
    if let Err(e) = compositor.run() {
        println!("Compositor error: {}", e);
        return 1;
    }

    0
}
