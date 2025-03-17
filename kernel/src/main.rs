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
#![feature(used_with_arg)]

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
pub mod initcall;
pub mod syscall;
#[cfg(test)]
pub mod test;

extern crate alloc;
use alloc::string::String;
use initcall::{early::early_initcall_call, initcall_task};

use core::panic::PanicInfo;

use arch::arch_init;
use library::std::print;
use task::new_kernel_task;
use vm::kernel_vm_init;
use sched::scheduler::get_scheduler;
use mem::allocator::init_heap;
use timer::get_kernel_timer;


/// A panic handler is required in Rust, this is probably the most basic one possible
#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    use arch::instruction::idle;

    println!("[Scarlet Kernel] panic: {}", info);
    loop {
        idle();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn start_kernel(cpu_id: usize) -> ! {
    early_println!("Hello, I'm Scarlet kernel!");
    early_println!("[Scarlet Kernel] Boot on CPU {}", cpu_id);
    early_println!("[Scarlet Kernel] Initializing .bss section...");
    init_bss();
    early_println!("[Scarlet Kernel] Initializing arch...");
    arch_init(cpu_id);
    early_println!("[Scarlet Kernel] Initializing heap...");
    init_heap();
    /* After this point, we can use the heap */
    early_initcall_call();
    /* Serial console also works */

    #[cfg(test)]
    test_main();

    println!("[Scarlet Kernel] Initializing Virtual Memory...");
    kernel_vm_init();
    println!("[Scarlet Kernel] Initializing timer...");
    get_kernel_timer().init();
    println!("[Scarlet Kernel] Initializing scheduler...");
    let scheduler = get_scheduler();
    /* Make idle task as initial task */
    println!("[Scarlet Kernel] Creating initial kernel task...");
    let mut task = new_kernel_task(String::from("Initcall"), 0, initcall_task);
    task.init();
    scheduler.add_task(task, cpu_id);
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

fn init_bss() {
    unsafe extern "C" {
        static mut __BSS_START: u8;
        static mut __BSS_END: u8;
    }

    unsafe {
        let bss_start = &raw mut __BSS_START as *mut u8;
        let bss_end = &raw mut __BSS_END as *mut u8;
        let bss_size = bss_end as usize - bss_start as usize;
        core::ptr::write_bytes(bss_start, 0, bss_size);
    }
}