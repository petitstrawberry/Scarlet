#![no_std]
#![no_main]
#![feature(naked_functions)]

#![feature(custom_test_frameworks)]
#![test_runner(crate::test::test_runner)]
#![reexport_test_harness_main = "test_main"]

pub mod arch;
pub mod driver;
pub mod timer;
pub mod library;
pub mod mem;
pub mod traits;
pub mod sched;
pub mod earlycon;
pub mod environment;
pub mod vm;
pub mod task;
#[cfg(test)]
pub mod test;


use core::panic::PanicInfo;

use arch::Arch;
use vm::kernel_vm_init;
use sched::scheduler::get_scheduler;
use mem::allocator::init_heap;
use timer::get_kernel_timer;


/// A panic handler is required in Rust, this is probably the most basic one possible
#[cfg(not(test))]
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

    #[cfg(test)]
    test_main();

    println!("[Scarlet Kernel] Initializing Virtual Memory...");
    kernel_vm_init(); /* After this point, the kernel is running in virtual memory */
    println!("[Scarlet Kernel] Initializing timer...");
    get_kernel_timer().init();
    println!("[Scarlet Kernel] Initializing scheduler...");
    let scheduler = get_scheduler();
    scheduler.init_test_tasks();
    println!("[Scarlet Kernel] Scheduler will start...");
    scheduler.kernel_schedule(cpu_id);
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
