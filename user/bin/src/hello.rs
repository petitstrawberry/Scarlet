#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{print, println, string::String, task::exit};


#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("Hello, world!");
    println!("PID  = {}", std::task::getpid());
    println!("PPID = {}", std::task::getppid());
    return 0;
}