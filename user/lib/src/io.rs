use crate::fs::{read, write};
use core::fmt;

// Functions related to character output
/// Outputs a single character to the console
/// 
/// This function uses fd 1 (stdout) to output characters.
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
/// This function uses fd 0 (stdin) to read characters.
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
        let bytes_read = read(0, &mut buf);
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

/// Write data to standard output (fd 1)
/// 
/// # Arguments
/// * `buf` - The buffer to write
/// 
/// # Returns
/// The number of bytes written on success, 0 on failure
fn write_to_stdout(buf: &[u8]) -> usize {
    let result = write(1, buf);
    if result as i32 >= 0 {
        result as usize
    } else {
        0
    }
}

/// Internal function to handle formatted output
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    struct Console;

    impl Write for Console {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            puts(s);
            Ok(())
        }
    }

    let _ = Console.write_fmt(args);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::io::_print(format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::print!("\n");
    };
    ($fmt:expr) => {
        $crate::print!(concat!($fmt, "\n"));
    };
    ($fmt:expr, $($arg:tt)*) => {
        $crate::print!(concat!($fmt, "\n"), $($arg)*);
    };
}

/// Read data from standard input (fd 0)
/// 
/// # Arguments
/// * `buf` - The buffer to read into
/// 
/// # Returns
/// The number of bytes read on success, negative value on error
fn read_from_stdin(buf: &mut [u8]) -> i32 {
    read(0, buf)
}

/// Read a line from standard input
/// 
/// # Arguments
/// * `buf` - The buffer to read into
/// * `max_len` - Maximum number of characters to read
/// 
/// # Returns
/// The number of characters read (excluding newline)
pub fn gets(buf: &mut [u8], max_len: usize) -> usize {
    let mut count = 0;
    let actual_max = core::cmp::min(max_len, buf.len().saturating_sub(1));
    
    while count < actual_max {
        let mut single_char = [0u8; 1];
        let bytes_read = read_from_stdin(&mut single_char);
        
        if bytes_read <= 0 {
            break;
        }
        
        let c = single_char[0] as char;
        
        // Stop on newline
        if c == '\n' || c == '\r' {
            break;
        }
        
        buf[count] = single_char[0];
        count += 1;
    }
    
    // Null-terminate if there's space
    if count < buf.len() {
        buf[count] = 0;
    }
    
    count
}
