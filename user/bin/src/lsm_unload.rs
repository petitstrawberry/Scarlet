#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::env;
use std::println;
use std::string::String;
use std::syscall::{Syscall, syscall1, syscall2};
use std::vec::Vec;

const LSM_LIST_ENTRY_SIZE: usize = 264;
const LSM_LIST_MAX_MODULES: usize = 128;

fn find_module_id_by_name(name: &str) -> Option<u64> {
    let mut buf = [0u8; LSM_LIST_ENTRY_SIZE * LSM_LIST_MAX_MODULES];
    let count = syscall2(Syscall::LsmList, buf.as_mut_ptr() as usize, buf.len());
    if count == 0 {
        return None;
    }

    let max_count = core::cmp::min(count, LSM_LIST_MAX_MODULES);
    for i in 0..max_count {
        let offset = i * LSM_LIST_ENTRY_SIZE;
        let id = u64::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
            buf[offset + 4],
            buf[offset + 5],
            buf[offset + 6],
            buf[offset + 7],
        ]);
        let name_bytes = &buf[offset + 8..offset + 8 + 256];
        let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(256);
        if let Ok(entry_name) = core::str::from_utf8(&name_bytes[..name_len]) {
            if entry_name == name {
                return Some(id);
            }
        }
    }

    None
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        println!("usage: lsm_unload <module_name>");
        return 1;
    }

    let name = &args[1];
    let module_id = match find_module_id_by_name(name) {
        Some(id) => id,
        None => {
            println!("module '{}' not found", name);
            return 1;
        }
    };

    let ret = syscall1(Syscall::LsmUnload, module_id as usize);

    if ret != 0 {
        println!(
            "failed to unload '{}' (id={}, error: {})",
            name, module_id, ret
        );
        return 1;
    }

    println!("module '{}' (id={}) unloaded successfully", name, module_id);
    0
}
