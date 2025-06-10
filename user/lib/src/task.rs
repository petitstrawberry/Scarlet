use core::clone;

use crate::syscall::{syscall0, syscall1, syscall3, syscall4, Syscall};
use crate::vec::Vec;
use crate::boxed::Box;

pub enum CloneFlagsDef {
    Vm      = 0b00000001, // Clone the VM
    Fs      = 0b00000010, // Clone the filesystem
    Files   = 0b00000100, // Clone the file descriptors
}

#[derive(Debug, Clone, Copy)]
pub struct CloneFlags {
    raw: u64,
}

impl CloneFlags {
    pub fn new() -> Self {
        CloneFlags { raw: 0 }
    }

    pub fn from_raw(raw: u64) -> Self {
        CloneFlags { raw }
    }

    pub fn set(&mut self, flag: CloneFlagsDef) {
        self.raw |= flag as u64;
    }

    pub fn clear(&mut self, flag: CloneFlagsDef) {
        self.raw &= !(flag as u64);
    }

    pub fn is_set(&self, flag: CloneFlagsDef) -> bool {
        (self.raw & (flag as u64)) != 0
    }

    pub fn get_raw(&self) -> u64 {
        self.raw
    }
}

impl Default for CloneFlags {
    fn default() -> Self {
        let raw = CloneFlagsDef::Fs as u64 | CloneFlagsDef::Files as u64;
        CloneFlags { raw }
    }
}

/// Clones the current process.
/// 
/// # Arguments
/// * `flags` - Flags to control the behavior of the clone operation.
/// 
/// # Return Value
/// - In the parent process: the ID of the child process
/// - In the child process: 0
/// - On error: -1
pub fn clone(flags: CloneFlags) -> i32 {
    syscall1(Syscall::Clone, flags.get_raw() as usize) as i32
}

/// Fork the current process.
/// 
/// # Return Value
/// - In the parent process: the ID of the child process
/// - In the child process: 0
/// - On error: -1
pub fn fork() -> i32 {
    let clone_flags = CloneFlags::default();
    clone(clone_flags)
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
///
/// # Return Value
/// - The process ID of the calling process
/// 
pub fn getpid() -> u32 {
    syscall0(Syscall::Getpid) as u32
}

/// Returns the parent process ID.
/// 
/// # Return Value
/// - The process ID of the parent process. If the process has no parent, returns own PID.
/// 
pub fn getppid() -> u32 {
    syscall0(Syscall::Getppid) as u32
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
pub fn execve(path: &str, argv: &[&str], envp: &[&str]) -> i32 {
    let path_boxed_slice = str_to_cstr_bytes(path).unwrap().into_boxed_slice();
    let path_boxed_slice_len = path_boxed_slice.len();
    let path_ptr = Box::into_raw(path_boxed_slice) as *const u8 as usize;

    let argv_ptr = 0; // argv is not used in this implementation
    let envp_ptr = 0; // envp is not used in this implementation
    let res = syscall3(Syscall::Execve, path_ptr, argv_ptr, envp_ptr);
    
    // If the syscall fails, we need to free the allocated memory
    // (On success, the context is switched, so this code is not reached)
    let _ = unsafe { Box::from_raw(core::slice::from_raw_parts_mut(path_ptr as *mut u8, path_boxed_slice_len)) };

    // Return the result of the syscall
    res as i32
}

pub fn execve_abi(path: &str, argv: &[&str], envp: &[&str], abi: &str) -> i32 {
    let path_boxed_slice = str_to_cstr_bytes(path).unwrap().into_boxed_slice();
    let path_boxed_slice_len = path_boxed_slice.len();
    let path_ptr = Box::into_raw(path_boxed_slice) as *const u8 as usize;

    let argv_ptr = 0; // argv is not used in this implementation
    let envp_ptr = 0; // envp is not used in this implementation
   
    let abi_boxed_slice = str_to_cstr_bytes(abi).unwrap().into_boxed_slice();
    let abi_boxed_slice_len = abi_boxed_slice.len();
    let abi_ptr = Box::into_raw(abi_boxed_slice) as *const u8 as usize;
    
    let res = syscall4(Syscall::ExecveABI, path_ptr, argv_ptr, envp_ptr, abi_ptr);

    let _ = unsafe { Box::from_raw(core::slice::from_raw_parts_mut(path_ptr as *mut u8, path_boxed_slice_len)) }; // Free the path
    let _ = unsafe { Box::from_raw(core::slice::from_raw_parts_mut(abi_ptr as *mut u8, abi_boxed_slice_len)) }; // Free the abi

    res as i32
} 

// Converts a Rust string to a null-terminated C string in bytes
fn str_to_cstr_bytes(s: &str) -> Result<Vec<u8>, ()> {
    if s.as_bytes().contains(&0) {
        return Err(()); // Error if there is a null byte inside
    }
    let mut v = Vec::with_capacity(s.len() + 1);
    v.extend_from_slice(s.as_bytes());
    v.push(0); // Null terminator
    Ok(v)
}

/// Waits for a child process to exit.
/// 
/// # Arguments
/// * `pid` - Process ID of the child process to wait for. If -1, wait for any child process.
/// * `options` - Options for the waitpid syscall. (Currently not implemented and always ignored.)
/// 
/// # Return Value
/// (pid, status)
/// - pid: The process ID of the child process that exited.
/// - status: The exit status of the child process.
/// 
pub fn waitpid(pid: i32, options: i32) -> (i32, i32) {
    let mut status: i32 = 0;
    let pid = syscall3(Syscall::Waitpid, pid as usize, &mut status as *mut i32 as usize, options as usize);
    (pid as i32, status)
}

/// Waits for any child process to exit.
/// 
/// # Return Value
/// (pid, status)
/// - pid: The process ID of the child process that exited.
/// - status: The exit status of the child process.
/// 
pub fn wait() -> (i32, i32) {
    waitpid(-1, 0)
}