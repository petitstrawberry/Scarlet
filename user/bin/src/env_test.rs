#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{println, task::{execve, fork, waitpid, getpid}};

// Function to safely convert a C string to Rust str
unsafe fn cstr_to_str(ptr: *const u8) -> Option<&'static str> {
    if ptr.is_null() {
        return None;
    }
    
    let mut len = 0;
    unsafe {
        while *ptr.add(len) != 0 {
            len += 1;
            if len > 1024 { // Safety limit
                return None;
            }
        }
        
        let slice = core::slice::from_raw_parts(ptr, len);
        core::str::from_utf8(slice).ok()
    }
}

// Function to safely convert argv array to Vec of strings
unsafe fn parse_argv(argc: usize, argv: *const *const u8) -> std::vec::Vec<std::string::String> {
    let mut args = std::vec::Vec::new();
    
    for i in 0..argc {
        if let Some(arg_str) = unsafe { cstr_to_str(*argv.add(i)) } {
            args.push(std::string::String::from(arg_str));
        }
    }
    
    args
}

// Function to safely get environment variables (placeholder implementation)
// TODO: Replace with actual environment variable access once startup routine supports envp
// In the future, this would either:
// 1. Read from envp passed to main() if startup routine is updated
// 2. Use getenv() syscall if implemented
// 3. Access environment through global variable set by startup routine
fn get_env_var(key: &str) -> Option<std::string::String> {
    // Placeholder implementation - simulates some environment variables
    // In real implementation, environment variables would be set by execve
    // and accessible through the startup routine or syscalls
    match key {
        "TEST_VAR" => Some(std::string::String::from("placeholder_value")),
        "PATH" => Some(std::string::String::from("/bin:/usr/bin")),
        _ => None,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    let args = std::env::args_vec();
    
    println!("=== Environment and Argument Test ===");
    println!("This test verifies execve() argument and environment variable passing");
    println!("PID: {}", getpid());
    println!("argc: {}", args.len());
    
    // Display all arguments
    for (i, arg) in args.iter().enumerate() {
        println!("argv[{}]: {}", i, arg);
    }
    // Display all environment variables
    for (k, v) in std::env::vars() {
        println!("env: {}={}", k, v);
    }
    0
}
