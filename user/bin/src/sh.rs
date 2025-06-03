#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{print, println, string::String, task::{clone, execve, exit, waitpid}};


#[unsafe(no_mangle)]
pub extern "C" fn main() {
    let mut inputs = String::new();

    println!("Scarlet Shell");
    println!("Enter 'exit' to quit");

    loop {
        inputs.clear();
        print!("# ");
        loop {
            let c = std::io::get_char();
            
            if c as u8 >= 0x20 && c as u8 <= 0x7e {
                // Handle printable characters
                inputs.push(c);
                print!("{}", c); 
            } else if c == '\r' {
                print!("\n");
                break;
            } else if c == '\x7f' {
                // Handle backspace
                if !inputs.is_empty() {
                    inputs.pop();
                    print!("\x08 \x08"); // Move back, print space, move back again
                }
            } else if c == '\t' {
                // Handle tab
                inputs.push(' ');
                print!(" ");
            }
        }
        if inputs == "exit" {
            break;
        }

        if inputs.is_empty() {
            continue;
        }

        match clone() {
            0 => {
                // Execute the shell program
                if execve(&inputs, &[], &[]) != 0 {
                    println!("Failed to execve");
                }
                exit(-1);
            }
            -1 => {
                println!("init: Failed to clone");
            }
            pid => {
                waitpid(pid, 0);
            }
        }
    }
    exit(0);
}