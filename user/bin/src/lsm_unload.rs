#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::env;
use std::println;
use std::syscall::{Syscall, syscall1};
use std::vec::Vec;

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args: Vec<std::string::String> = env::args().collect();

    if args.len() < 2 {
        println!("usage: lsm_unload <module_id>");
        return 1;
    }

    let module_id: u64 = match args[1].parse() {
        Ok(id) => id,
        Err(_) => {
            println!("invalid module id: {}", args[1]);
            return 1;
        }
    };

    let ret = syscall1(Syscall::LsmUnload, module_id as usize);

    if ret != 0 {
        println!("failed to unload module {} (error: {})", module_id, ret);
        return 1;
    }

    println!("module {} unloaded successfully", module_id);
    0
}
