//! Integration test runner for scarlet_std
//!
//! This test binary runs all unit tests defined in scarlet_std modules.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

#[unsafe(no_mangle)]
unsafe extern "C" fn main() -> i32 {
    std::println!("=== scarlet_std Test Runner ===\n");

    // Run tests manually
    let mut passed = 0;
    let mut failed = 0;

    // Test 1: Basic assertion
    std::println!("[Test Runner] Running test: test_basic_assertion");
    if run_test(test_basic_assertion) {
        passed += 1;
    } else {
        failed += 1;
    }

    // Test 2: Vec operations (from collections.rs)
    std::println!("[Test Runner] Running test: test_vec_creation");
    if run_test(test_vec_creation) {
        passed += 1;
    } else {
        failed += 1;
    }

    std::println!("[Test Runner] Running test: test_vec_push");
    if run_test(test_vec_push) {
        passed += 1;
    } else {
        failed += 1;
    }

    // Summary
    std::println!("\n[Test Runner] Test Results:");
    std::println!("  Passed: {}", passed);
    std::println!("  Failed: {}", failed);

    if failed == 0 {
        std::println!("[Test Runner] All {} tests passed", passed);
        0
    } else {
        std::println!("[Test Runner] Some tests failed");
        1
    }
}

fn run_test(test: fn()) -> bool {
    // In a real implementation, we'd use catch_unwind, but that's not available in no_std
    // For now, just run the test directly
    test();
    true
}

fn test_basic_assertion() {
    assert_eq!(1 + 1, 2);
}

fn test_vec_creation() {
    use std::vec::Vec;
    let v: Vec<i32> = Vec::new();
    assert_eq!(v.len(), 0);
    assert_eq!(v.capacity(), 0);
}

fn test_vec_push() {
    use std::vec::Vec;
    let mut v = Vec::new();
    v.push(1);
    v.push(2);
    v.push(3);

    assert_eq!(v.len(), 3);
    assert_eq!(v[0], 1);
    assert_eq!(v[1], 2);
    assert_eq!(v[2], 3);
}
