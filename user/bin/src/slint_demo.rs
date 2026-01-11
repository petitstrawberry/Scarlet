//! Slint Demo Application for Scarlet OS
//!
//! This demonstrates a real Slint application running on Scarlet Window Server.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::println;

slint::include_modules!();

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[slint_demo] Starting Slint application on Scarlet");

    // Initialize the Slint-Scarlet backend
    match slint_scarlet::init() {
        Ok(_) => println!("[slint_demo] Slint backend initialized"),
        Err(e) => {
            println!("[slint_demo] Failed to initialize Slint backend: {:?}", e);
            return 1;
        }
    }

    // Create the main window
    println!("[slint_demo] Creating MainWindow...");
    let window = match MainWindow::new() {
        Ok(w) => w,
        Err(e) => {
            println!("[slint_demo] Failed to create window: {:?}", e);
            return 1;
        }
    };
    println!("[slint_demo] MainWindow created");

    window.on_click_me(|| {
        println!("[slint_demo] Click Me pressed");
    });

    println!("[slint_demo] Window created, starting event loop");

    // Run the application
    match window.run() {
        Ok(_) => {
            println!("[slint_demo] Application exited normally");
            0
        }
        Err(e) => {
            println!("[slint_demo] Application error: {:?}", e);
            1
        }
    }
}
