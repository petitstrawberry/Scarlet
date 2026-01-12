//! Task-related system call implementations.
//!
//! This module implements system calls that interact with task management,
//! filesystem operations, and process control. Many operations leverage
//! the VfsManager for filesystem access when tasks have isolated namespaces.
//!
//! # VfsManager Integration
//!
//! System calls automatically use the task's VfsManager when available:
//! - Tasks with `vfs: Some(Arc<VfsManager>)` use their isolated filesystem namespace
//! - Tasks with `vfs: None` fall back to global filesystem operations
//! - Bind mount operations enable controlled sharing between isolated namespaces
//! - All filesystem operations are thread-safe and handle concurrent access properly

use core::usize;

use alloc::vec::Vec;

use crate::abi::MAX_ABI_LENGTH;
use crate::device::manager::DeviceManager;
use crate::executor::executor::TransparentExecutor;
use crate::fs::MAX_PATH_LENGTH;
use crate::library::std::string::{
    parse_c_string_from_userspace, parse_string_array_from_userspace,
};

use crate::arch::{Trapframe, get_cpu};
use crate::sched::scheduler::get_scheduler;
use crate::task::{CloneFlags, WaitError, get_parent_waitpid_waker, get_waitpid_waker};
use crate::timer::ns_to_ticks;

const MAX_ARG_COUNT: usize = 256; // Maximum number of arguments for execve

// Flags for execve system calls
pub const EXECVE_FORCE_ABI_REBUILD: usize = 0x1; // Force ABI environment reconstruction

use super::mytask;

pub fn sys_brk(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let brk = trapframe.get_arg(0);
    trapframe.increment_pc_next(task);
    match task.set_brk(brk) {
        Ok(_) => task.get_brk(),
        Err(_) => usize::MAX, /* -1 */
    }
}

pub fn sys_sbrk(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let increment = trapframe.get_arg(0);
    let brk = task.get_brk();
    trapframe.increment_pc_next(task);
    match task.set_brk(unsafe { brk.unchecked_add(increment) }) {
        Ok(_) => brk,
        Err(_) => usize::MAX, /* -1 */
    }
}

pub fn sys_putchar(trapframe: &mut Trapframe) -> usize {
    let c = trapframe.get_arg(0) as u32;
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    if let Some(ch) = char::from_u32(c) {
        let manager = DeviceManager::get_manager();
        if let Some(device_id) = manager.get_first_device_by_type(crate::device::DeviceType::Char) {
            if let Some(char_device) = manager.get_device(device_id).unwrap().as_char_device() {
                // Use CharDevice trait methods to write
                if let Err(e) = char_device.write_byte(ch as u8) {
                    crate::print!("Error writing character: {}", e);
                    return usize::MAX; // -1
                }
                // Successfully written character
                return 0;
            }
        }
    }
    return usize::MAX; // -1
}

pub fn sys_getchar(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    // Find TTY device for blocking input
    let manager = DeviceManager::get_manager();
    if let Some(borrowed_device) = manager.get_device_by_name("tty0") {
        if let Some(char_device) = borrowed_device.as_char_device() {
            // Check if data is available
            if let Some(byte) = char_device.read_byte() {
                return byte as usize;
            }
        }
    }

    0 // Return 0 if no device found (should not happen)
}

pub fn sys_exit(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    task.vcpu.store(trapframe);
    let exit_code = trapframe.get_arg(0) as i32;
    task.exit(exit_code);
    usize::MAX // -1 (If exit is successful, this will not be reached)
}

pub fn sys_clone(trapframe: &mut Trapframe) -> usize {
    let parent_task = mytask().unwrap();
    trapframe.increment_pc_next(parent_task); /* Increment the program counter */
    /* Save the trapframe to the task before cloning */
    parent_task.vcpu.store(trapframe);
    let clone_flags = CloneFlags::from_raw(trapframe.get_arg(0) as u64);
    let child_stack = trapframe.get_arg(1); // Second argument: child stack pointer
    let child_fn = trapframe.get_arg(2); // Third argument: function pointer (trampoline)
    let child_arg = trapframe.get_arg(3); // Fourth argument: argument to pass to function (closure pointer)

    // crate::println!("[CLONE] Parent task {} cloning with flags: 0x{:x}", parent_task.get_id(), clone_flags.get_raw());

    /* Clone the task */
    match parent_task.clone_task(clone_flags) {
        Ok(mut child_task) => {
            // Kernel internals use global task IDs, but syscalls should expose namespace-local IDs.
            let child_global_id = child_task.get_id();
            let child_ns_pid = child_task.get_namespace_id();
            // crate::println!("[CLONE] Successfully created child task {}, state: {:?}, PC: 0x{:x}",
            //     child_id, child_task.get_state(), child_task.vcpu.get_pc());
            child_task.vcpu.iregs.set_return_value(0); /* Set the return value to 0 in the child task */

            // If child_stack is provided, set child's user SP
            if child_stack != 0 {
                child_task.vcpu.set_sp(child_stack);
            }

            // If child_fn is provided, set it as PC (thread entry point)
            if child_fn != 0 {
                child_task.vcpu.set_pc(child_fn as u64);
            }

            // If child_arg is provided, pass it as first argument (a0/x0)
            if child_arg != 0 {
                child_task.vcpu.iregs.set_arg(0, child_arg);
            }

            get_scheduler().add_task(child_task, get_cpu().get_cpuid());
            // crate::println!("[CLONE] Child task {} added to scheduler", child_id);
            /* Return the child task PID (namespace-local) to the parent task */
            let _ = child_global_id;
            child_ns_pid
        }
        Err(_) => {
            usize::MAX /* Return -1 on error */
        }
    }
}

pub fn sys_execve(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();

    // crate::println!("[EXECVE] Task {} starting execve", task.get_id());

    // Increment PC to avoid infinite loop if execve fails
    trapframe.increment_pc_next(task);

    // Get arguments from trapframe
    let path_ptr = trapframe.get_arg(0);
    let argv_ptr = trapframe.get_arg(1);
    let envp_ptr = trapframe.get_arg(2);
    let flags = trapframe.get_arg(3); // New flags argument

    // Parse path
    let path_str = match parse_c_string_from_userspace(task, path_ptr, MAX_PATH_LENGTH) {
        Ok(path) => {
            // crate::println!("[EXECVE] Task {}: Executing path: {}", task.get_id(), path);
            path
        }
        Err(_) => {
            // crate::println!("[EXECVE] Task {}: Path parsing error", task.get_id());
            return usize::MAX; // Path parsing error
        }
    };

    // Parse argv and envp
    let argv_strings =
        match parse_string_array_from_userspace(task, argv_ptr, MAX_ARG_COUNT, MAX_PATH_LENGTH) {
            Ok(args) => {
                // crate::println!("[EXECVE] Task {}: argv count: {}", task.get_id(), args.len());
                args
            }
            Err(_) => {
                // crate::println!("[EXECVE] Task {}: argv parsing error", task.get_id());
                return usize::MAX; // argv parsing error
            }
        };

    let envp_strings =
        match parse_string_array_from_userspace(task, envp_ptr, MAX_ARG_COUNT, MAX_PATH_LENGTH) {
            Ok(env) => {
                // crate::println!("[EXECVE] Task {}: envp count: {}", task.get_id(), env.len());
                env
            }
            Err(_) => {
                // crate::println!("[EXECVE] Task {}: envp parsing error", task.get_id());
                return usize::MAX; // envp parsing error
            }
        };

    // Convert Vec<String> to Vec<&str> for TransparentExecutor
    let argv_refs: Vec<&str> = argv_strings.iter().map(|s| s.as_str()).collect();
    let envp_refs: Vec<&str> = envp_strings.iter().map(|s| s.as_str()).collect();

    // Check if force ABI rebuild is requested
    let force_abi_rebuild = (flags & EXECVE_FORCE_ABI_REBUILD) != 0;

    // crate::println!("[EXECVE] Task {}: Starting TransparentExecutor::execute_binary", task.get_id());

    // Use TransparentExecutor for cross-ABI execution
    match TransparentExecutor::execute_binary(
        &path_str,
        &argv_refs,
        &envp_refs,
        task,
        trapframe,
        force_abi_rebuild,
    ) {
        Ok(_) => {
            // crate::println!("[EXECVE] Task {}: execute_binary succeeded", task.get_id());
            // execve normally should not return on success - the process is replaced
            // However, if ABI module sets trapframe return value and returns here,
            // we should respect that value instead of hardcoding 0
            trapframe.get_return_value()
        }
        Err(e) => {
            crate::println!(
                "[EXECVE] Task {}: execute_binary failed for path='{}': {}",
                task.get_id(),
                path_str,
                e
            );
            // Execution failed - return error code
            // The trap handler will automatically set trapframe return value from our return
            usize::MAX // Error return value
        }
    }
}

pub fn sys_execve_abi(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();

    // Increment PC to avoid infinite loop if execve fails
    trapframe.increment_pc_next(task);

    // Get arguments from trapframe
    let path_ptr = trapframe.get_arg(0);
    let argv_ptr = trapframe.get_arg(1);
    let envp_ptr = trapframe.get_arg(2);
    let abi_str_ptr = trapframe.get_arg(3);
    let flags = trapframe.get_arg(4); // New flags argument

    // Parse path
    let path_str = match parse_c_string_from_userspace(task, path_ptr, MAX_PATH_LENGTH) {
        Ok(path) => path,
        Err(_) => return usize::MAX, // Path parsing error
    };

    // Parse ABI string
    let abi_str = match parse_c_string_from_userspace(task, abi_str_ptr, MAX_ABI_LENGTH) {
        Ok(abi) => abi,
        Err(_) => return usize::MAX, // ABI parsing error
    };

    // Parse argv and envp
    let argv_strings = match parse_string_array_from_userspace(task, argv_ptr, 256, MAX_PATH_LENGTH)
    {
        Ok(args) => args,
        Err(_) => return usize::MAX, // argv parsing error
    };

    let envp_strings = match parse_string_array_from_userspace(task, envp_ptr, 256, MAX_PATH_LENGTH)
    {
        Ok(env) => env,
        Err(_) => return usize::MAX, // envp parsing error
    };

    // Convert Vec<String> to Vec<&str> for TransparentExecutor
    let argv_refs: Vec<&str> = argv_strings.iter().map(|s| s.as_str()).collect();
    let envp_refs: Vec<&str> = envp_strings.iter().map(|s| s.as_str()).collect();

    // Check if force ABI rebuild is requested
    let force_abi_rebuild = (flags & EXECVE_FORCE_ABI_REBUILD) != 0;

    // Use TransparentExecutor for ABI-aware execution
    match TransparentExecutor::execute_with_abi(
        &path_str,
        &argv_refs,
        &envp_refs,
        &abi_str,
        task,
        trapframe,
        force_abi_rebuild,
    ) {
        Ok(()) => {
            // execve normally should not return on success - the process is replaced
            // However, if ABI module sets trapframe return value and returns here,
            // we should respect that value instead of hardcoding 0
            trapframe.get_return_value()
        }
        Err(_) => {
            // Execution failed - return error code
            // The trap handler will automatically set trapframe return value from our return
            usize::MAX // Error return value
        }
    }
}

pub fn sys_waitpid(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    // pid is namespace-local PID as seen by the calling task.
    let pid = trapframe.get_arg(0) as i32;
    let status_ptr = trapframe.get_arg(1) as *mut i32;
    let options = trapframe.get_arg(2) as i32;

    // WNOHANG flag (0x1): Return immediately if no child has exited
    let wnohang = (options & 0x1) != 0;

    // Loop until a child exits or an error occurs
    loop {
        if pid == -1 {
            // Wait for any child process
            for child_pid in task.get_children().clone() {
                match task.wait(child_pid) {
                    Ok(status) => {
                        // Child has exited, return the status
                        if status_ptr != core::ptr::null_mut() {
                            let status_ptr = task
                                .vm_manager
                                .translate_vaddr(status_ptr as usize)
                                .unwrap() as *mut i32;
                            unsafe {
                                *status_ptr = status;
                            }
                        }
                        trapframe.increment_pc_next(task);
                        // Return child's PID in caller's namespace (if visible)
                        if let Some(local) = task.get_namespace().resolve_local_id(child_pid) {
                            return local;
                        }
                        // Not visible in this namespace; keep searching
                        continue;
                    }
                    Err(error) => match error {
                        WaitError::ChildNotExited(_) => continue,
                        _ => {
                            trapframe.increment_pc_next(task);
                            return usize::MAX;
                        }
                    },
                }
            }

            // No child has exited yet
            if wnohang {
                // WNOHANG: Return immediately without blocking
                trapframe.increment_pc_next(task);
                return 0; // Return 0 to indicate no child has exited
            }

            // Block until a child exits
            let parent_waker = get_parent_waitpid_waker(task.get_id());
            parent_waker.wait(task.get_id(), trapframe);
            // Continue the loop to re-check after waking up
            continue;
        }

        // Wait for specific child process
        if pid <= 0 {
            trapframe.increment_pc_next(task);
            return usize::MAX;
        }

        let target_global = match task.get_namespace().resolve_global_id(pid as usize) {
            Some(g) => g,
            None => {
                trapframe.increment_pc_next(task);
                return usize::MAX;
            }
        };

        match task.wait(target_global) {
            Ok(status) => {
                // Child has exited, return the status
                if status_ptr != core::ptr::null_mut() {
                    let status_ptr = task
                        .vm_manager
                        .translate_vaddr(status_ptr as usize)
                        .unwrap() as *mut i32;
                    unsafe {
                        *status_ptr = status;
                    }
                }
                trapframe.increment_pc_next(task);
                return pid as usize;
            }
            Err(error) => {
                match error {
                    WaitError::NoSuchChild(_) => {
                        trapframe.increment_pc_next(task);
                        return usize::MAX;
                    }
                    WaitError::ChildTaskNotFound(_) => {
                        trapframe.increment_pc_next(task);
                        crate::print!("Child task with PID {} not found", pid);
                        return usize::MAX;
                    }
                    WaitError::ChildNotExited(_) => {
                        // Child has not exited yet
                        if wnohang {
                            // WNOHANG: Return immediately without blocking
                            trapframe.increment_pc_next(task);
                            return 0; // Return 0 to indicate child has not exited
                        }

                        // Block until child exits
                        let child_waker = get_waitpid_waker(target_global);
                        child_waker.wait(task.get_id(), trapframe);
                        assert_eq!(mytask().unwrap().get_id(), task.get_id());
                        // Continue the loop to re-check after waking up
                        continue;
                    }
                }
            }
        }
    }
}

pub fn sys_getpid(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    // Expose namespace-local task ID to user space.
    // This allows task namespaces (PID namespaces) to provide independent PID spaces.
    task.get_namespace_id() as usize
}

pub fn sys_getppid(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    // Return parent's PID as seen from the caller's namespace.
    // If the parent is not mapped/visible in this namespace, return 0.
    match task.get_parent_id() {
        Some(parent_global) => task
            .get_namespace()
            .resolve_local_id(parent_global)
            .unwrap_or(0),
        None => 0,
    }
}

pub fn sys_sleep(trapframe: &mut Trapframe) -> usize {
    let nanosecs = trapframe.get_arg(0) as u64;
    let task = mytask().unwrap();

    let ticks = ns_to_ticks(nanosecs);

    // Increment PC before sleeping to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Call the blocking sleep method - this will return when sleep completes
    task.sleep(trapframe, ticks);

    // Set return value to 0 for successful sleep
    0
}

/// Yield execution to the scheduler
///
/// This is a cooperative scheduling primitive similar to `sched_yield(2)`.
/// The calling task remains runnable, but allows another ready task to run.
///
/// # Returns
/// * `0` on success
pub fn sys_yield(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();

    // Increment PC before yielding to avoid re-executing the syscall on resume
    trapframe.increment_pc_next(task);

    // Yield CPU to scheduler - returns when this task is scheduled again
    get_scheduler().schedule(trapframe);

    0
}

/// Register an ABI zone for a specific memory range
///
/// # Arguments
/// * `start` - Start address of the memory range
/// * `len` - Length of the memory range in bytes
/// * `abi_name_ptr` - Pointer to null-terminated ABI name string in user space
///
/// # Returns
/// * `0` on success
/// * `usize::MAX` (-1) on failure
pub fn sys_register_abi_zone(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let start = trapframe.get_arg(0);
    let len = trapframe.get_arg(1);
    let abi_name_ptr = trapframe.get_arg(2);

    trapframe.increment_pc_next(task);

    // Parse the ABI name from user space
    let abi_name = match parse_c_string_from_userspace(task, abi_name_ptr, MAX_ABI_LENGTH) {
        Ok(name) => name,
        Err(_) => {
            crate::early_println!("[syscall] Failed to parse ABI name from user space");
            return usize::MAX; // -1
        }
    };

    crate::early_println!(
        "[syscall] Registering ABI zone: start={:#x}, len={:#x}, abi={}",
        start,
        len,
        abi_name
    );

    // Instantiate the ABI module
    let abi = match crate::abi::AbiRegistry::instantiate(&abi_name) {
        Some(abi) => abi,
        None => {
            crate::early_println!("[syscall] ABI '{}' not found in registry", abi_name);
            return usize::MAX; // -1
        }
    };

    // Create the ABI zone
    let zone = crate::task::AbiZone {
        range: start..(start + len),
        abi,
    };

    // Insert into the task's ABI zones map
    task.abi_zones.insert(start, zone);

    crate::early_println!("[syscall] Successfully registered ABI zone");
    0
}

/// Unregister an ABI zone
///
/// # Arguments
/// * `start` - Start address of the memory range to unregister
///
/// # Returns
/// * `0` on success
/// * `usize::MAX` (-1) on failure (zone not found)
pub fn sys_unregister_abi_zone(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let start = trapframe.get_arg(0);

    trapframe.increment_pc_next(task);

    crate::early_println!("[syscall] Unregistering ABI zone at start={:#x}", start);

    // Remove the ABI zone from the map
    match task.abi_zones.remove(&start) {
        Some(_) => {
            crate::early_println!("[syscall] Successfully unregistered ABI zone");
            0
        }
        None => {
            crate::early_println!("[syscall] ABI zone not found at start={:#x}", start);
            usize::MAX // -1
        }
    }
}

// Namespace creation flags (bit flags for smart control)
pub const NS_CREATE_TASK: usize = 0x01; // Create separate task namespace
pub const NS_CREATE_VFS: usize = 0x02; // Create separate VFS namespace
pub const NS_CREATE_NET: usize = 0x04; // Create separate network namespace (future)
pub const NS_CREATE_IPC: usize = 0x08; // Create separate IPC namespace (future)

// Syscall error return value
const SYSCALL_ERROR: usize = usize::MAX;

/// Create a new namespace for the current task (Scarlet-style smart syscall)
///
/// # Arguments
/// * `flags` - Bitfield specifying which namespaces to create (NS_CREATE_*)
/// * `name_ptr` - Pointer to C string with namespace name (optional, can be null)
///
/// # Returns
/// * `0` on success
/// * `SYSCALL_ERROR` (-1) on failure
///
/// # Example
/// ```rust
/// // Create separate task and VFS namespaces
/// sys_create_namespace(NS_CREATE_TASK | NS_CREATE_VFS, "container1");
/// ```
pub fn sys_create_namespace(trapframe: &mut Trapframe) -> usize {
    use crate::fs::VfsManager;
    use crate::task::namespace::TaskNamespace;

    let task = mytask().unwrap();
    let flags = trapframe.get_arg(0);
    let name_ptr = trapframe.get_arg(1);

    trapframe.increment_pc_next(task);

    // Parse namespace name (optional)
    let name = if name_ptr == 0 {
        alloc::format!("ns_{}", task.get_id())
    } else {
        match parse_c_string_from_userspace(task, name_ptr, 64) {
            Ok(s) => s,
            Err(_) => {
                crate::early_println!("[syscall] Failed to parse namespace name");
                return SYSCALL_ERROR;
            }
        }
    };

    crate::early_println!(
        "[syscall] Creating namespace '{}' with flags={:#x}",
        name,
        flags
    );

    // Create task namespace if requested
    if flags & NS_CREATE_TASK != 0 {
        let new_task_ns = TaskNamespace::new_child(task.get_namespace().clone(), name.clone());
        task.set_namespace(new_task_ns);
        crate::early_println!("[syscall] Created task namespace '{}'", name);
    }

    // Create VFS namespace if requested
    if flags & NS_CREATE_VFS != 0 {
        // Deep-clone the current mount topology so the initial view is the same,
        // but future mount operations are isolated.
        let source_vfs = match task.get_vfs() {
            Some(vfs) => vfs,
            None => return SYSCALL_ERROR,
        };

        let new_vfs = match VfsManager::clone_mount_namespace_deep(&source_vfs) {
            Ok(vfs) => vfs,
            Err(e) => {
                crate::early_println!(
                    "[syscall] Failed to clone VFS namespace '{}': {}",
                    name,
                    e.message
                );
                return SYSCALL_ERROR;
            }
        };

        // Preserve current working directory when possible.
        let cwd_path = source_vfs.get_cwd_path();
        let _ = new_vfs.set_cwd_by_path(&cwd_path);

        task.set_vfs(new_vfs);
        crate::early_println!("[syscall] Created VFS namespace '{}'", name);
    }

    // Future: Network namespace
    if flags & NS_CREATE_NET != 0 {
        crate::early_println!("[syscall] Network namespace not yet implemented");
    }

    // Future: IPC namespace
    if flags & NS_CREATE_IPC != 0 {
        crate::early_println!("[syscall] IPC namespace not yet implemented");
    }

    0
}
