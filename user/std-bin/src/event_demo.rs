use core::sync::atomic::{AtomicBool, Ordering};

use scarlet_os::ipc::{EventInfo, event_types, register_event_handler};
use std::process::{exit, id};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn interrupt_handler(_event_info: &EventInfo) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

fn main() -> ! {
    println!("Event demo: press Ctrl+C to interrupt");
    println!("PID = {}", id());

    // SAFETY: The static C-ABI handler only sets an atomic flag; it neither
    // retains EventInfo nor allocates, locks, or unwinds in event context.
    unsafe { register_event_handler(event_types::PROCESS_CONTROL, interrupt_handler, false) }
        .expect("Failed to register event handler");

    println!("Event handler registered. Waiting...");

    while !INTERRUPTED.load(Ordering::Relaxed) {
        core::hint::spin_loop();
    }
    println!("\nInterrupted!!");
    exit(130);
}
