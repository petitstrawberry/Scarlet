#![no_std]
#![no_main]

extern crate scarlet_std as std;
use std::{format, println, string::ToString, vec::Vec};

#[unsafe(no_mangle)]
fn main() -> i32 {
    // TODO: Implement login functionality. User authentication, session management, etc.
    std::env::set_var("USER", "root");
    std::env::set_var("HOME", "/root");
    std::env::set_var("SHELL", "/bin/sh");
    println!("Login successful for user: {}", std::env::var("USER").unwrap_or("unknown".to_string()));

    let mut env = Vec::new();

    for (key, value) in std::env::vars() {
        env.push(format!("{}={}", key, value));
    }

    // Convert Vec<String> to Vec<&str> for execve
    let env: Vec<&str> = env.iter().map(|s| s.as_str()).collect();

    // Start the shell process
    match std::task::fork() {
        0 => {
            let shell_path = std::env::var("SHELL").unwrap_or("/bin/sh".to_string());
            // Child process: Execute the shell program
            if std::task::execve(&shell_path, &[&shell_path], &env) != 0 {
                println!("Failed to execve /bin/sh");
                return -1; // Exit with error}
            }
        }
        -1 => {
            println!("Failed to fork");
            return -1; // Exit with error
        }
        pid => {
            let res = std::task::waitpid(pid, 0);
            println!("Child process (PID={}) exited with status: {}", res.0, res.1);
            if res.1 != 0 {
                println!("Child process exited with error");
            }
        }
    }

    return 0;
}