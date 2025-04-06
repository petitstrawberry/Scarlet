use crate::syscall::{syscall1, Syscall};
use core::fmt;

// Functions related to character output
/// Outputs a single character to the console
/// 
/// This is a temporary implementation that will eventually be replaced
/// by standard output or device files.
/// 
/// # Arguments
/// * `c` - The character to output
/// 
/// # Returns
/// Always returns 0 (success)
pub fn putchar(c: char) -> usize {
    sys_putchar(c)
}

/// Outputs a string to the console
/// 
/// # Arguments
/// * `s` - The string to output
/// 
/// # Returns
/// The number of characters output
pub fn puts(s: &str) -> usize {
    let mut count = 0;
    for c in s.chars() {
        putchar(c);
        count += 1;
    }
    count
}

// Wrapper function for character output
pub fn sys_putchar(c: char) -> usize {
    syscall1(Syscall::Putchar, c as usize)
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
