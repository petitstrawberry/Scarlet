//! Test framework for scarlet-ui
//!
//! This module provides a custom test runner for no_std environments.

/// Trait for testable functions
pub trait TestableFn {
    fn run(&self);
}

impl<T> TestableFn for T
where
    T: Fn(),
{
    fn run(&self) {
        use crate::std::println;
        println!("[Test Runner] test name={}", core::any::type_name::<T>());
        self();
    }
}

/// Test runner function
pub fn test_runner(tests: &[&dyn TestableFn]) {
    use crate::std::{println, task};
    
    println!("[Test Runner] Running {} tests", tests.len());
    for test in tests {
        test.run();
    }

    println!("[Test Runner] All {} tests passed", tests.len());
    
    // Exit successfully
    task::exit(0);
}

