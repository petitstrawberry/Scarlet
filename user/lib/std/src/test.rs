//! Test framework for scarlet_std
//!
//! This module provides a custom test runner for no_std environments.
//! It's based on the kernel's test infrastructure and adapted for userland.

use core::panic::PanicInfo;

/// Trait for testable functions
pub trait TestableFn {
    fn run(&self);
}

impl<T> TestableFn for T
where
    T: Fn(),
{
    fn run(&self) {
        crate::println!("[Test Runner] test name={}", core::any::type_name::<T>());
        self();
    }
}

/// Panic handler for test mode (lib tests)
#[cfg(test)]
#[panic_handler]
fn panic_test(info: &PanicInfo) -> ! {
    crate::println!("[Test Runner] panic: {}", info);
    crate::println!("[Test Runner] Test failed");
    
    // Exit with error code
    crate::task::exit(1);
}

/// Test runner function
pub fn test_runner(tests: &[&dyn TestableFn]) {
    crate::println!("[Test Runner] Running {} tests", tests.len());
    for test in tests {
        test.run();
    }

    crate::println!("[Test Runner] All {} tests passed", tests.len());
    
    // Exit successfully
    crate::task::exit(0);
}



