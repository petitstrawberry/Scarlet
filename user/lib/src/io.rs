use crate::syscall::{syscall1, Syscall};

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
