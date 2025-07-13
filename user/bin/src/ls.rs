#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{format, println};
use std::string::String;
use std::vec::Vec;


#[unsafe(no_mangle)]
fn main() -> i32 {
    let path;
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        // Use the string directly, no need for cstr_ptr_to_str
        path = args[1].as_str();
    } else {
        path = ".";
    }

    match std::fs::list_directory(path) {
        Ok(entries) => {
            if entries.is_empty() {
                return 0;
            }

            // Calculate maximum width for each column
            let mut max_file_id_width = 0;
            let mut max_file_type_width = 0;
            let mut max_size_width = 0;
            let mut max_name_width = 0;

            for entry in &entries {
                let file_id_str = format!("{}", entry.file_id);
                let file_type_str = get_file_type_str(entry.file_type);
                let size_str = format!("{}", entry.size);
                
                max_file_id_width = max_file_id_width.max(file_id_str.len());
                max_file_type_width = max_file_type_width.max(file_type_str.len());
                max_size_width = max_size_width.max(size_str.len());
                max_name_width = max_name_width.max(entry.name.len());
            }

            // Add padding for better readability
            max_file_id_width += 1;
            max_file_type_width += 1;
            max_size_width += 1;

            // Print entries with calculated widths
            for entry in entries {
                let name = entry.name;
                let file_id = entry.file_id;
                let file_type = entry.file_type;
                let size = entry.size;
                let file_type_str = get_file_type_str(file_type);
                
                println!("{:>width_id$} {:width_type$} {:>width_size$} {}", 
                    file_id, 
                    file_type_str, 
                    size, 
                    name,
                    width_id = max_file_id_width,
                    width_type = max_file_type_width,
                    width_size = max_size_width
                );
            }
            return 0;
        }
        Err(errno) => {
            println!("ls: cannot open '{}': error {}", path, errno);
            return 1;
        }
    }
}

fn get_file_type_str(file_type: u8) -> &'static str {
    match file_type {
        0u8 => "Regular File",
        1u8 => "Directory",
        2u8 => "Symbolic Link",
        3u8 => "Character Device",
        4u8 => "Block Device",
        5u8 => "Pipe",
        6u8 => "Socket",
        _ => "Unknown",
    }
}