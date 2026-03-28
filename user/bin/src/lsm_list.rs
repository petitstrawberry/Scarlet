#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::println;
use std::syscall::{Syscall, syscall2};

const ENTRY_SIZE: usize = 264;
const MAX_MODULES: usize = 16;

#[unsafe(no_mangle)]
fn main() -> i32 {
    let mut buf = [0u8; ENTRY_SIZE * MAX_MODULES];
    let count = syscall2(Syscall::LsmList, buf.as_mut_ptr() as usize, buf.len());

    if count == 0 {
        println!("no modules loaded");
        return 0;
    }

    println!("{} module(s) loaded:", count);
    println!("{:<5} {:<40}", "ID", "NAME");

    for i in 0..count {
        let offset = i * ENTRY_SIZE;
        let id = u64::from_le_bytes(buf[offset..offset + 8].try_into().unwrap());
        let name_bytes = &buf[offset + 8..offset + 8 + 256];
        let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(256);
        let name_str = core::str::from_utf8(&name_bytes[..name_len]).unwrap_or("<invalid>");
        println!("{:<5} {}", id, name_str);
    }

    0
}
