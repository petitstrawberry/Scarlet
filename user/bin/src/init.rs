#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::println;


#[unsafe(no_mangle)]
pub extern "C" fn main() {
    println!("/bin/init: Hello, world!");
    loop {}
}