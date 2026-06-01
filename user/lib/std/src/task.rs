use crate::boxed::Box;
use crate::string::ToString;
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
    crate::allocator::fork_prepare();
    let result = clone(clone_flags);
    if result == 0 {
        crate::allocator::fork_child();
    } else {
        crate::allocator::fork_parent();
    }
    result
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
    crate::thread::exit_current_thread(code);
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

/// Errors returned by native session/process-group operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskControlError {
    /// The requested task or process group is not visible in this namespace.
    NotFound,
    /// The requested relationship or state transition is not permitted.
    PermissionDenied,
}

fn task_control_result(value: usize) -> Result<usize, TaskControlError> {
    if value == usize::MAX {
        Err(TaskControlError::PermissionDenied)
    } else {
        Ok(value)
    }
}

/// Create a new session for the current task.
///
/// # Returns
///
/// Namespace-local session ID on success.
pub fn create_session() -> Result<u32, TaskControlError> {
    task_control_result(syscall0(Syscall::CreateSession)).map(|id| id as u32)
}

/// Return the session ID for a task.
///
/// # Arguments
///
/// * `pid` - Namespace-local task ID. `None` means the current task.
///
/// # Returns
///
/// Namespace-local session ID on success.
pub fn session_id(pid: Option<u32>) -> Result<u32, TaskControlError> {
    let pid = pid.unwrap_or(0) as usize;
    task_control_result(syscall1(Syscall::GetSessionId, pid)).map(|id| id as u32)
}

/// Return the process group ID for a task.
///
/// # Arguments
///
/// * `pid` - Namespace-local task ID. `None` means the current task.
///
/// # Returns
///
/// Namespace-local process group ID on success.
pub fn process_group_id(pid: Option<u32>) -> Result<u32, TaskControlError> {
    let pid = pid.unwrap_or(0) as usize;
    task_control_result(syscall1(Syscall::GetProcessGroupId, pid)).map(|id| id as u32)
}

/// Set a task's process group.
///
/// # Arguments
///
/// * `pid` - Namespace-local task ID. `None` means the current task.
/// * `process_group_id` - Namespace-local process group ID. `None` makes the
///   target task become a process-group leader.
///
/// # Returns
///
/// `Ok(())` on success.
pub fn set_process_group(
    pid: Option<u32>,
    process_group_id: Option<u32>,
) -> Result<(), TaskControlError> {
    let pid = pid.unwrap_or(0) as usize;
    let process_group_id = process_group_id.unwrap_or(0) as usize;
    task_control_result(syscall2(Syscall::SetProcessGroup, pid, process_group_id)).map(|_| ())
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

    let res = syscall4(
        Syscall::Execve,
        path_ptr,
        argv_ptr_array as usize,
        envp_ptr_array as usize,
        0_usize,
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

    let res = syscall5(
        Syscall::ExecveABI,
        path_ptr,
        argv_ptr_array as usize,
        envp_ptr_array as usize,
        abi_ptr,
        0_usize,
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

/// Return immediately from [`waitpid`] if no matching child changed state.
pub const WAIT_NOHANG: i32 = 0x1;

/// Report children stopped by Scarlet process-control events.
pub const WAIT_STOPPED: i32 = 0x2;

/// Status returned when a child is stopped by a Scarlet process-control event.
pub const WAIT_STOPPED_STATUS: i32 = 0x7f;

/// Waits for a child process to exit or, with [`WAIT_STOPPED`], stop.
///
/// # Arguments
/// * `pid` - Process ID of the child process to wait for. If -1, wait for any child process.
/// * `options` - Bitmask of `WAIT_*` options.
///
/// # Return Value
/// (pid, status)
/// - pid: The process ID of the child process that changed state.
/// - status: The exit status, or a Scarlet-native stopped status.
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
#[repr(C)]
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

// ---------------------------------------------------------------------------
// Task information (for ps / top)
// ---------------------------------------------------------------------------

/// Task state, mirroring the kernel `TaskState` discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TaskState {
    NotInitialized = 0,
    Ready = 1,
    Running = 2,
    BlockedInterruptible = 3,
    BlockedUninterruptible = 4,
    Zombie = 5,
    Terminated = 6,
}

impl TaskState {
    /// Parse from the raw kernel discriminant.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::NotInitialized),
            1 => Some(Self::Ready),
            2 => Some(Self::Running),
            3 => Some(Self::BlockedInterruptible),
            4 => Some(Self::BlockedUninterruptible),
            5 => Some(Self::Zombie),
            6 => Some(Self::Terminated),
            _ => None,
        }
    }

    /// Short human-readable label (fits in fixed-width columns).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotInitialized => "NotInit",
            Self::Ready => "Ready",
            Self::Running => "Running",
            Self::BlockedInterruptible => "Sleep",
            Self::BlockedUninterruptible => "DiskSlp",
            Self::Zombie => "Zombie",
            Self::Terminated => "Term",
        }
    }
}

impl core::fmt::Display for TaskState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a task runs in kernel or user mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TaskType {
    Kernel = 0,
    User = 1,
}

impl TaskType {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Kernel),
            1 => Some(Self::User),
            _ => None,
        }
    }

    /// Short label for display.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Kernel => "K",
            Self::User => "U",
        }
    }
}

impl core::fmt::Display for TaskType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Snapshot of a single task's metadata.
///
/// This is the user-space mirror of `kernel::task::TaskInfo`.
/// Obtained via [`task::info()`] or the lower-level [`task::info_raw()`].
///
/// # Examples
///
/// ```no_run
/// use std::task;
///
/// for t in task::info() {
///     println!("{} {} {} CPU{}", t.pid(), t.name(), t.state(), t.cpu());
/// }
/// ```
#[derive(Debug, Clone)]
pub struct TaskInfo {
    pid: usize,
    ppid: usize,
    state: TaskState,
    task_type: TaskType,
    cpu: u8,
    exit_status: i32,
    tgid: usize,
    name: crate::string::String,
}

impl TaskInfo {
    /// Process ID (namespace-local).
    pub fn pid(&self) -> usize {
        self.pid
    }
    /// Parent PID (0 if none).
    pub fn ppid(&self) -> usize {
        self.ppid
    }
    /// Current task state.
    pub fn state(&self) -> TaskState {
        self.state
    }
    /// Whether this is a kernel or user task.
    pub fn task_type(&self) -> TaskType {
        self.task_type
    }
    /// CPU the task last ran on.
    pub fn cpu(&self) -> u8 {
        self.cpu
    }
    /// Exit status (meaningful only when `state == Zombie`).
    pub fn exit_status(&self) -> i32 {
        self.exit_status
    }
    /// Thread-group ID (PID for multi-threaded tasks).
    pub fn tgid(&self) -> usize {
        self.tgid
    }
    /// Task name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

// ---------------------------------------------------------------------------
// Raw C-layout struct for the syscall boundary
// ---------------------------------------------------------------------------

/// Opaque raw layout shared with the kernel (`#[repr(C)]`).
///
/// Users should prefer [`TaskInfo`] obtained through [`info()`].
/// This type is public only for advanced use-cases that need zero-copy.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct RawTaskInfo {
    pub pid: usize,
    pub ppid: usize,
    pub state: u8,
    pub task_type: u8,
    pub cpu_id: u8,
    pub _reserved: u8,
    pub exit_status: i32,
    pub tgid: usize,
    pub name: [u8; 64],
}

impl RawTaskInfo {
    fn decode(&self) -> TaskInfo {
        let end = self
            .name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.name.len());
        let name = core::str::from_utf8(&self.name[..end])
            .unwrap_or("<invalid>")
            .to_string();
        TaskInfo {
            pid: self.pid,
            ppid: self.ppid,
            state: TaskState::from_u8(self.state).unwrap_or(TaskState::NotInitialized),
            task_type: TaskType::from_u8(self.task_type).unwrap_or(TaskType::Kernel),
            cpu: self.cpu_id,
            exit_status: self.exit_status,
            tgid: self.tgid,
            name,
        }
    }
}

/// Collect a snapshot of all task metadata.
///
/// This is the primary high-level API — analogous to reading `/proc`
/// on a Unix system.
///
/// # Examples
///
/// ```no_run
/// use std::task;
///
/// for t in task::info() {
///     println!("{:>4} {:>4} {:>8} {} CPU{}", t.pid(), t.ppid(), t.state(), t.name(), t.cpu());
/// }
/// ```
pub fn info() -> crate::vec::Vec<TaskInfo> {
    let raw = info_raw();
    raw.iter().map(|r| r.decode()).collect()
}

/// Collect raw task info snapshots (zero-allocation decode deferred).
///
/// Prefer [`info()`] for ergonomics. Use this when you need maximum
/// performance or want to decode only a subset.
pub fn info_raw() -> crate::vec::Vec<RawTaskInfo> {
    let total = syscall0(Syscall::GetTaskInfoCount);
    let mut buf = crate::vec![RawTaskInfo {
        pid: 0, ppid: 0, state: 0, task_type: 0, cpu_id: 0,
        _reserved: 0, exit_status: 0, tgid: 0, name: [0; 64],
    }; total];
    let n = syscall2(
        Syscall::GetTaskInfoList,
        buf.as_mut_ptr() as usize,
        buf.len(),
    );
    buf.truncate(n);
    buf
}
