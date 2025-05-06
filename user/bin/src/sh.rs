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
            if c == '\r' {
                break;
            }
            inputs.push(c);
            print!("{}", c);
        }
        println!();
        if inputs == "exit" {
            break;
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