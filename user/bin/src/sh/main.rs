#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{
    format, print, println,
    string::String,
    task::{execve, exit, fork, waitpid, pipe},
    vec::Vec,
    mem,
};
use std::handle::Handle;
use std::fs::OpenOptions;
use std::io::Read;

// New modules for enhanced shell
mod line_editor;
mod history;
mod parser;

use parser::{Pipeline, Command, RedirectType};

/// Parse a command line into a program and arguments
fn parse_command(input: &str) -> (String, Vec<String>) {
    // First expand environment variables
    let expanded_input = expand_variables(input);

    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let chars = expanded_input.chars();

    for c in chars {
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
                    format!("{path_dir}{program}")
                } else {
                    format!("{path_dir}/{program}")
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
            let current_path = format!("./{program}");
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

            // Get all environment variables and convert them to the format needed for execve
            let env_vars = std::env::vars();
            let env_strings: Vec<String> = env_vars
                .map(|(key, value)| format!("{key}={value}"))
                .collect();
            let env_refs: Vec<&str> = env_strings.iter().map(|s| s.as_str()).collect();

            if execve(&executable_path, &arg_refs, &env_refs) != 0 {
                println!("sh: {}: execution failed", executable_path);
            }
            exit(126); // Standard exit code for "command not executable"
        }
        -1 => {
            println!("sh: fork failed");
            1
        }
        pid => {
            let (_, status) = waitpid(pid, 0);
            status
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

/// Execute a single command from the new Command struct
fn execute_single_command(cmd: &Command) -> i32 {
    let program = &cmd.program;
    let args = &cmd.args;

    // Apply redirects using the existing logic from execute_command
    let mut stdin_handle: Option<Handle> = None;
    let mut stdout_handle: Option<Handle> = None;

    for (rtype, filename) in &cmd.redirects {
        match rtype {
            RedirectType::Output => {
                let mut opts = OpenOptions::new();
                opts.write(true).create(true).truncate(true);
                match opts.open(filename.as_str()) {
                    Ok(f) => {
                        println!("DEBUG: Opened {} for output redirection", filename);
                        stdout_handle = Some(f.into_handle());
                    }
                    Err(_) => {
                        println!("sh: {}: Failed to open file", filename);
                        return 1;
                    }
                }
            }
            RedirectType::Append => {
                let mut opts = OpenOptions::new();
                opts.write(true).create(true).append(true);
                match opts.open(filename.as_str()) {
                    Ok(f) => {
                        stdout_handle = Some(f.into_handle());
                    }
                    Err(_) => {
                        println!("sh: {}: Failed to open file", filename);
                        return 1;
                    }
                }
            }
            RedirectType::Input => {
                match std::fs::File::open(filename.as_str()) {
                    Ok(f) => {
                        stdin_handle = Some(f.into_handle());
                    }
                    Err(_) => {
                        println!("sh: {}: Failed to open file", filename);
                        return 1;
                    }
                }
            }
        }
    }

    // Check if it's a built-in command
    let is_builtin = match program.as_str() {
        "exit" | "env" | "export" | "cd" | "unset" | "echo" | "source" | "." => true,
        _ => false,
    };

    // If builtin with no redirects, run directly
    if is_builtin && stdin_handle.is_none() && stdout_handle.is_none() {
        if let Some(code) = handle_builtin_command(program, args) {
            return code;
        }
        return 0;
    }

    // Locate executable
    let executable_path = if !is_builtin {
        match find_executable_in_path(program) {
            Some(path) => path,
            None => {
                println!("sh: {}: command not found", program);
                return 127;
            }
        }
    } else {
        String::new()
    };

    match fork() {
        0 => {
            // Child: apply redirects and execute
            // For stdin: close handle 0, then dup to get handle 0
            if let Some(h) = stdin_handle {
                // Close stdin (handle 0)
                let _ = unsafe { Handle::from_raw(0) }.close();
                // Duplicate the input file handle - should get assigned to handle 0
                if let Ok(new_h) = h.duplicate() {
                    // If not handle 0, we have a problem, but continue anyway
                    println!("DEBUG: Stdin redirect got handle {}", new_h.as_raw());
                    std::mem::forget(new_h);
                }
                std::mem::forget(h); // Don't close the original
            }
            // For stdout: close handle 1, then dup to get handle 1
            if let Some(h) = stdout_handle {
                println!("DEBUG: Closing stdout and duplicating file");
                // Close stdout (handle 1)
                let _ = unsafe { Handle::from_raw(1) }.close();
                // Duplicate the output file handle - should get assigned to handle 1
                match h.duplicate() {
                    Ok(new_h) => {
                        println!("DEBUG: Stdout redirect got handle {}", new_h.as_raw());
                        std::mem::forget(new_h);
                    }
                    Err(_) => {
                        println!("DEBUG: Failed to duplicate stdout handle!");
                    }
                }
                std::mem::forget(h); // Don't close the original
            }

            let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let env_vars = std::env::vars();
            let env_strings: Vec<String> = env_vars
                .map(|(key, value)| format!("{key}={value}"))
                .collect();
            let env_refs: Vec<&str> = env_strings.iter().map(|s| s.as_str()).collect();

            if is_builtin {
                if let Some(code) = handle_builtin_command(program, args) {
                    exit(code);
                }
            }

            if execve(&executable_path, &arg_refs, &env_refs) != 0 {
                println!("sh: {}: execution failed", executable_path);
            }
            exit(126);
        }
        -1 => {
            println!("sh: fork failed");
            1
        }
        pid => {
            // Parent: wait for child
            let (_, status) = waitpid(pid, 0);
            status
        }
    }
}

/// Execute a pipeline of commands
fn execute_pipeline(pipeline: &Pipeline) -> i32 {
    let num_commands = pipeline.commands.len();

    // Single command - no pipes needed
    if num_commands == 1 {
        return execute_single_command(&pipeline.commands[0]);
    }

    // Multiple commands - set up pipes
    let mut pipes: Vec<(Handle, Handle)> = Vec::new();

    // Create pipes between consecutive commands
    for _ in 0..num_commands - 1 {
        match pipe() {
            Ok((read_end, write_end)) => {
                pipes.push((read_end, write_end));
            }
            Err(_) => {
                println!("sh: failed to create pipe");
                return 1;
            }
        }
    }

    let mut pids: Vec<i32> = Vec::new();

    // Fork and execute each command
    for (i, cmd) in pipeline.commands.iter().enumerate() {
        match fork() {
            0 => {
                // Child process

                // Set up stdin from previous pipe
                if i > 0 {
                    // Not the first command - read from previous pipe
                    if let Err(_) = pipes[i - 1].0.set_role(2) {
                        // stdin role
                        exit(1);
                    }
                }

                // Set up stdout to next pipe
                if i < num_commands - 1 {
                    // Not the last command - write to next pipe
                    if let Err(_) = pipes[i].1.set_role(3) {
                        // stdout role
                        exit(1);
                    }
                }

                // Note: We don't close pipes here because Handle::close() consumes self
                // The handles will be automatically closed when the child process exits
                // or they go out of scope

                // Execute the command
                let exit_code = execute_single_command(cmd);
                exit(exit_code);
            }
            -1 => {
                println!("sh: fork failed");
                // Clean up will happen automatically when pipes go out of scope
                return 1;
            }
            pid => {
                // Parent process
                pids.push(pid);
            }
        }
    }

    // Close all pipes in parent by taking ownership
    for (read_end, write_end) in pipes {
        let _ = read_end.close();
        let _ = write_end.close();
    }

    // Wait for all children
    let mut last_status = 0;
    for pid in pids {
        let (_, status) = waitpid(pid, 0);
        last_status = status;
    }

    last_status
}

/// Interactive shell mode (enhanced version with line editing and history)
fn interactive_shell() -> i32 {
    println!("Scarlet Shell (Enhanced Interactive Mode)");
    println!("Features: cursor movement, command history, pipes");

    // Try to execute .shrc on startup
    execute_shrc();

    println!("Enter 'exit' to quit");

    // Initialize history
    let mut history = history::History::new(100);
    let history_file = "/.sh_history";

    // Try to load history from file
    match history.load_from_file(history_file) {
        Ok(_) => {
            // Successfully loaded history
        }
        Err(_) => {
            // No history file or failed to load, that's fine
        }
    }

    // Initialize line editor with raw mode enabled
    let mut editor = line_editor::LineEditor::new("# ");
    if let Err(_) = editor.set_raw_mode(true) {
        println!("Warning: Failed to enable raw mode, falling back to canonical mode");
    }

    loop {
        // Read a line with history support
        let input = match editor.read_line_with_history(&mut history) {
            Ok(line) => line,
            Err(_) => {
                // Ctrl-C or error
                continue;
            }
        };

        let trimmed = input.trim();

        if trimmed.is_empty() {
            continue;
        }

        // Add to history
        history.add(input.clone());

        // Parse the command using the new parser
        match parser::parse_pipeline(trimmed) {
            Ok(pipeline) => {
                // Execute the pipeline
                let status = execute_pipeline(&pipeline);
                if status != 0 {
                    // Command failed, but continue shell
                }
            }
            Err(err) => {
                println!("sh: parse error: {:?}", err);
            }
        }
    }

    // Save history before exiting (unreachable in practice due to exit command)
    #[allow(unreachable_code)]
    {
        let _ = history.save_to_file(history_file);
        0
    }
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

                    for var_char in chars.by_ref() {
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
                } else if next_char.is_alphabetic()
                    || next_char == '_'
                    || next_char == '?'
                    || next_char == '$'
                    || next_char == '0'
                {
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
                let value = &assignment[eq_pos + 1..];

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
        shrc_paths.push(format!("{home}/.shrc"));
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

            execute_command(&program, &cmd_args)
        } else {
            // Execute script file
            execute_script(script_or_command)
        }
    } else {
        // Interactive mode
        interactive_shell()
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
