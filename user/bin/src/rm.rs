#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::println;

use std::env;
use std::fs;

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args = env::args_vec();
    if args.len() < 2 {
        println!("rm: missing operand");
        println!("Usage: rm FILE...");
        return 1;
    }
    let mut exit_code = 0;
    for filename in &args[1..] {
        match fs::remove_file(filename) {
            Ok(_) => {}
            Err(_) => {
                println!(
                    "rm: cannot remove '{}': No such file or cannot remove",
                    filename
                );
                exit_code = 1;
            }
        }
    }
    exit_code
}
