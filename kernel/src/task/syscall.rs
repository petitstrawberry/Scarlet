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
use crate::library::std::usercopy::copy_to_user;
use crate::object::capability::memory_mapping::syscall::reclaim_private_removed_mapping;

use crate::arch::Trapframe;
use crate::environment::PAGE_SIZE;
use crate::sched::scheduler::{
    cleanup_zombie, cpu_usage_snapshot, enqueue_task, get_all_task_ids, get_task_by_id,
    register_task, remove_task_from_queues, schedule,
};
use crate::task::{
    CloneFlags, CloneFlagsDef, SCHED_UTIL_SCALE, TaskState, WaitError, get_parent_waitpid_waker,
    get_waitpid_waker,
};
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
        if let Some(device) = manager.get_device_by_name("tty0")
            && let Some(char_device) = device.as_char_device()
            && char_device.can_write()
            && char_device.write_byte(ch as u8).is_ok()
        {
            return 0;
        }

        for (_name, device) in manager.get_named_devices() {
            if let Some(char_device) = device.as_char_device()
                && char_device.can_write()
                && char_device.write_byte(ch as u8).is_ok()
            {
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
    task.vcpu.lock().store(trapframe);
    let exit_code = trapframe.get_arg(0) as i32;
    task.exit(exit_code);
    usize::MAX // -1 (If exit is successful, this will not be reached)
}

fn unmap_thread_cleanup_range(task: &crate::task::Task, vaddr: usize, length: usize) {
    if length == 0 || vaddr % PAGE_SIZE != 0 {
        return;
    }

    let removed_maps = task.vm_manager.remove_memory_map_range(vaddr, length);
    for removed_map in &removed_maps {
        if let Some(owner) = &removed_map.owner {
            owner.on_unmapped(removed_map.vmarea.start, removed_map.vmarea.size());
        }
        reclaim_private_removed_mapping(task, removed_map);
    }
}

/// Exit the current thread and release libc-owned thread mappings.
///
/// This is the Scarlet equivalent of libc's final thread-exit path: the kernel
/// is already running on the task's kernel stack, so it can safely unmap the
/// user stack/TLS mappings that the exiting thread was using.
///
/// # Arguments
///
/// * `trapframe.arg(0)` - Thread exit status.
/// * `trapframe.arg(1)` - Stack mapping base.
/// * `trapframe.arg(2)` - Stack mapping length.
/// * `trapframe.arg(3)` - TLS mapping base.
/// * `trapframe.arg(4)` - TLS mapping length.
///
/// # Returns
///
/// This syscall does not return on success.
pub fn sys_thread_exit_cleanup(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    task.vcpu.lock().store(trapframe);

    let exit_code = trapframe.get_arg(0) as i32;
    let stack_mapping_base = trapframe.get_arg(1);
    let stack_mapping_len = trapframe.get_arg(2);
    let tls_mapping_base = trapframe.get_arg(3);
    let tls_mapping_len = trapframe.get_arg(4);

    task.exit_with_cleanup(exit_code, |task| {
        unmap_thread_cleanup_range(task, stack_mapping_base, stack_mapping_len);
        unmap_thread_cleanup_range(task, tls_mapping_base, tls_mapping_len);
    });
    usize::MAX
}

pub fn sys_clone(trapframe: &mut Trapframe) -> usize {
    let parent_task = mytask().unwrap();
    trapframe.increment_pc_next(parent_task); /* Increment the program counter */
    /* Save the trapframe to the task before cloning */
    parent_task.vcpu.lock().store(trapframe);
    let clone_flags = CloneFlags::from_raw(trapframe.get_arg(0) as u64);
    let child_stack = trapframe.get_arg(1); // Second argument: child stack pointer
    let child_fn = trapframe.get_arg(2); // Third argument: function pointer (trampoline)
    let child_arg = trapframe.get_arg(3); // Fourth argument: argument to pass to function (closure pointer)
    let tls_ptr = trapframe.get_arg(4); // Fifth argument: TLS pointer

    // crate::println!("[CLONE] Parent task {} cloning with flags: 0x{:x}", parent_task.get_id(), clone_flags.get_raw());

    /* Clone the task */
    match parent_task.clone_task(clone_flags) {
        Ok(child_task) => {
            // crate::println!("[CLONE] Successfully created child task {}, state: {:?}, PC: 0x{:x}",
            //     child_id, child_task.get_state(), child_task.vcpu.get_pc());
            child_task.vcpu.lock().iregs.set_return_value(0); /* Set the return value to 0 in the child task */

            // If child_stack is provided, set child's user SP
            if child_stack != 0 {
                child_task.vcpu.lock().set_sp(child_stack);
            }

            // If child_fn is provided, set it as PC (thread entry point)
            if child_fn != 0 {
                child_task.vcpu.lock().set_pc(child_fn as u64);
            }

            // If child_arg is provided, pass it as first argument (a0/x0)
            if child_arg != 0 {
                child_task.vcpu.lock().iregs.set_arg(0, child_arg);
            }

            let parent_id = parent_task.get_id();

            // Handle SetTls flag: set TLS pointer and tp register
            if clone_flags.is_set(CloneFlagsDef::SetTls) {
                // Set TLS pointer in task's ABI state
                // SAFETY: Child task is not yet visible to scheduler
                unsafe {
                    if let Some(abi) = child_task.default_abi.get_mut().as_mut() {
                        abi.set_tls_pointer(tls_ptr);
                    }
                }

                // Set TLS pointer using architecture-specific VCPU method
                child_task.vcpu.lock().set_tls_pointer(tls_ptr);
            }

            // Register the child first, finish parent/child metadata, then enqueue it.
            // On SMP, enqueueing before the metadata is complete lets a remote CPU
            // start the child immediately from the reschedule IPI.
            let cpu_id = crate::sched::scheduler::select_cpu_for_task(&child_task);
            let child_id = register_task(child_task);
            // crate::println!("[CLONE] Child task {} added to scheduler", child_id);

            // Establish parent-child relationship now that both have valid IDs
            if let Some(child) = get_task_by_id(child_id) {
                child.set_parent_id(parent_id);
            }
            if let Some(parent) = get_task_by_id(parent_id) {
                parent.add_child(child_id);
            }

            // Get the child's namespace-local PID (after add_task has set the IDs)
            let child_ns_pid = get_task_by_id(child_id)
                .map(|t| t.get_namespace_id())
                .unwrap_or(0);

            enqueue_task(child_id, cpu_id);

            /* Return the child task PID (namespace-local) to the parent task */
            child_ns_pid
        }
        Err(err) => {
            crate::println!(
                "[clone] failed: parent={} name={} flags={:#x} reason={}",
                parent_task.get_id(),
                parent_task.name.read().as_str(),
                clone_flags.get_raw(),
                err
            );
            usize::MAX /* Return -1 on error */
        }
    }
}

/// Detach a child thread from the caller's wait set.
///
/// Dropping a user-space JoinHandle calls this syscall. Detached threads must
/// not remain as zombies owned by the spawning thread; if the thread already
/// exited, it is reaped immediately.
///
/// # Arguments
///
/// * `trapframe.arg(0)` - Namespace-local thread ID to detach.
///
/// # Returns
///
/// 0 on success, or `usize::MAX` on failure.
pub fn sys_thread_detach(trapframe: &mut Trapframe) -> usize {
    let caller = mytask().unwrap();
    let local_thread_id = trapframe.get_arg(0);
    trapframe.increment_pc_next(caller);

    let target_id = match caller.get_namespace().resolve_global_id(local_thread_id) {
        Some(id) => id,
        None => return usize::MAX,
    };

    if target_id == caller.get_id() {
        return usize::MAX;
    }

    let Some(target) = get_task_by_id(target_id) else {
        return usize::MAX;
    };

    // Only allow detaching threads in the caller's thread group. This keeps the
    // syscall scoped to std::thread JoinHandle semantics, not arbitrary waitpid.
    if target.get_thread_group_id() != caller.get_thread_group_id() {
        return usize::MAX;
    }

    let Some(parent_id) = target.get_parent_id() else {
        if target.get_state() == TaskState::Zombie {
            target.set_state(TaskState::Terminated);
            remove_task_from_queues(target_id);
            cleanup_zombie(target_id);
        }
        return 0;
    };

    let Some(parent) = get_task_by_id(parent_id) else {
        target.clear_parent_id();
        if target.get_state() == TaskState::Zombie {
            target.set_state(TaskState::Terminated);
            remove_task_from_queues(target_id);
            cleanup_zombie(target_id);
        }
        return 0;
    };

    if parent_id != caller.get_id() && parent.get_thread_group_id() != caller.get_thread_group_id()
    {
        return usize::MAX;
    }

    parent.remove_child(target_id);
    target.clear_parent_id();

    if target.get_state() == TaskState::Zombie {
        target.set_state(TaskState::Terminated);
        remove_task_from_queues(target_id);
        cleanup_zombie(target_id);
    }

    0
}

/// Set the TLS pointer for the current task
pub fn sys_set_tls(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let tls_ptr = trapframe.get_arg(0);

    // Update ABI state
    // SAFETY: This is the currently executing task on this hart
    unsafe {
        if let Some(abi) = task.default_abi.get_mut().as_mut() {
            abi.set_tls_pointer(tls_ptr);
        }
    }

    // The current task's live register state is the syscall trapframe. The VCPU
    // save area is updated from the trapframe only when the scheduler stores it.
    trapframe.set_tls_pointer(tls_ptr);

    trapframe.increment_pc_next(task);
    0 // Success
}

/// Get the TLS pointer for the current task
pub fn sys_get_tls(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();

    // Get TLS pointer from ABI state
    // SAFETY: This is the currently executing task on this hart
    let tls_ptr = unsafe {
        task.default_abi
            .get()
            .as_ref()
            .and_then(|abi| abi.get_tls_pointer())
            .unwrap_or(0)
    };

    trapframe.increment_pc_next(task);
    tls_ptr // Return TLS pointer
}

/// Set the clear_child_tid pointer for thread exit notification
pub fn sys_set_tid_address(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let tid_ptr = trapframe.get_arg(0);

    // Update ABI state
    // SAFETY: This is the currently executing task on this hart
    unsafe {
        if let Some(abi) = task.default_abi.get_mut().as_mut() {
            abi.set_clear_child_tid(tid_ptr);
        }
    }

    trapframe.increment_pc_next(task);
    task.get_namespace_id() // Return current TID (Linux-compatible)
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
    let status_addr = trapframe.get_arg(1);
    let options = trapframe.get_arg(2) as i32;

    // WNOHANG flag (0x1): Return immediately if no child has exited
    let wnohang = (options & 0x1) != 0;
    // WUNTRACED-like native flag (0x2): report process-control stops.
    let report_stopped = (options & 0x2) != 0;
    const PROCESS_CONTROL_STOP_STATUS: i32 = 0x7f;

    // Loop until a child exits or an error occurs
    loop {
        if pid == -1 {
            // Wait for any child process
            for child_pid in task.get_children().clone() {
                if report_stopped
                    && let Some(child_task) = crate::sched::scheduler::get_task_by_id(child_pid)
                    && child_task.get_state() != TaskState::Zombie
                    && child_task.take_process_control_stop_report()
                {
                    if status_addr != 0
                        && copy_to_user(
                            task,
                            status_addr,
                            &PROCESS_CONTROL_STOP_STATUS.to_ne_bytes(),
                        )
                        .is_err()
                    {
                        trapframe.increment_pc_next(task);
                        return usize::MAX;
                    }
                    trapframe.increment_pc_next(task);
                    if let Some(local) = task.get_namespace().resolve_local_id(child_pid) {
                        return local;
                    }
                    continue;
                }

                match task.wait(child_pid) {
                    Ok(status) => {
                        // Child has exited, return the status
                        if status_addr != 0
                            && copy_to_user(task, status_addr, &status.to_ne_bytes()).is_err()
                        {
                            trapframe.increment_pc_next(task);
                            return usize::MAX;
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

        if report_stopped
            && task.get_children().contains(&target_global)
            && let Some(child_task) = crate::sched::scheduler::get_task_by_id(target_global)
            && child_task.get_state() != TaskState::Zombie
            && child_task.take_process_control_stop_report()
        {
            if status_addr != 0
                && copy_to_user(
                    task,
                    status_addr,
                    &PROCESS_CONTROL_STOP_STATUS.to_ne_bytes(),
                )
                .is_err()
            {
                trapframe.increment_pc_next(task);
                return usize::MAX;
            }
            trapframe.increment_pc_next(task);
            return pid as usize;
        }

        match task.wait(target_global) {
            Ok(status) => {
                // Child has exited, return the status
                if status_addr != 0
                    && copy_to_user(task, status_addr, &status.to_ne_bytes()).is_err()
                {
                    trapframe.increment_pc_next(task);
                    return usize::MAX;
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

/// Create a new session for the current task.
///
/// # Arguments
///
/// None.
///
/// # Returns
///
/// Namespace-local session ID on success, or `usize::MAX` on failure.
pub fn sys_create_session(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    match task.create_session() {
        Ok(session_id) => task
            .get_namespace()
            .resolve_local_id(session_id)
            .unwrap_or(session_id),
        Err(_) => usize::MAX,
    }
}

/// Return a task's session ID.
///
/// # Arguments
///
/// * `trapframe.get_arg(0)` - Namespace-local task ID, or 0 for current task.
///
/// # Returns
///
/// Namespace-local session ID on success, or `usize::MAX` on failure.
pub fn sys_get_session_id(trapframe: &mut Trapframe) -> usize {
    let caller = mytask().unwrap();
    let pid = trapframe.get_arg(0);
    trapframe.increment_pc_next(caller);

    let namespace = caller.get_namespace();
    let target_global_id = if pid == 0 {
        caller.get_id()
    } else {
        match namespace.resolve_global_id(pid) {
            Some(id) => id,
            None => return usize::MAX,
        }
    };

    let Some(target) = get_task_by_id(target_global_id) else {
        return usize::MAX;
    };

    namespace
        .resolve_local_id(target.get_session_id())
        .unwrap_or(target.get_session_id())
}

/// Return a task's process group ID.
///
/// # Arguments
///
/// * `trapframe.get_arg(0)` - Namespace-local task ID, or 0 for current task.
///
/// # Returns
///
/// Namespace-local process group ID on success, or `usize::MAX` on failure.
pub fn sys_get_process_group_id(trapframe: &mut Trapframe) -> usize {
    let caller = mytask().unwrap();
    let pid = trapframe.get_arg(0);
    trapframe.increment_pc_next(caller);

    let namespace = caller.get_namespace();
    let target_global_id = if pid == 0 {
        caller.get_id()
    } else {
        match namespace.resolve_global_id(pid) {
            Some(id) => id,
            None => return usize::MAX,
        }
    };

    let Some(target) = get_task_by_id(target_global_id) else {
        return usize::MAX;
    };

    namespace
        .resolve_local_id(target.get_process_group_id())
        .unwrap_or(target.get_process_group_id())
}

/// Set a task's process group.
///
/// # Arguments
///
/// * `trapframe.get_arg(0)` - Namespace-local task ID, or 0 for current task.
/// * `trapframe.get_arg(1)` - Namespace-local process group ID, or 0 to use
///   the target task ID as its process group.
///
/// # Returns
///
/// 0 on success, or `usize::MAX` on failure.
pub fn sys_set_process_group(trapframe: &mut Trapframe) -> usize {
    let caller = mytask().unwrap();
    let pid = trapframe.get_arg(0);
    let pgid = trapframe.get_arg(1);
    trapframe.increment_pc_next(caller);

    let namespace = caller.get_namespace();
    let target_global_id = if pid == 0 {
        caller.get_id()
    } else {
        match namespace.resolve_global_id(pid) {
            Some(id) => id,
            None => return usize::MAX,
        }
    };

    let Some(target) = get_task_by_id(target_global_id) else {
        return usize::MAX;
    };

    if target.get_id() != caller.get_id() && target.get_parent_id() != Some(caller.get_id()) {
        return usize::MAX;
    }
    if target.get_session_id() != caller.get_session_id() || target.is_session_leader() {
        return usize::MAX;
    }

    let new_pgid = if pgid == 0 {
        target.get_id()
    } else {
        match namespace.resolve_global_id(pgid) {
            Some(id) => id,
            None => return usize::MAX,
        }
    };

    if new_pgid != target.get_id() {
        let Some(group_leader) = get_task_by_id(new_pgid) else {
            return usize::MAX;
        };
        if group_leader.get_process_group_id() != new_pgid
            || group_leader.get_session_id() != target.get_session_id()
        {
            return usize::MAX;
        }
    }

    target.set_process_group_id(new_pgid);
    0
}

/// Set the current task's minimum scheduler utilization clamp.
///
/// # Arguments
///
/// * `trapframe.arg(0)` - Minimum utilization in scheduler capacity units,
///   where [`SCHED_UTIL_SCALE`] is a full-capacity CPU.
///
/// # Returns
///
/// `0` on success, or `usize::MAX` if the utilization value is invalid.
pub fn sys_set_task_util_min(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let util_min = trapframe.get_arg(0);
    trapframe.increment_pc_next(task);

    if util_min > SCHED_UTIL_SCALE as usize {
        return usize::MAX;
    }
    task.set_sched_util_min(util_min as u32)
        .map(|()| 0)
        .unwrap_or(usize::MAX)
}

/// Return the current task's minimum scheduler utilization clamp.
///
/// # Arguments
///
/// None.
///
/// # Returns
///
/// Current minimum utilization in scheduler capacity units.
pub fn sys_get_task_util_min(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    task.sched_util_min() as usize
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

/// Read the kernel monotonic clock.
///
/// Returns boot-relative monotonic time in nanoseconds. The value is derived
/// from the platform architected timer and is suitable for elapsed-time
/// measurement across scheduler migration on supported SMP platforms.
///
/// # Returns
///
/// Current monotonic time in nanoseconds since boot.
pub fn sys_monotonic_time(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    crate::time::current_time_ns() as usize
}

/// Read the kernel wall-clock (real) time.
///
/// Returns wall-clock nanoseconds since the Unix epoch. If no RTC source has
/// initialized the wall clock yet, returns `usize::MAX` as a sentinel.
///
/// # Returns
///
/// Wall-clock nanoseconds since the Unix epoch, or `usize::MAX` if unavailable.
pub fn sys_system_time(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    match crate::time::system_time_ns() {
        Some(ns) => ns as usize,
        None => usize::MAX,
    }
}

/// Read cumulative system-wide CPU usage accounting.
///
/// # Arguments
/// * `trapframe.get_arg(0)` - pointer to a writable `CpuUsageInfo` buffer.
///
/// # Returns
/// `0` on success, or `usize::MAX` on copy failure.
pub fn sys_get_cpu_usage_info(trapframe: &mut Trapframe) -> usize {
    use crate::library::std::usercopy::copy_to_user;
    use crate::task::CpuUsageInfo;

    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let snapshot = cpu_usage_snapshot();
    let total_time_ns = snapshot.busy_time_ns.saturating_add(snapshot.idle_time_ns);
    let usage_per_mille = if total_time_ns == 0 {
        0
    } else {
        ((snapshot.busy_time_ns as u128 * 1000) / total_time_ns as u128) as u32
    };
    let info = CpuUsageInfo {
        online_cpus: snapshot.online_cpus,
        busy_time_ns: snapshot.busy_time_ns,
        idle_time_ns: snapshot.idle_time_ns,
        total_time_ns,
        usage_per_mille,
        _reserved: 0,
    };

    let info_bytes = unsafe {
        core::slice::from_raw_parts(
            &info as *const CpuUsageInfo as *const u8,
            core::mem::size_of::<CpuUsageInfo>(),
        )
    };
    match copy_to_user(task, trapframe.get_arg(0), info_bytes) {
        Ok(()) => 0,
        Err(_) => usize::MAX,
    }
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
    schedule(trapframe);

    0
}

/// Exit all tasks in the thread group
///
/// This system call terminates all tasks with the same TGID (thread group).
/// It is similar to Linux's exit_group system call and is the proper way
/// for multi-threaded processes to exit.
///
/// # Arguments
/// * `trapframe.arg(0)` - Exit status code
///
/// # Returns
/// This function does not return on success (all tasks are terminated).
/// Returns `usize::MAX` (-1) on error.
///
/// # Behavior
/// - Terminates all tasks with the same TGID as the caller
/// - The calling task is set to Zombie/Terminated
/// - Other tasks in the group are forcefully terminated
pub fn sys_exit_group(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    task.vcpu.lock().store(trapframe);
    let exit_code = trapframe.get_arg(0) as i32;
    task.exit_group(exit_code);
    usize::MAX // -1 (If exit_group is successful, this will not be reached)
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
            crate::println!("[syscall] Failed to parse ABI name from user space");
            return usize::MAX; // -1
        }
    };

    crate::println!(
        "[syscall] Registering ABI zone: start={:#x}, len={:#x}, abi={}",
        start,
        len,
        abi_name
    );

    // Instantiate the ABI module
    let abi = match crate::abi::AbiRegistry::instantiate(&abi_name) {
        Some(abi) => abi,
        None => {
            crate::println!("[syscall] ABI '{}' not found in registry", abi_name);
            return usize::MAX; // -1
        }
    };

    // Create the ABI zone
    let zone = crate::task::AbiZone {
        range: start..(start + len),
        abi,
    };

    // Insert into the task's ABI zones map
    // SAFETY: This is the currently executing task on this hart
    unsafe {
        task.abi_zones.get_mut().insert(start, zone);
    }

    crate::println!("[syscall] Successfully registered ABI zone");
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

    crate::println!("[syscall] Unregistering ABI zone at start={:#x}", start);

    // Remove the ABI zone from the map
    // SAFETY: This is the currently executing task on this hart
    let result = unsafe { task.abi_zones.get_mut().remove(&start) };
    match result {
        Some(_) => {
            crate::println!("[syscall] Successfully unregistered ABI zone");
            0
        }
        None => {
            crate::println!("[syscall] ABI zone not found at start={:#x}", start);
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
                crate::println!("[syscall] Failed to parse namespace name");
                return SYSCALL_ERROR;
            }
        }
    };

    crate::println!(
        "[syscall] Creating namespace '{}' with flags={:#x}",
        name,
        flags
    );

    // Create task namespace if requested
    if flags & NS_CREATE_TASK != 0 {
        let new_task_ns = TaskNamespace::new_child(task.get_namespace().clone(), name.clone());
        task.set_namespace(new_task_ns);
        crate::println!("[syscall] Created task namespace '{}'", name);
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
                crate::println!(
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
        crate::println!("[syscall] Created VFS namespace '{}'", name);
    }

    // Future: Network namespace
    if flags & NS_CREATE_NET != 0 {
        crate::println!("[syscall] Network namespace not yet implemented");
    }

    // Future: IPC namespace
    if flags & NS_CREATE_IPC != 0 {
        crate::println!("[syscall] IPC namespace not yet implemented");
    }

    0
}

/// System call to shutdown the system gracefully
///
/// This system call initiates a graceful shutdown sequence:
/// 1. Terminate all user tasks
/// 2. Sync all filesystems to ensure data is written to disk
/// 3. Unmount all filesystems
/// 4. Request platform shutdown via SBI (RISC-V) or PSCI (AArch64)
///
/// # Arguments
/// * `trapframe.get_arg(0)` - Shutdown type: 0 = poweroff, 1 = reboot
///
/// # Returns
/// This function does not return on success (system shuts down)
/// Returns error code on failure
#[allow(unreachable_code)]
pub fn sys_shutdown(trapframe: &mut Trapframe) -> usize {
    use crate::arch::shutdown;

    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    // Authorization: Only the init thread group id = 1 can shutdown
    // This allows any thread in the init process (including IPC threads) to shutdown
    let tgid = task.get_thread_group_id();
    if tgid != 1 {
        crate::println!("[SHUTDOWN] Rejected: tgid={}", tgid);
        return SYSCALL_ERROR;
    }

    let shutdown_type = trapframe.get_arg(0); // 0 = poweroff, 1 = reboot

    crate::println!(
        "[SHUTDOWN] Initiating graceful shutdown (type={})...",
        shutdown_type
    );

    // TODO: In the future, this should be the FINAL step after stemd has:
    // 1. Sent SIGTERM to all processes
    // 2. Waited for processes to exit gracefully (with timeout)
    // 3. Sent SIGKILL to remaining processes
    // 4. Synced filesystems explicitly
    // 5. Unmounted filesystems
    // 6. Called sys_shutdown as the last resort
    //
    // Current implementation: Force kill all tasks immediately (simpler fallback)

    // Step 1: Terminate all tasks except the current one
    // We iterate through all possible task IDs and terminate those that are running
    crate::println!("[SHUTDOWN] Step 1: Terminating all tasks...");

    let current_task_id = task.get_id();

    crate::println!("[SHUTDOWN] Dropping all tasks...");
    for task_id in 1..crate::sched::scheduler::MAX_TASKS {
        if task_id == current_task_id {
            continue;
        }
        remove_task_from_queues(task_id);
        if let Some(task) = crate::sched::scheduler::get_task_pool().remove_task(task_id) {
            drop(task);
        }
    }

    crate::println!("[SHUTDOWN] Step 2: Enumerating mounted filesystems (no sync support yet)...");

    // Step 2: Enumerate all filesystems for logging/diagnostics.
    // NOTE: Actual sync/flush-to-disk semantics are not yet implemented here.
    //       This step does NOT guarantee that all pending data is durable.
    // Enumerate filesystems from the global VFS manager first
    if let Some(vfs) = crate::fs::manager::get_global_vfs_manager_safe() {
        let mounted_fs = vfs.mounted_filesystems.read();
        for fs in mounted_fs.iter() {
            crate::println!("[SHUTDOWN] Global filesystem: {}", fs.name());
        }
    }

    // Enumerate task-specific filesystems
    if let Some(vfs) = task.get_vfs() {
        let mounted_fs = vfs.mounted_filesystems.read();
        for fs in mounted_fs.iter() {
            crate::println!("[SHUTDOWN] Task filesystem: {}", fs.name());
        }
    }

    crate::println!(
        "[SHUTDOWN] Step 3: Filesystem unmount phase (not yet implemented, skipping)..."
    );

    // NOTE: Actual filesystem unmount logic is not yet implemented.
    // The shutdown sequence currently enumerates all known filesystems
    // (global and task-specific) but leaves the mounts in place. This is
    // sufficient for the current platforms where the underlying firmware
    // or hypervisor tears down any remaining state on poweroff/reboot.
    // When proper unmount support is added to the VFS layer, it should be
    // invoked from here in a dependency-safe order (e.g. leaves first).

    crate::println!("[SHUTDOWN] Step 4: Requesting platform shutdown...");

    // Step 4: Platform shutdown
    match shutdown_type {
        0 => {
            // Power off
            crate::println!("[SHUTDOWN] Powering off...");
            shutdown();
        }
        1 => {
            // Reboot
            crate::println!("[SHUTDOWN] Rebooting...");
            crate::arch::reboot();
        }
        _ => {
            crate::println!("[SHUTDOWN] Invalid shutdown type, defaulting to poweroff");
            shutdown();
        }
    }

    // This line should never be reached if shutdown succeeds
    crate::println!("[SHUTDOWN] ERROR: Shutdown did not complete!");
    usize::MAX
}

/// Return the number of tasks currently visible to the caller.
///
/// # Arguments
/// None.
///
/// # Returns
/// The number of tasks in the system.
pub fn sys_get_task_info_count(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    get_all_task_ids().len()
}

/// Populate a user-supplied buffer with [`TaskInfo`] snapshots.
///
/// # Arguments
/// * `trapframe.get_arg(0)` — pointer to a user buffer of `TaskInfo` slots.
/// * `trapframe.get_arg(1)` — capacity of the buffer (number of slots).
///
/// # Returns
/// The number of `TaskInfo` entries actually written.  If the buffer is
/// smaller than the total number of tasks, only the first `capacity` entries
/// are written and the return value equals `capacity`.  The caller can
/// compare against `GetTaskInfoCount` to detect truncation.
pub fn sys_get_task_info_list(trapframe: &mut Trapframe) -> usize {
    use crate::library::std::usercopy::copy_to_user;
    use crate::task::TaskInfo;
    use core::sync::atomic::Ordering;

    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let buf_ptr = trapframe.get_arg(0);
    let capacity = trapframe.get_arg(1);
    let caller_namespace = task.get_namespace();

    let ids = get_all_task_ids();
    let count = core::cmp::min(capacity, ids.len());
    let now_ns = crate::time::current_time_ns();
    let mut written = 0;

    for task_id in ids.iter().take(count) {
        let Some(target) = get_task_by_id(*task_id) else {
            continue;
        };

        // Resolve namespace-local PID/PPID.
        let pid = caller_namespace.resolve_local_id(*task_id).unwrap_or(0);
        let ppid = match target.get_parent_id() {
            Some(parent_global) => caller_namespace
                .resolve_local_id(parent_global)
                .unwrap_or(0),
            None => 0,
        };

        let state_u8 = target.state.load(Ordering::SeqCst).to_u8();
        let task_type_u8 = match target.task_type {
            super::TaskType::Kernel => 0,
            super::TaskType::User => 1,
        };
        let last_cpu = target.last_cpu.load(Ordering::SeqCst);
        let cpu_id = u8::try_from(last_cpu).unwrap_or(u8::MAX);

        let exit_status = target.exit_status.load(Ordering::SeqCst);

        let tgid = caller_namespace
            .resolve_local_id(target.get_thread_group_id())
            .unwrap_or(0);

        let mut name = [0u8; 64];
        {
            let task_name = target.name.read();
            let bytes = task_name.as_bytes();
            let len = core::cmp::min(bytes.len(), TaskInfo::NAME_CAP);
            name[..len].copy_from_slice(&bytes[..len]);
            // name[len] is already 0
        }

        let info = TaskInfo {
            pid,
            ppid,
            state: state_u8,
            task_type: task_type_u8,
            cpu_id,
            _reserved: 0,
            exit_status,
            tgid,
            name,
            cpu_time_ns: target.cpu_time_snapshot_ns(now_ns),
            sched_util_avg: target.sched_util_avg_snapshot(now_ns),
            sched_util_min: target.sched_util_min(),
            sched_required_capacity: crate::sched::scheduler::task_required_capacity_snapshot(
                target, now_ns,
            ),
            core_preference: target.core_preference().to_u8(),
            _reserved2: [0; 3],
            sched_migration_count: target.sched_migration_count(),
        };

        let info_bytes = unsafe {
            core::slice::from_raw_parts(
                &info as *const TaskInfo as *const u8,
                core::mem::size_of::<TaskInfo>(),
            )
        };
        let dest = buf_ptr + written * core::mem::size_of::<TaskInfo>();
        // Best-effort: skip on copy error.
        if copy_to_user(task, dest, info_bytes).is_ok() {
            written += 1;
        }
    }

    written
}
