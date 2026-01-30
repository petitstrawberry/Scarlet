//! Integration test runner for scarlet_std
//!
//! This test binary runs all unit tests defined in scarlet_std modules.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate scarlet_std as std;
use core::arch::naked_asm;

// Test runner trait
pub trait TestableFn {
    fn run(&self);
}

impl<T> TestableFn for T
where
    T: Fn(),
{
    fn run(&self) {
        std::println!("[Test Runner] test name={}", core::any::type_name::<T>());
        self();
    }
}

// Test runner function
pub fn test_runner(tests: &[&dyn TestableFn]) {
    std::println!("[Test Runner] Running {} tests", tests.len());
    for test in tests {
        test.run();
    }
    std::println!("[Test Runner] All {} tests passed", tests.len());
    std::task::exit(0);
}

// Panic handler for test mode
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    std::println!("[Test Runner] panic: {}", info);
    std::println!("[Test Runner] Test failed");
    std::task::exit(1);
}

#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_entry")]
#[unsafe(naked)]
pub extern "C" fn _entry() {
    #[cfg(target_arch = "riscv64")]
    naked_asm!(
        "
    .option norvc
    .option norelax
    .align 8
            ecall
            j main
    ",
    );

    #[cfg(target_arch = "aarch64")]
    naked_asm!(
        "
    .align 8
            b main
    ",
    );
}

#[unsafe(link_section = ".text")]
#[unsafe(export_name = "main")]
fn main() {
    std::println!("=== scarlet_std Test Runner ===\n");
    test_main();
}
