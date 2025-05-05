use crate::println;
use crate::syscall::{syscall0, syscall1, syscall3, Syscall};
use crate::vec::Vec;
use crate::boxed::Box;

// use crate::string::String;

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
/// * `argv` - Argument array
/// * `envp` - Environment variable array
///
/// # Return Value
/// - Returns only if an error occurred
/// - On error: -1 (usize::MAX)
pub fn execve(path: &str, argv: &[&str], envp: &[&str]) -> usize {
    let path_ptr = Box::into_raw(str_to_cstr_bytes(path).unwrap().into_boxed_slice()) as *const u8 as usize;
    let argv_ptr = 0; // argv is not used in this implementation
    let envp_ptr = 0; // envp is not used in this implementation
    let res = syscall3(Syscall::Execve, path_ptr, argv_ptr, envp_ptr);
    
    // If the syscall fails, we need to free the allocated memory
    // (On success, the context is switched, so this code is not reached)
    let _ = unsafe { Box::from_raw(path_ptr as *mut u8) }; // Free the path

    // Return the result of the syscall
    res
}

fn str_to_cstr_bytes(s: &str) -> Result<Vec<u8>, ()> {
    if s.as_bytes().contains(&0) {
        return Err(()); // 内部に null バイトがある場合はエラー
    }
    let mut v = Vec::with_capacity(s.len() + 1);
    v.extend_from_slice(s.as_bytes());
    v.push(0); // null 終端
    Ok(v)
}