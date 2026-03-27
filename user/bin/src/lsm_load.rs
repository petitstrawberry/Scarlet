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
        println!("usage: lsm_load <module.lsm>");
        return 1;
    }

    let path_str = &args[1];
    println!("loading module: {}", path_str);

    let path_ptr = path_str.as_ptr();
    let ret = syscall1(Syscall::LsmLoad, path_ptr as usize);

    if ret == usize::MAX {
        println!("failed to load module");
        return 1;
    }

    println!("module loaded successfully");
    0
}
