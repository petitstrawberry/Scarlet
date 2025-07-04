#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{print, println, string::String, vec::Vec, task::{execve, exit, fork, waitpid}};

/// Parse a command line into a program and arguments
fn parse_command(input: &str) -> (String, Vec<String>) {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = input.chars();
    
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
            }
            ' ' | '\t' => {
                if in_quotes {
                    current.push(c);
                } else if !current.is_empty() {
                    parts.push(current.clone());
                    current.clear();
                }
            }
            _ => {
                current.push(c);
            }
        }
    }
    
    if !current.is_empty() {
        parts.push(current);
    }
    
    if parts.is_empty() {
        return (String::new(), Vec::new());
    }
    
    let program = parts[0].clone();
    let args = parts;
    
    (program, args)
}

/// Execute a script file
fn execute_script(script_path: &str) -> i32 {
    println!("Executing script: {}", script_path);
    // TODO: Implement script file reading and execution
    // For now, just try to execute it as a binary
    match fork() {
        0 => {
            if execve(script_path, &[script_path], &[]) != 0 {
                println!("Failed to execute script: {}", script_path);
            }
            exit(-1);
        }
        -1 => {
            println!("Failed to fork for script execution");
            return -1;
        }
        pid => {
            let (_, status) = waitpid(pid, 0);
            return status;
        }
    }
}

/// Interactive shell mode
fn interactive_shell() -> i32 {
    let mut inputs = String::new();

    println!("Scarlet Shell (Interactive Mode)");
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
        
        if inputs.trim() == "exit" {
            break;
        }

        if inputs.trim().is_empty() {
            continue;
        }

        let (program, args) = parse_command(inputs.trim());
        
        if program.is_empty() {
            continue;
        }

        match fork() {
            0 => {
                // Convert args to &[&str] for execve
                let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                
                if execve(&program, &arg_refs, &[]) != 0 {
                    println!("Failed to execute: {}", program);
                }
                exit(-1);
            }
            -1 => {
                println!("Failed to fork");
            }
            pid => {
                let (_, _) = waitpid(pid, 0);
            }
        }
    }
    0
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args = std::env::args_vec();
    
    // Check command line arguments
    if args.len() > 1 {
        // Non-interactive mode: execute script or command
        let script_or_command = &args[1];
        
        // Check for -c flag (execute command string)
        if args.len() > 2 && args[1] == "-c" {
            let command = &args[2];
            let (program, cmd_args) = parse_command(command);
            
            if program.is_empty() {
                println!("No command specified");
                return 1;
            }
            
            match fork() {
                0 => {
                    let arg_refs: Vec<&str> = cmd_args.iter().map(|s| s.as_str()).collect();
                    if execve(&program, &arg_refs, &[]) != 0 {
                        println!("Failed to execute: {}", program);
                    }
                    exit(-1);
                }
                -1 => {
                    println!("Failed to fork");
                    return -1;
                }
                pid => {
                    let (_, status) = waitpid(pid, 0);
                    return status;
                }
            }
        } else {
            // Execute script file
            return execute_script(script_or_command);
        }
    } else {
        // Interactive mode
        return interactive_shell();
    }
}