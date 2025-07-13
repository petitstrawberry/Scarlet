#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::println;

// Function to safely convert a C string to Rust str
unsafe fn cstr_to_str(ptr: *const u8) -> Option<&'static str> {
    if ptr.is_null() {
        return None;
    }
    
    let mut len = 0;
    unsafe {
        while *ptr.add(len) != 0 {
            len += 1;
            if len > 1024 { // Safety limit
                return None;
            }
        }
        
        let slice = core::slice::from_raw_parts(ptr, len);
        core::str::from_utf8(slice).ok()
    }
}

// Main function that receives argc and argv from _start
#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("Hello, world!");
    println!("PID  = {}", std::task::getpid());
    println!("PPID = {}", std::task::getppid());
    return 0;
}