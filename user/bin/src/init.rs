#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{println, task::{execve, exit}};


#[unsafe(no_mangle)]
pub extern "C" fn main() {
    println!("init: I'm the init process");
    match std::task::clone() {
        0 => {
            println!("init: I am the child process");
            if execve("/bin/hello", &[], &[]) != 0 {
                println!("Failed to execve");
            }
            exit(-1);
        }
        pid => {
            println!("init: I am the parent process, child PID: {}", pid);
            loop {}
        }
    }
}