#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::println;
use std::ffi::cstr_ptr_to_str;


#[unsafe(no_mangle)]
pub extern "C" fn main(argc: isize, argv: *const *const u8) -> i32 {
    let mut path = "/";
    if argc > 1 {
        unsafe {
            let arg_ptr = *argv.offset(1);
            if let Some(s) = cstr_ptr_to_str(arg_ptr) {
                path = s;
            }
        }
    }
    match std::fs::list_directory(path) {
        Ok(entries) => {
            for entry in entries {
                println!("{}", entry.name_str());
            }
            0
        }
        Err(errno) => {
            println!("ls: cannot open '{}': error {}", path, errno);
            1
        }
    }
}