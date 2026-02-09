use crate::boxed::Box;
use crate::syscall::{Syscall, syscall0, syscall1, syscall2, syscall3, syscall4, syscall5};
use crate::vec::Vec;

// Flags for execve system calls
pub const EXECVE_FORCE_ABI_REBUILD: usize = 0x1; // Force ABI environment reconstruction

#[repr(u64)]
pub enum CloneFlagsDef {
    Vm = 0b00000001,     // Clone the VM
    Fs = 0b00000010,     // Clone the filesystem
    Files = 0b00000100,  // Clone the file descriptors
    Thread = 0b00001000, // Join thread group (share TGID) - Linux CLONE_THREAD semantics
    SetTls = 0b00010000, // Set TLS pointer for cloned task
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
    /// Returns default CloneFlags with FS flag set
    /// This mimics the behavior of fork(), where the filesystem context is shared.
    fn default() -> Self {
        let raw = CloneFlagsDef::Fs as u64;
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
    syscall5(Syscall::Clone, flags.get_raw() as usize, 0, 0, 0, 0) as i32
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

/// Exits the current process (all threads).
///
/// This function terminates the entire process, including all threads.
/// This matches the behavior of Rust's `std::process::exit()` and is the
/// standard way to exit a multi-threaded program.
///
/// # Arguments
/// * `code` - Exit code
///
/// # Behavior
/// - Terminates all tasks with the same TGID (entire process)
/// - This is the equivalent of calling `exit_group()`
/// - All threads in the process are terminated
///
/// # Example
/// ```rust
/// use std::task;
/// use std::thread;
///
/// thread::spawn(|| {
///     loop {} // This thread will be terminated
/// });
///
/// task::exit(0); // Terminates entire process
/// ```
pub fn exit(code: i32) -> ! {
    syscall1(Syscall::ExitGroup, code as usize);
    unreachable!("exit syscall should not return");
}

/// Exits only the current thread.
///
/// This function terminates only the calling thread, not the entire process.
/// Other threads in the process continue running. Use `exit()` to terminate
/// the entire process.
///
/// # Arguments
/// * `code` - Exit code (only meaningful if this is the last thread)
///
/// # Behavior
/// - Terminates only the calling thread
/// - Other threads in the process continue running
/// - If this is the last thread in the process, the process exits
///
/// # Example
/// ```rust
/// use std::task;
/// use std::thread;
///
/// thread::spawn(|| {
///     task::exit_thread(0); // Only this thread exits
/// });
///
/// // Main thread continues running
/// loop {}
/// ```
pub fn exit_thread(code: i32) -> ! {
    syscall1(Syscall::Exit, code as usize);
    unreachable!("exit_thread syscall should not return");
}

/// Exits all tasks in the current thread group
///
/// This function terminates all threads (tasks) in the current process/thread group.
/// This is similar to the Linux exit_group system call and is the proper way
/// for multi-threaded processes to exit.
///
/// # Arguments
/// * `code` - Exit status code for all threads in the group
///
/// # Behavior
/// - Terminates all tasks with the same TGID as the caller
/// - The calling task and all sibling threads are terminated
/// - This function does not return on success
///
/// # Example
/// ```rust
/// use std::task;
/// use std::thread;
///
/// thread::spawn(|| {
///     // This thread will be terminated when main calls exit_group
///     loop {}
/// });
///
/// // This will terminate both main and the spawned thread
/// task::exit_group(0);
/// ```
pub fn exit_group(code: i32) -> ! {
    syscall1(Syscall::ExitGroup, code as usize);
    unreachable!("exit_group syscall should not return");
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

    // Convert argv to C-style array
    let (argv_data, argv_ptrs) = if argv.is_empty() {
        (Vec::new(), create_empty_ptr_array())
    } else {
        strarr_to_cstr_ptrs(argv).unwrap_or_else(|_| (Vec::new(), create_empty_ptr_array()))
    };
    let (argv_ptr_array, argv_len) = create_ptr_array_box(argv_ptrs);

    // Convert envp to C-style array
    let (envp_data, envp_ptrs) = if envp.is_empty() {
        (Vec::new(), create_empty_ptr_array())
    } else {
        strarr_to_cstr_ptrs(envp).unwrap_or_else(|_| (Vec::new(), create_empty_ptr_array()))
    };
    let (envp_ptr_array, envp_len) = create_ptr_array_box(envp_ptrs);

    let res = syscall3(
        Syscall::Execve,
        path_ptr,
        argv_ptr_array as usize,
        envp_ptr_array as usize,
    );

    // If the syscall fails, we need to free the allocated memory
    // (On success, the context is switched, so this code is not reached)
    let _ = unsafe {
        Box::from_raw(core::ptr::slice_from_raw_parts_mut(
            path_ptr as *mut u8,
            path_boxed_slice_len,
        ))
    };
    let _ = unsafe {
        Box::from_raw(core::ptr::slice_from_raw_parts_mut(
            argv_ptr_array as *mut usize,
            argv_len,
        ))
    };
    let _ = unsafe {
        Box::from_raw(core::ptr::slice_from_raw_parts_mut(
            envp_ptr_array as *mut usize,
            envp_len,
        ))
    };

    // Keep argv_data and envp_data alive until syscall completes
    drop(argv_data);
    drop(envp_data);

    // Return the result of the syscall
    res as i32
}

pub fn execve_abi(path: &str, argv: &[&str], envp: &[&str], abi: &str) -> i32 {
    let path_boxed_slice = str_to_cstr_bytes(path).unwrap().into_boxed_slice();
    let path_boxed_slice_len = path_boxed_slice.len();
    let path_ptr = Box::into_raw(path_boxed_slice) as *const u8 as usize;

    // Convert argv to C-style array
    let (argv_data, argv_ptrs) = if argv.is_empty() {
        (Vec::new(), create_empty_ptr_array())
    } else {
        strarr_to_cstr_ptrs(argv).unwrap_or_else(|_| (Vec::new(), create_empty_ptr_array()))
    };
    let (argv_ptr_array, argv_len) = create_ptr_array_box(argv_ptrs);

    // Convert envp to C-style array
    let (envp_data, envp_ptrs) = if envp.is_empty() {
        (Vec::new(), create_empty_ptr_array())
    } else {
        strarr_to_cstr_ptrs(envp).unwrap_or_else(|_| (Vec::new(), create_empty_ptr_array()))
    };
    let (envp_ptr_array, envp_len) = create_ptr_array_box(envp_ptrs);

    let abi_boxed_slice = str_to_cstr_bytes(abi).unwrap().into_boxed_slice();
    let abi_boxed_slice_len = abi_boxed_slice.len();
    let abi_ptr = Box::into_raw(abi_boxed_slice) as *const u8 as usize;

    let res = syscall4(
        Syscall::ExecveABI,
        path_ptr,
        argv_ptr_array as usize,
        envp_ptr_array as usize,
        abi_ptr,
    );

    let _ = unsafe {
        Box::from_raw(core::ptr::slice_from_raw_parts_mut(
            path_ptr as *mut u8,
            path_boxed_slice_len,
        ))
    }; // Free the path
    let _ = unsafe {
        Box::from_raw(core::ptr::slice_from_raw_parts_mut(
            abi_ptr as *mut u8,
            abi_boxed_slice_len,
        ))
    }; // Free the abi
    let _ = unsafe {
        Box::from_raw(core::ptr::slice_from_raw_parts_mut(
            argv_ptr_array as *mut usize,
            argv_len,
        ))
    };
    let _ = unsafe {
        Box::from_raw(core::ptr::slice_from_raw_parts_mut(
            envp_ptr_array as *mut usize,
            envp_len,
        ))
    };

    // Keep argv_data and envp_data alive until syscall completes
    drop(argv_data);
    drop(envp_data);

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

// Converts a slice of strings to a null-terminated array of C string pointers
fn strarr_to_cstr_ptrs(arr: &[&str]) -> Result<(Vec<Vec<u8>>, Vec<usize>), ()> {
    let mut string_data = Vec::with_capacity(arr.len());
    let mut ptrs = Vec::with_capacity(arr.len() + 1);

    for s in arr {
        let cstr_bytes = str_to_cstr_bytes(s)?;
        ptrs.push(cstr_bytes.as_ptr() as usize);
        string_data.push(cstr_bytes);
    }
    ptrs.push(0); // Null terminator for the array

    Ok((string_data, ptrs))
}

// Creates an empty pointer array with just null terminator
fn create_empty_ptr_array() -> Vec<usize> {
    let mut v = Vec::with_capacity(1);
    v.push(0);
    v
}

// Creates a boxed slice from pointer array for passing to syscalls
fn create_ptr_array_box(ptrs: Vec<usize>) -> (*const usize, usize) {
    let len = ptrs.len();
    let boxed_slice = ptrs.into_boxed_slice();
    let ptr = Box::into_raw(boxed_slice) as *const usize;
    (ptr, len)
}

/// Execute a program with flags support
///
/// This function extends execve() to support additional flags,
/// particularly for forcing ABI environment reconstruction.
///
/// # Arguments
/// * `path` - Path to the executable
/// * `argv` - Command line arguments
/// * `envp` - Environment variables
/// * `flags` - Execution flags (e.g., EXECVE_FORCE_ABI_REBUILD)
///
/// # Return Value
/// - Returns only if an error occurred
/// - On error: -1 (usize::MAX)
pub fn execve_with_flags(path: &str, argv: &[&str], envp: &[&str], flags: usize) -> i32 {
    let path_boxed_slice = str_to_cstr_bytes(path).unwrap().into_boxed_slice();
    let path_boxed_slice_len = path_boxed_slice.len();
    let path_ptr = Box::into_raw(path_boxed_slice) as *const u8 as usize;

    // Convert argv to C-style array
    let (argv_data, argv_ptrs) = if argv.is_empty() {
        (Vec::new(), create_empty_ptr_array())
    } else {
        strarr_to_cstr_ptrs(argv).unwrap_or_else(|_| (Vec::new(), create_empty_ptr_array()))
    };
    let (argv_ptr_array, argv_len) = create_ptr_array_box(argv_ptrs);

    // Convert envp to C-style array
    let (envp_data, envp_ptrs) = if envp.is_empty() {
        (Vec::new(), create_empty_ptr_array())
    } else {
        strarr_to_cstr_ptrs(envp).unwrap_or_else(|_| (Vec::new(), create_empty_ptr_array()))
    };
    let (envp_ptr_array, envp_len) = create_ptr_array_box(envp_ptrs);

    let res = syscall4(
        Syscall::Execve,
        path_ptr,
        argv_ptr_array as usize,
        envp_ptr_array as usize,
        flags,
    );

    // If the syscall fails, we need to free the allocated memory
    // (On success, the context is switched, so this code is not reached)
    let _ = unsafe {
        Box::from_raw(core::ptr::slice_from_raw_parts_mut(
            path_ptr as *mut u8,
            path_boxed_slice_len,
        ))
    };
    let _ = unsafe {
        Box::from_raw(core::ptr::slice_from_raw_parts_mut(
            argv_ptr_array as *mut usize,
            argv_len,
        ))
    };
    let _ = unsafe {
        Box::from_raw(core::ptr::slice_from_raw_parts_mut(
            envp_ptr_array as *mut usize,
            envp_len,
        ))
    };

    // Keep argv_data and envp_data alive until syscall completes
    drop(argv_data);
    drop(envp_data);

    // Return the result of the syscall
    res as i32
}

/// Execute a program with explicit ABI specification and flags support
///
/// This function extends execve_abi() to support additional flags,
/// particularly for forcing ABI environment reconstruction.
///
/// # Arguments
/// * `path` - Path to the executable
/// * `argv` - Command line arguments
/// * `envp` - Environment variables
/// * `abi` - Target ABI name
/// * `flags` - Execution flags (e.g., EXECVE_FORCE_ABI_REBUILD)
///
/// # Return Value
/// - Returns only if an error occurred
/// - On error: -1 (usize::MAX)
pub fn execve_abi_with_flags(
    path: &str,
    argv: &[&str],
    envp: &[&str],
    abi: &str,
    flags: usize,
) -> i32 {
    let path_boxed_slice = str_to_cstr_bytes(path).unwrap().into_boxed_slice();
    let path_boxed_slice_len = path_boxed_slice.len();
    let path_ptr = Box::into_raw(path_boxed_slice) as *const u8 as usize;

    // Convert argv to C-style array
    let (argv_data, argv_ptrs) = if argv.is_empty() {
        (Vec::new(), create_empty_ptr_array())
    } else {
        strarr_to_cstr_ptrs(argv).unwrap_or_else(|_| (Vec::new(), create_empty_ptr_array()))
    };
    let (argv_ptr_array, argv_len) = create_ptr_array_box(argv_ptrs);

    // Convert envp to C-style array
    let (envp_data, envp_ptrs) = if envp.is_empty() {
        (Vec::new(), create_empty_ptr_array())
    } else {
        strarr_to_cstr_ptrs(envp).unwrap_or_else(|_| (Vec::new(), create_empty_ptr_array()))
    };
    let (envp_ptr_array, envp_len) = create_ptr_array_box(envp_ptrs);

    let abi_boxed_slice = str_to_cstr_bytes(abi).unwrap().into_boxed_slice();
    let abi_boxed_slice_len = abi_boxed_slice.len();
    let abi_ptr = Box::into_raw(abi_boxed_slice) as *const u8 as usize;

    let res = syscall5(
        Syscall::ExecveABI,
        path_ptr,
        argv_ptr_array as usize,
        envp_ptr_array as usize,
        abi_ptr,
        flags,
    );

    let _ = unsafe {
        Box::from_raw(core::ptr::slice_from_raw_parts_mut(
            path_ptr as *mut u8,
            path_boxed_slice_len,
        ))
    }; // Free the path
    let _ = unsafe {
        Box::from_raw(core::ptr::slice_from_raw_parts_mut(
            abi_ptr as *mut u8,
            abi_boxed_slice_len,
        ))
    }; // Free the abi
    let _ = unsafe {
        Box::from_raw(core::ptr::slice_from_raw_parts_mut(
            argv_ptr_array as *mut usize,
            argv_len,
        ))
    };
    let _ = unsafe {
        Box::from_raw(core::ptr::slice_from_raw_parts_mut(
            envp_ptr_array as *mut usize,
            envp_len,
        ))
    };

    // Keep argv_data and envp_data alive until syscall completes
    drop(argv_data);
    drop(envp_data);

    res as i32
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
    let pid = syscall3(
        Syscall::Waitpid,
        pid as usize,
        &mut status as *mut i32 as usize,
        options as usize,
    );
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

/// Creates a pipe pair
///
/// Creates a unidirectional pipe with read and write ends.
///
/// # Return Value
/// - Ok((read_handle, write_handle)): On success, returns tuple of handles
/// - Err(error_code): On failure
///
/// # Example
/// ```no_run
/// use scarlet_std::task::pipe;
/// use scarlet_std::handle::Handle;
///
/// let (read_end, write_end) = pipe().expect("Failed to create pipe");
/// // Use read_end and write_end for IPC
/// ```
pub fn pipe() -> Result<(crate::handle::Handle, crate::handle::Handle), i32> {
    let mut pipefd = [0u32; 2];
    let result = syscall2(
        Syscall::Pipe,
        pipefd.as_mut_ptr() as usize,
        0, // flags (not used yet in sys_pipe)
    );

    if result == usize::MAX {
        return Err(-1);
    }

    let read_handle = match unsafe { crate::handle::Handle::from_raw(pipefd[0] as i32) } {
        Ok(h) => h,
        Err(_) => {
            let _ = syscall1(Syscall::HandleClose, pipefd[0] as usize);
            let _ = syscall1(Syscall::HandleClose, pipefd[1] as usize);
            return Err(-1);
        }
    };
    let write_handle = match unsafe { crate::handle::Handle::from_raw(pipefd[1] as i32) } {
        Ok(h) => h,
        Err(_) => {
            let _ = syscall1(Syscall::HandleClose, pipefd[0] as usize);
            let _ = syscall1(Syscall::HandleClose, pipefd[1] as usize);
            return Err(-1);
        }
    };
    Ok((read_handle, write_handle))
}

/// Shutdown types for the system
#[derive(Debug, Clone, Copy)]
pub enum ShutdownType {
    /// Power off the system
    PowerOff = 0,
    /// Reboot the system
    Reboot = 1,
}

/// Shutdown the system gracefully
///
/// This function initiates a graceful shutdown sequence:
/// 1. Terminate all tasks
/// 2. Sync all filesystems
/// 3. Unmount all filesystems
/// 4. Request platform shutdown
///
/// # Arguments
/// * `shutdown_type` - Type of shutdown (PowerOff or Reboot)
///
/// # Example
/// ```no_run
/// use scarlet_std::task::{shutdown, ShutdownType};
///
/// // Power off the system
/// shutdown(ShutdownType::PowerOff);
///
/// // Or reboot
/// // shutdown(ShutdownType::Reboot);
/// ```
pub fn shutdown(shutdown_type: ShutdownType) -> ! {
    syscall1(Syscall::Shutdown, shutdown_type as usize);
    unreachable!("shutdown syscall should not return");
}
