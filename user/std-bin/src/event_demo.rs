use scarlet_os::ipc::{EventInfo, event_types, register_event_handler};
use std::process::{exit, id};

extern "C" fn interrupt_handler(_event_info: &EventInfo) {
    println!("\nInterrupted!!");
    exit(130);
}

fn main() -> ! {
    println!("Event demo: press Ctrl+C to interrupt");
    println!("PID = {}", id());

    register_event_handler(event_types::PROCESS_CONTROL, interrupt_handler, false)
        .expect("Failed to register event handler");

    println!("Event handler registered. Waiting...");

    loop {
        core::hint::spin_loop();
    }
}
