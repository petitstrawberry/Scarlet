#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{println, string::ToString};


#[unsafe(no_mangle)]
pub extern "C" fn main() {
    println!("/bin/init: Hello, world!");
    match std::task::clone() {
        0 => {
            println!("/bin/init: I am the child process");
            std::task::exit(0);
        }
        pid => {
            println!("/bin/init: I am the parent process, child PID: {}", pid);
            loop {}
        }
    }
}