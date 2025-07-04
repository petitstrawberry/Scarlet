#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{println, task::{execve, fork, waitpid, getpid}};

const TEST_ENV_KEY: &str = "SCARLET_TEST";
const TEST_ENV_VALUE: &str = "test_value_123";
const CHILD_MARKER: &str = "--child";

fn run_child_test() -> i32 {
    println!("=== CHILD PROCESS TEST ===");
    println!("This is the child process from execve()");
    println!("PID: {}", getpid());
    
    let args = std::env::args_vec();
    println!("Child argc: {}", args.len());
    
    // Display all arguments received by child
    for (i, arg) in args.iter().enumerate() {
        println!("Child argv[{}]: {}", i, arg);
    }
    
    // Check if test environment variable was passed correctly
    match std::env::var(TEST_ENV_KEY) {
        Some(value) => {
            if value == TEST_ENV_VALUE {
                println!("✓ Environment variable passed correctly: {}={}", TEST_ENV_KEY, value);
            } else {
                println!("✗ Environment variable value mismatch: expected '{}', got '{}'", 
                         TEST_ENV_VALUE, value);
                return 1;
            }
        }
        None => {
            println!("✗ Test environment variable '{}' not found", TEST_ENV_KEY);
            return 1;
        }
    }
    
    // Display all environment variables
    println!("Child environment variables:");
    for (k, v) in std::env::vars() {
        println!("  {}={}", k, v);
    }
    
    println!("✓ Child process test completed successfully");
    0
}

fn run_parent_test() -> i32 {
    println!("=== PARENT PROCESS TEST ===");
    println!("This is the parent process, about to exec child");
    println!("PID: {}", getpid());
    
    let args = std::env::args_vec();
    println!("Parent argc: {}", args.len());
    
    // Display parent arguments
    for (i, arg) in args.iter().enumerate() {
        println!("Parent argv[{}]: {}", i, arg);
    }
    
    // Display parent environment
    println!("Parent environment variables:");
    for (k, v) in std::env::vars() {
        println!("  {}={}", k, v);
    }
    
    // Prepare arguments for child process
    let mut child_args = std::vec::Vec::new();
    child_args.push(args[0].as_str());  // Program name
    child_args.push(CHILD_MARKER);      // Marker to indicate child mode
    child_args.push("test_arg1");       // Test argument 1
    child_args.push("test arg with spaces"); // Test argument with spaces
    
    // Prepare environment for child process
    let test_env_var = std::format!("{}={}", TEST_ENV_KEY, TEST_ENV_VALUE);
    let mut child_env = std::vec::Vec::new();
    child_env.push("PATH=/bin:/usr/bin");
    child_env.push("HOME=/root");
    child_env.push(test_env_var.as_str()); // Our test variable
    child_env.push("SHELL=/bin/sh");
    
    println!("Forking and executing child process...");
    
    match fork() {
        0 => {
            // Child process - exec the same program with different args/env
            println!("Child: About to execve with args: {:?}", child_args);
            if execve(&args[0], &child_args, &child_env) != 0 {
                println!("execve failed");
                std::task::exit(1);
            }
            // This should never be reached
            std::task::exit(1);
        }
        -1 => {
            println!("Fork failed");
            return 1;
        }
        child_pid => {
            println!("Parent: Child process created with PID {}", child_pid);
            let (waited_pid, exit_status) = waitpid(child_pid, 0);
            println!("Parent: Child process {} exited with status {}", waited_pid, exit_status);
            
            if exit_status == 0 {
                println!("execve test completed successfully");
                return 0;
            } else {
                println!("Child process failed with exit status {}", exit_status);
                return 1;
            }
        }
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args = std::env::args_vec();
    
    println!("=== Environment and Argument Test ===");
    println!("This test verifies execve() argument and environment variable passing");
    
    // Check if this is a child process (spawned by execve)
    if args.len() > 1 && args[1] == CHILD_MARKER {
        return run_child_test();
    } else {
        return run_parent_test();
    }
}
