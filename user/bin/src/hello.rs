#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{println, task::exit};


#[unsafe(no_mangle)]
pub extern "C" fn main() {
    println!("Hello, world!");
    exit(0);
}