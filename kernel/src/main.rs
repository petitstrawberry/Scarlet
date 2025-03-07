//! # Scarlet Kernel
//!
//! The Scarlet Kernel is a bare metal, `no_std` operating system kernel.
//!
//! ## Core Features
//!
//! - Runs without standard library support (`no_std`)
//! - Custom entry points and initialization sequence
//! - Architecture-specific abstractions
//! - Memory management with heap allocation
//! - Virtual memory support
//! - Task scheduling
//! - Early console for boot-time logging
//! - Timer and driver subsystems
//!
//! ## Boot Process
//!
//! The kernel has two main entry points:
//! - `start_kernel`: Main boot entry point for the bootstrap processor
//! - `start_ap`: Entry point for application processors (APs) in multicore systems
//!
//! The initialization sequence for the bootstrap processor includes:
//! 1. Architecture-specific initialization
//! 2. Heap initialization
//! 3. Virtual memory setup
//! 4. Timer initialization
//! 5. Scheduler initialization and task creation
//! 6. Task scheduling
//!
//! ## Modules
//!
//! - `arch`: Architecture-specific code
//! - `driver`: Device drivers
//! - `timer`: System timing facilities
//! - `library`: General utilities and common functions
//! - `mem`: Memory management subsystems
//! - `traits`: Common traits used across the kernel
//! - `sched`: Task scheduling
//! - `earlycon`: Early boot console output
//! - `environment`: Environment settings and parameters
//! - `vm`: Virtual memory management
//! - `task`: Task abstractions
//! - `test`: Testing framework (only in test builds)
//!
//! ## Development Notes
//!
//! The kernel uses Rust's advanced features like naked functions and custom test frameworks.
//! In non-test builds, a simple panic handler is provided that prints the panic information 
//! and enters an infinite loop.

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

use arch::arch_init;
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
    arch_init(cpu_id);
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
    scheduler.start_scheduler();
    loop {} 
}

#[unsafe(no_mangle)]
pub extern "C" fn start_ap(cpu_id: usize) {
    println!("[Scarlet Kernel] CPU {} is up and running", cpu_id);
    println!("[Scarlet Kernel] Initializing arch...");
    arch_init(cpu_id);
    loop {}
}
