#![no_std]
#![no_main]
#![feature(naked_functions)]
#![feature(used_with_arg)]

extern crate scarlet;

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    // This is the main function of the user program.
    // You can put your code here.
    loop {}
}