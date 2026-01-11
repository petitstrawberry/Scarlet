//! Slint Demo Application for Scarlet OS
//!
//! This demonstrates a real Slint application running on Scarlet Window Server.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::println;

// We'll use the slint! macro to embed UI
slint::slint! {
    export component MainWindow inherits Window {
        width: 600px;
        height: 400px;
        
        VerticalBox {
            padding: 20px;
            spacing: 16px;
            
            Text {
                text: "Hello from Slint on Scarlet!";
                font-size: 28px;
                horizontal-alignment: center;
            }
            
            Text {
                text: "This is a real Slint application";
                font-size: 14px;
                horizontal-alignment: center;
            }
            
            Rectangle {
                height: 2px;
                background: #cccccc;
            }
            
            HorizontalBox {
                spacing: 12px;
                
                Button {
                    text: "Click Me!";
                    clicked => {
                        debug("Button clicked from Slint!");
                    }
                }
                
                Button {
                    text: "Another Button";
                }
            }
        }
    }
}

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
    let window = match MainWindow::new() {
        Ok(w) => w,
        Err(e) => {
            println!("[slint_demo] Failed to create window: {:?}", e);
            return 1;
        }
    };
    
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
