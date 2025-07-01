#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{println, task::execve_abi};


#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("xv6 container");
    println!("Preparing to execute xv6 init...");

    if execve_abi("/system/xv6-riscv64/init", &[], &[], "xv6-riscv64") != 0 {
        println!("Failed to execve xv6 init");
        return -1;
    }

    return 0;
}