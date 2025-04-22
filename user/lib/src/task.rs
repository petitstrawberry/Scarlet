use crate::syscall::{syscall0, syscall1, syscall3, Syscall};

/// Clones the current process.
/// 
/// # Return Value
/// - In the parent process: the ID of the child process
/// - In the child process: 0
/// - On error: -1 (usize::MAX)
pub fn clone() -> usize {
    syscall0(Syscall::Clone)
}

/// Exits the current process.
/// 
/// # Arguments
/// * `code` - Exit code
pub fn exit(code: i32) -> ! {
    syscall1(Syscall::Exit, code as usize);
    unreachable!("exit syscall should not return");
}

/// Returns the current process ID.
/// Note: This implementation is a placeholder. Until the actual getpid syscall
/// is implemented, it always returns 1.
pub fn getpid() -> usize {
    // Placeholder implementation until the actual getpid syscall is implemented
    1
}

/// Executes a program, replacing the current process image.
/// 
/// # Arguments
/// * `path` - Path to the executable
/// * `argv` - Argument array (can be null)
/// * `envp` - Environment variable array (can be null)
///
/// # Return Value
/// - Returns only if an error occurred
/// - On error: -1 (usize::MAX)
pub fn execve(path: *const u8, argv: *const *const u8, envp: *const *const u8) -> usize {
    syscall3(Syscall::Execve, path as usize, argv as usize, envp as usize)
}
