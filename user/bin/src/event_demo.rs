#![no_std]
#![no_main]

extern crate scarlet_std as std;

use core::sync::atomic::{AtomicBool, Ordering};

use std::ipc::{event_types, register_event_handler};
use std::println;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn interrupt_handler(_event_info: &std::ipc::EventInfo) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("Event demo: press Ctrl+C to interrupt");
    println!("PID = {}", std::task::getpid());

    // SAFETY: The static C-ABI handler only sets an atomic flag; it neither
    // retains EventInfo nor allocates, locks, or unwinds in event context.
    unsafe { register_event_handler(event_types::PROCESS_CONTROL, interrupt_handler, false) }
        .expect("Failed to register event handler");

    println!("Event handler registered. Waiting...");

    while !INTERRUPTED.load(Ordering::Relaxed) {
        core::hint::spin_loop();
    }
    println!("\nInterrupted!!");
    std::task::exit(130);
}
