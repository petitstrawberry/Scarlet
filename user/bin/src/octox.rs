#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{println, task::execve_abi};

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("octox container");
    println!("Preparing to execute octox init...");

    if execve_abi("/scarlet/system/octox-riscv64/init", &[], &[], "octox-riscv64") != 0 {
        println!("Failed to execve octox init");
        return -1;
    }

    0
}
