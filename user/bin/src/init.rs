#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{println, task::{execve, exit, waitpid}};


#[unsafe(no_mangle)]
pub extern "C" fn main() {
    println!("init: I'm the init process: PID={}", std::task::getpid());
    match std::task::clone() {
        0 => {
            // Execute the shell program
            if execve("/bin/sh", &[], &[]) != 0 {
                println!("Failed to execve");
            }
            exit(-1);
        }
        -1 => {
            println!("init: Failed to clone");
            loop {}
        }
        pid => {
            println!("init: I am the parent process, child PID: {}", pid);
            let res = waitpid(pid, 0);
            println!("init: Child process (PID={}) exited with status: {}", res.0, res.1);
            if res.1 != 0 {
                println!("init: Child process exited with error");
            }
            loop {}
        }
    }
}