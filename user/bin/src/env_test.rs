#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{println, task::{execve, fork, waitpid}};

#[unsafe(no_mangle)]
pub extern "C" fn main(_argc: usize, _argv: *const *const u8) -> i32 {
    println!("Environment variable test starting...");
    
    let pid = fork();
    if pid == 0 {
        // Child process - test execve with environment variables
        let path = "/bin/hello";
        let argv = &["hello", "from_env_test"];
        let envp = &["TEST_VAR=hello_world", "PATH=/bin", "USER=scarlet"];
        
        println!("Child: About to execve with env vars");
        let result = execve(path, argv, envp);
        println!("Child: execve returned {}", result);
    } else if pid > 0 {
        // Parent process - wait for child
        println!("Parent: Created child with PID {}", pid);
        let (waited_pid, status) = waitpid(pid, 0);
        println!("Parent: Child {} exited with status {}", waited_pid, status);
    } else {
        println!("Fork failed");
    }
    
    println!("Environment variable test completed");
    return 0;
}
