#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{println, print, format};
use std::fs::File;
use std::string::String;
use std::vec::Vec;

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    
    // If no arguments provided, read from stdin (not implemented yet)
    if args.len() <= 1 {
        println!("cat: missing file operand");
        println!("usage: cat [FILE]");
        return 1;
    }

    let mut exit_code = 0;

    // Process each file argument
    for i in 1..args.len() {
        let filename = &args[i];
        
        match cat_file(filename) {
            Ok(_) => {},
            Err(err) => {
                println!("cat: {}: {}", filename, err);
                exit_code = 1;
            }
        }
    }

    exit_code
}

fn cat_file(filename: &str) -> Result<(), String> {
    // Open the file
    let mut file = match File::open(filename) {
        Ok(f) => f,
        Err(_) => return Err(format!("No such file or directory")),
    };

    // Read file contents in chunks
    let mut buffer = [0u8; 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(bytes_read) => {
                if bytes_read == 0 {
                    break; // End of file
                }
                
                // Convert bytes to string and print
                // Handle potential UTF-8 conversion errors gracefully
                let slice = &buffer[..bytes_read];
                match core::str::from_utf8(slice) {
                    Ok(s) => print!("{}", s),
                    Err(_) => {
                        // If UTF-8 conversion fails, print bytes as hex
                        for &byte in slice {
                            if byte >= 32 && byte <= 126 {
                                // Printable ASCII
                                print!("{}", char::from(byte));
                            } else {
                                // Non-printable, show as hex
                                print!("\\x{:02x}", byte);
                            }
                        }
                    }
                }
            },
            Err(_) => return Err(format!("Read error")),
        }
    }

    Ok(())
}
