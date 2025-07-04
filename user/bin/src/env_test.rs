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
pub extern "C" fn main(argc: usize, argv: *const *const u8) -> i32 {
    let args = unsafe { parse_argv(argc, argv) };
    
    println!("=== Environment and Argument Test ===");
    println!("This test verifies execve() argument and environment variable passing");
    println!("NOTE: Environment variable access requires startup routine updates");
    println!("PID: {}", getpid());
    println!("argc: {}", argc);
    
    // Display all arguments
    for (i, arg) in args.iter().enumerate() {
        println!("argv[{}]: {}", i, arg);
    }
    
    // Check if this is a child process (has specific arguments)
    if args.len() > 1 && args[1] == "child_process" {
        println!("\n--- Child Process Environment Test ---");
        println!("NOTE: Environment variable access not yet implemented in startup routine");
        println!("This test verifies that execve with envp arguments works correctly");
        
        // Test argument passing (this should work)
        if args.len() > 2 {
            println!("SUCCESS: Extra argument from parent received: {}", args[2]);
        } else {
            println!("WARNING: Expected extra argument from parent not found");
        }
        
        // Test placeholder environment variable access
        // TODO: Replace with actual environment access once implemented
        println!("Testing placeholder environment variable access:");
        if let Some(test_var) = get_env_var("TEST_VAR") {
            println!("  TEST_VAR = {} (placeholder)", test_var);
        }
        
        if let Some(path) = get_env_var("PATH") {
            println!("  PATH = {} (placeholder)", path);
        }
        
        println!("Child process completed - argument passing verified");
        return 42; // Return specific exit code for parent to verify
    }
    
    // Parent process - test execve with various arguments and environment
    println!("\n--- Parent Process - Testing execve ---");
    
    let pid = fork();
    if pid == 0 {
        // Child process - execve with new arguments and environment
        let path = "/bin/env_test"; // Execute ourselves
        let argv = &["env_test", "child_process", "passed_from_parent"];
        let envp = &[
            "TEST_VAR=hello_from_parent",
            "PATH=/bin:/usr/bin:/system/scarlet/bin", 
            "USER=scarlet_test",
            "CUSTOM_VAR=execve_test_value"
        ];
        
        println!("Parent: About to execve child with custom args and env");
        println!("Parent: execve path: {}", path);
        println!("Parent: execve argv: {:?}", argv);
        println!("Parent: execve envp: {:?}", envp);
        let result = execve(path, argv, envp);
        println!("Parent: execve failed with result {}", result);
        return -1; // Should not reach here if execve succeeds
    } else if pid > 0 {
        // Parent process - wait for child
        println!("Parent: Created child with PID {}", pid);
        let (waited_pid, status) = waitpid(pid, 0);
        println!("Parent: Child {} exited with status {}", waited_pid, status);
        
        if status == 42 {
            println!("SUCCESS: Child returned expected exit code");
        } else {
            println!("WARNING: Child returned unexpected exit code {}", status);
        }
    } else {
        println!("ERROR: Fork failed");
        return 1;
    }
    
    println!("\n=== Environment and Argument Test Completed ===");
    return 0;
}
