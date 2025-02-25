#![no_std]
#![no_main]
#![feature(naked_functions)]

pub mod arch;
pub mod driver;
pub mod timer;
pub mod library;
pub mod mem;
pub mod traits;
pub mod sched;
pub mod earlycon;

use core::panic::PanicInfo;
use sched::scheduler::get_scheduler;
use arch::Arch;
use mem::allocator::init_heap;

/// A panic handler is required in Rust, this is probably the most basic one possible
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("[Scarlet Kernel] panic: {}", info);
    loop {}
}

#[unsafe(no_mangle)]
pub extern "C" fn start_kernel(cpu_id: usize) {
    early_println!("Hello, I'm Scarlet kernel!");
    early_println!("[Scarlet Kernel] Boot on CPU {}", cpu_id);
    early_println!("[Scarlet Kernel] Initializing arch...");
    let mut arch = Arch::new(cpu_id);
    arch.init(cpu_id);
    early_println!("[Scarlet Kernel] Initializing heap...");
    init_heap();
    /* After this point, we can use the heap */
    /* Serial console also works */
    println!("[Scarlet Kernel] Initializing scheduler...");
    let scheduler = get_scheduler();
    println!("[Scarlet Kernel] Scheduler will start...");
    scheduler.schedule();
    loop {} 
}

#[unsafe(no_mangle)]
pub extern "C" fn start_ap(cpu_id: usize) {
    println!("[Scarlet Kernel] CPU {} is up and running", cpu_id);
    println!("[Scarlet Kernel] Initializing arch...");
    let mut arch = Arch::new(cpu_id);
    arch.init(cpu_id);
    loop {}
}
