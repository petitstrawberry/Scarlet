#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{format, print, println, string::String, vec::Vec, task::{execve, exit, fork, waitpid}};
use std::io::Read;

/// Parse a command line into a program and arguments
fn parse_command(input: &str) -> (String, Vec<String>) {
    // First expand environment variables
    let expanded_input = expand_variables(input);
    
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = expanded_input.chars();
    
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

/// Find executable in PATH environment variable
fn find_executable_in_path(program: &str) -> Option<String> {
    // If program contains '/', treat it as an absolute or relative path
    if program.contains('/') {
        return Some(String::from(program));
    }
    
    // Get PATH environment variable
    match std::env::var("PATH") {
        Some(path_var) => {
            // Split PATH by ':' and search in each directory
            for path_dir in path_var.split(':') {
                if path_dir.is_empty() {
                    continue;
                }

                let full_path = if path_dir.ends_with('/') {
                    format!("{}{}", path_dir, program)
                } else {
                    format!("{}/{}", path_dir, program)
                };
                
                // Check if file exists by trying to open it
                match std::fs::File::open(&full_path) {
                    Ok(_) => return Some(full_path),
                    Err(_) => continue,
                }
            }
            None
        }
        None => {
            // No PATH set, try current directory
            let current_path = format!("./{}", program);
            match std::fs::File::open(&current_path) {
                Ok(_) => Some(current_path),
                Err(_) => None,
            }
        }
    }
}

/// Execute a command with PATH resolution
fn execute_command(program: &str, args: &[String]) -> i32 {
    // First check if it's a built-in command
    if let Some(exit_code) = handle_builtin_command(program, args) {
        return exit_code;
    }
    
    let executable_path = match find_executable_in_path(program) {
        Some(path) => path,
        None => {
            println!("sh: {}: command not found", program);
            return 127; // Standard exit code for "command not found"
        }
    };
    
    match fork() {
        0 => {
            // Convert args to &[&str] for execve
            let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            
            if execve(&executable_path, &arg_refs, &[]) != 0 {
                println!("sh: {}: execution failed", executable_path);
            }
            exit(126); // Standard exit code for "command not executable"
        }
        -1 => {
            println!("sh: fork failed");
            return 1;
        }
        pid => {
            let (_, status) = waitpid(pid, 0);
            return status;
        }
    }
}

/// Execute a script file
/// Execute a shell script file
fn execute_script(script_path: &str) -> i32 {
    // Try to read the script file
    let script_content = match read_file(script_path) {
        Ok(content) => content,
        Err(_) => {
            // If we can't read as a script, try to execute as a binary
            println!("Cannot read as script, trying as binary...");
            return execute_command(script_path, &[String::from(script_path)]);
        }
    };
    
    execute_script_content(&script_content)
}

/// Read a file and return its content as a string
fn read_file(file_path: &str) -> Result<String, i32> {
    match std::fs::File::open(file_path) {
        Ok(mut file) => {
            let mut content = String::new();
            let mut buffer = [0u8; 1024];
            
            loop {
                match file.read(&mut buffer) {
                    Ok(0) => break, // EOF
                    Ok(bytes_read) => {
                        // Convert bytes to string (assuming UTF-8)
                        if let Ok(text) = std::str::from_utf8(&buffer[..bytes_read]) {
                            content.push_str(text);
                        } else {
                            return Err(-1); // Invalid UTF-8
                        }
                    }
                    Err(_) => return Err(-1),
                }
            }
            
            Ok(content)
        }
        Err(_) => Err(-1),
    }
}

/// Execute script content line by line
fn execute_script_content(content: &str) -> i32 {
    let mut last_exit_code = 0;
    
    for line in content.lines() {
        let trimmed_line = line.trim();
        
        // Skip empty lines and comments
        if trimmed_line.is_empty() || trimmed_line.starts_with('#') {
            continue;
        }
        
        let (program, args) = parse_command(trimmed_line);
        
        if program.is_empty() {
            continue;
        }
        
        last_exit_code = execute_command(&program, &args);
        
        // If a command fails, we could choose to continue or stop
        // For now, we continue executing the rest of the script
    }
    
    last_exit_code
}

/// Interactive shell mode
fn interactive_shell() -> i32 {
    let mut inputs = String::new();

    println!("Scarlet Shell (Interactive Mode)");
    
    // Try to execute .shrc on startup
    execute_shrc();
    
    println!("Enter 'exit' to quit");

    loop {
        inputs.clear();
        print!("# ");
        loop {
            let c = std::io::get_char();            
            
            if c as u8 >= 0x20 && c as u8 <= 0x7e {
                // Handle printable characters
                inputs.push(c);
            } else if c == '\n' {
                break;
            } else if c == '\x7f' {
                // Handle backspace
                if !inputs.is_empty() {
                    inputs.pop();
                }
            } else if c == '\t' {
                // Handle tab
                inputs.push(' ');
            }
        }
        
        if inputs.trim().is_empty() {
            continue;
        }

        let (program, args) = parse_command(inputs.trim());
        
        if program.is_empty() {
            continue;
        }

        let status = execute_command(&program, &args);
        if status != 0 {
            // Command failed, but continue shell
        }
    }
    // This line is unreachable because 'exit' command terminates the process
    // But we keep it for compiler satisfaction
    #[allow(unreachable_code)]
    0
}

/// Expand environment variables in a string
/// Supports $VAR, ${VAR}, and special variables like $?, $$, $0
fn expand_variables(input: &str) -> String {
    let mut result = String::new();
    let mut chars = input.chars().peekable();
    
    while let Some(c) = chars.next() {
        if c == '$' {
            // Check if this is a variable expansion
            if let Some(&next_char) = chars.peek() {
                if next_char == '{' {
                    // Handle ${VAR} syntax
                    chars.next(); // consume '{'
                    let mut var_name = String::new();
                    let mut found_close = false;
                    
                    while let Some(var_char) = chars.next() {
                        if var_char == '}' {
                            found_close = true;
                            break;
                        }
                        var_name.push(var_char);
                    }
                    
                    if found_close && !var_name.is_empty() {
                        // Expand the variable
                        if let Some(value) = get_variable_value(&var_name) {
                            result.push_str(&value);
                        }
                        // If variable doesn't exist, just ignore it (common shell behavior)
                    } else {
                        // Malformed ${...}, treat as literal
                        result.push('$');
                        result.push('{');
                        result.push_str(&var_name);
                        if !found_close {
                            // Put back the chars we consumed if no closing brace
                            // This is a simplified approach
                        }
                    }
                } else if next_char.is_alphabetic() || next_char == '_' || next_char == '?' || next_char == '$' || next_char == '0' {
                    // Handle $VAR syntax and special variables
                    let mut var_name = String::new();
                    
                    if next_char == '?' || next_char == '$' || next_char == '0' {
                        // Special single-character variables
                        var_name.push(chars.next().unwrap());
                    } else {
                        // Regular variable name
                        while let Some(&var_char) = chars.peek() {
                            if var_char.is_alphanumeric() || var_char == '_' {
                                var_name.push(chars.next().unwrap());
                            } else {
                                break;
                            }
                        }
                    }
                    
                    if !var_name.is_empty() {
                        // Expand the variable
                        if let Some(value) = get_variable_value(&var_name) {
                            result.push_str(&value);
                        }
                        // If variable doesn't exist, just ignore it
                    } else {
                        result.push('$');
                    }
                } else {
                    // Not a variable, just a literal $
                    result.push('$');
                }
            } else {
                // $ at end of string
                result.push('$');
            }
        } else {
            result.push(c);
        }
    }
    
    result
}

/// Get the value of a variable (environment variable or special variable)
fn get_variable_value(var_name: &str) -> Option<String> {
    match var_name {
        "?" => {
            // Exit status of last command (simplified, always return 0 for now)
            Some(String::from("0"))
        }
        "$" => {
            // Process ID (simplified, return a placeholder)
            Some(String::from("1000"))
        }
        "0" => {
            // Name of the shell or script
            Some(String::from("sh"))
        }
        _ => {
            // Regular environment variable
            std::env::var(var_name)
        }
    }
}

/// Handle built-in shell commands
fn handle_builtin_command(program: &str, args: &[String]) -> Option<i32> {
    match program {
        "exit" => {
            let exit_code = if args.len() > 1 {
                args[1].parse::<i32>().unwrap_or(0)
            } else {
                0
            };
            exit(exit_code);
        }
        "env" => {
            // Display all environment variables
            let env_vars = std::env::vars();
            for (key, value) in env_vars {
                println!("{}={}", key, value);
            }
            Some(0)
        }
        "export" => {
            if args.len() < 2 {
                println!("export: usage: export NAME=VALUE");
                return Some(1);
            }
            
            let assignment = &args[1];
            if let Some(eq_pos) = assignment.find('=') {
                let name = &assignment[..eq_pos];
                let value = &assignment[eq_pos+1..];
                
                // Validate variable name (basic check)
                if name.is_empty() {
                    println!("export: invalid variable name");
                    return Some(1);
                }
                
                // Set the environment variable
                std::env::set_var(name, value);
                Some(0)
            } else {
                // If no '=' is provided, show the variable if it exists
                let var_name = assignment;
                match std::env::var(var_name) {
                    Some(value) => {
                        println!("export {}={}", var_name, value);
                        Some(0)
                    }
                    None => {
                        println!("export: {}: variable not set", var_name);
                        Some(1)
                    }
                }
            }
        }
        "cd" => {
            // Change directory
            let target_dir = if args.len() >= 2 {
                &args[1]
            } else {
                // If no argument provided, go to home directory
                &match std::env::var("HOME") {
                    Some(home) => home,
                    None => {
                        println!("cd: HOME not set");
                        return Some(1);
                    }
                }
            };
            
            match std::fs::change_directory(target_dir) {
                Ok(()) => {
                    // Success - update PWD environment variable
                    std::env::set_var("PWD", target_dir);
                    Some(0)
                }
                Err(_) => {
                    println!("cd: {}: No such file or directory", target_dir);
                    Some(1)
                }
            }
        }
        "unset" => {
            if args.len() < 2 {
                println!("unset: usage: unset NAME");
                return Some(1);
            }
            
            let var_name = &args[1];
            
            // Check if variable exists before unsetting
            match std::env::var(var_name) {
                Some(_) => {
                    std::env::remove_var(var_name);
                    println!("unset: removed {}", var_name);
                    Some(0)
                }
                None => {
                    println!("unset: {}: variable not set", var_name);
                    Some(1)
                }
            }
        }
        "echo" => {
            // Echo command - print arguments separated by spaces
            // Supports -n (no newline) and -e (interpret escapes)
            let mut no_newline = false;
            let mut interpret_escapes = false;
            let mut start_index = 1;
            
            // Parse options
            while start_index < args.len() {
                let arg = &args[start_index];
                if arg == "-n" {
                    no_newline = true;
                    start_index += 1;
                } else if arg == "-e" {
                    interpret_escapes = true;
                    start_index += 1;
                } else if arg == "-ne" || arg == "-en" {
                    no_newline = true;
                    interpret_escapes = true;
                    start_index += 1;
                } else if arg.starts_with('-') {
                    // Unknown option, stop parsing
                    break;
                } else {
                    // Not an option, stop parsing
                    break;
                }
            }
            
            if start_index < args.len() {
                let mut output = String::new();
                for (i, arg) in args[start_index..].iter().enumerate() {
                    if i > 0 {
                        output.push(' ');
                    }
                    
                    if interpret_escapes {
                        output.push_str(&process_escape_sequences(arg));
                    } else {
                        output.push_str(arg);
                    }
                }
                
                if no_newline {
                    print!("{}", output);
                } else {
                    println!("{}", output);
                }
            } else {
                // No arguments to print
                if !no_newline {
                    println!();
                }
            }
            Some(0)
        }
        "source" | "." => {
            // Source a script file in the current shell context
            if args.len() < 2 {
                println!("source: usage: source FILENAME");
                return Some(1);
            }
            
            let script_path = &args[1];
            match read_file(script_path) {
                Ok(content) => {
                    let exit_code = execute_script_content(&content);
                    Some(exit_code)
                }
                Err(_) => {
                    println!("source: {}: file not found or cannot read", script_path);
                    Some(1)
                }
            }
        }
        _ => None, // Not a built-in command
    }
}

/// Execute .shrc file if it exists
fn execute_shrc() {
    let mut shrc_paths = Vec::new();
    
    // Add HOME/.shrc if HOME is set
    if let Some(home) = std::env::var("HOME") {
        shrc_paths.push(format!("{}/.shrc", home));
    }
    
    // Add standard paths
    shrc_paths.push(String::from("/.shrc"));
    shrc_paths.push(String::from("/etc/shrc"));
    shrc_paths.push(String::from("./.shrc"));
    
    for shrc_path in &shrc_paths {
        // Check if file exists by trying to open it
        match std::fs::File::open(shrc_path) {
            Ok(_) => {
                println!("Loading {}", shrc_path);
                let exit_code = execute_script(shrc_path);
                if exit_code != 0 {
                    println!("Warning: {} exited with code {}", shrc_path, exit_code);
                }
                return; // Only execute the first found .shrc
            }
            Err(_) => continue,
        }
    }
    
    // No .shrc file found, which is normal
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
            
            return execute_command(&program, &cmd_args);
        } else {
            // Execute script file
            return execute_script(script_or_command);
        }
    } else {
        // Interactive mode
        return interactive_shell();
    }
}

/// Process escape sequences in a string (for echo -e)
fn process_escape_sequences(input: &str) -> String {
    let mut result = String::new();
    let mut chars = input.chars();
    
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next_char) = chars.next() {
                match next_char {
                    'n' => result.push('\n'),
                    't' => result.push('\t'),
                    'r' => result.push('\r'),
                    '\\' => result.push('\\'),
                    '0' => result.push('\0'),
                    'a' => result.push('\x07'), // Bell
                    'b' => result.push('\x08'), // Backspace
                    'f' => result.push('\x0C'), // Form feed
                    'v' => result.push('\x0B'), // Vertical tab
                    'e' => result.push('\x1B'), // Escape
                    _ => {
                        // Unknown escape sequence, treat as literal
                        result.push('\\');
                        result.push(next_char);
                    }
                }
            } else {
                // Backslash at end of string
                result.push('\\');
            }
        } else {
            result.push(c);
        }
    }
    
    result
}