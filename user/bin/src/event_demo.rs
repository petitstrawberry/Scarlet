#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::ipc::{event_types, register_event_handler};
use std::println;

extern "C" fn interrupt_handler(_event_info: &std::ipc::EventInfo) {
    println!("\nInterrupted!!");
    std::task::exit(130);
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("Event demo: press Ctrl+C to interrupt");
    println!("PID = {}", std::task::getpid());

    register_event_handler(event_types::PROCESS_CONTROL, interrupt_handler, false)
        .expect("Failed to register event handler");

    println!("Event handler registered. Waiting...");

    loop {
        core::hint::spin_loop();
    }
}
