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
pub extern "C" fn main(argc: usize, argv: *const *const u8) -> i32 {
    println!("Hello, world!");
    println!("PID  = {}", std::task::getpid());
    println!("PPID = {}", std::task::getppid());
    
    println!("\nArguments received:");
    println!("argc = {}", argc);
    
    if argc > 0 && !argv.is_null() {
        unsafe {
            for i in 0..argc {
                let arg_ptr = *argv.add(i);
                if let Some(arg_str) = cstr_to_str(arg_ptr) {
                    println!("argv[{}] = \"{}\"", i, arg_str);
                } else {
                    println!("argv[{}] = (invalid)", i);
                }
            }
        }
    } else {
        println!("No arguments or invalid argv pointer");
    }
    
    println!("\nNote: Environment variables are managed by kernel task.env");
    println!("and converted by ABI modules during process execution.");
    
    return 0;
}