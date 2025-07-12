//! Basic I/O utilities for Scarlet user programs
//!
//! This module provides high-level I/O functions using the new Handle-based API.

use crate::handle::Handle;
use core::fmt;

/// Write data to stdout (handle 1)
fn write_to_stdout(data: &[u8]) -> usize {
    // Use handle 1 (stdout) directly
    let stdout = Handle::from_raw(1);
    if let Ok(stream) = stdout.as_stream() {
        match stream.write(data) {
            Ok(bytes_written) => bytes_written,
            Err(_) => 0,
        }
    } else {
        0
    }
}

/// Read data from stdin (handle 0)
fn read_from_stdin(buffer: &mut [u8]) -> usize {
    // Use handle 0 (stdin) directly
    let stdin = Handle::from_raw(0);
    if let Ok(stream) = stdin.as_stream() {
        match stream.read(buffer) {
            Ok(bytes_read) => bytes_read,
            Err(_) => 0,
        }
    } else {
        0
    }
}

/// Outputs a single character to the console
/// 
/// This function uses stdout to output characters.
/// 
/// # Arguments
/// * `c` - The character to output
/// 
/// # Returns
/// The number of bytes written on success, 0 on failure
/// 
pub fn putchar(c: char) -> usize {
    let mut buf = [0u8; 4];
    let char_str = c.encode_utf8(&mut buf);
    write_to_stdout(char_str.as_bytes())
}

/// Reads a single character from the console
/// This function uses stdin to read characters.
/// 
/// # Note
/// This function is blocking and will wait for user input.
/// 
/// # Returns
/// The character read from the console.
/// 
pub fn get_char() -> char {
    let mut buf = [0u8; 1];
    loop {
        let bytes_read = read_from_stdin(&mut buf);
        if bytes_read > 0 {
            return buf[0] as char;
        }
        // If no data available, continue trying
    }
}

/// Outputs a string to the console
/// 
/// # Arguments
/// * `s` - The string to output
/// 
/// # Returns
/// The number of characters output
pub fn puts(s: &str) -> usize {
    write_to_stdout(s.as_bytes())
}

/// Print implementation for Scarlet
pub fn _print(args: fmt::Arguments) {
    use fmt::Write;
    
    let mut writer = StdoutWriter;
    writer.write_fmt(args).unwrap();
}

/// A simple writer that outputs to stdout
struct StdoutWriter;

impl fmt::Write for StdoutWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write_to_stdout(s.as_bytes());
        Ok(())
    }
}

/// Macro for printing to stdout
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::io::_print(format_args!($($arg)*)));
}

/// Macro for printing to stdout with a newline
#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}
