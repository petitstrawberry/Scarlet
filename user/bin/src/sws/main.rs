//! Scarlet Window Server (SWS)
//!
//! A compositing window server for Scarlet OS

#![no_std]
#![no_main]

extern crate scarlet_std as std;

mod compositor;
mod cursor;
mod input;
mod window;

use compositor::Compositor;
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

    println!("Compositor ready. Starting main loop...");

    // Run main loop
    if let Err(e) = compositor.run() {
        println!("Compositor error: {}", e);
        return 1;
    }

    0
}
