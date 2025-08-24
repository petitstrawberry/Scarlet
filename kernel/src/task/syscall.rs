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
use crate::library::std::string::{parse_c_string_from_userspace, parse_string_array_from_userspace};

use crate::arch::{get_cpu, Trapframe};
use crate::sched::scheduler::get_scheduler;
use crate::task::{get_parent_waitpid_waker, get_waitpid_waker, CloneFlags, WaitError};
use crate::timer::{get_tick, ms_to_ticks, ns_to_ticks};

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

    // crate::println!("[CLONE] Parent task {} cloning with flags: 0x{:x}", parent_task.get_id(), clone_flags.get_raw());

    /* Clone the task */
    match parent_task.clone_task(clone_flags) {
        Ok(mut child_task) => {
            let child_id = child_task.get_id();
            // crate::println!("[CLONE] Successfully created child task {}, state: {:?}, PC: 0x{:x}", 
            //     child_id, child_task.get_state(), child_task.vcpu.get_pc());
            child_task.vcpu.regs.reg[10] = 0; /* Set the return value to 0 in the child task */
            get_scheduler().add_task(child_task, get_cpu().get_cpuid());
            // crate::println!("[CLONE] Child task {} added to scheduler", child_id);
            /* Return the child task ID to the parent task */
            child_id
        },
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
        },
        Err(_) => {
            // crate::println!("[EXECVE] Task {}: Path parsing error", task.get_id());
            return usize::MAX; // Path parsing error
        }
    };
    
    // Parse argv and envp
    let argv_strings = match parse_string_array_from_userspace(task, argv_ptr, MAX_ARG_COUNT, MAX_PATH_LENGTH) {
        Ok(args) => {
            // crate::println!("[EXECVE] Task {}: argv count: {}", task.get_id(), args.len());
            args
        },
        Err(_) => {
            // crate::println!("[EXECVE] Task {}: argv parsing error", task.get_id());
            return usize::MAX; // argv parsing error
        }
    };
    
    let envp_strings = match parse_string_array_from_userspace(task, envp_ptr, MAX_ARG_COUNT, MAX_PATH_LENGTH) {
        Ok(env) => {
            // crate::println!("[EXECVE] Task {}: envp count: {}", task.get_id(), env.len());
            env
        },
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
    match TransparentExecutor::execute_binary(&path_str, &argv_refs, &envp_refs, task, trapframe, force_abi_rebuild) {
        Ok(_) => {
            // crate::println!("[EXECVE] Task {}: execute_binary succeeded", task.get_id());
            // execve normally should not return on success - the process is replaced
            // However, if ABI module sets trapframe return value and returns here,
            // we should respect that value instead of hardcoding 0
            trapframe.get_return_value()
        },
        Err(_) => {
            // crate::println!("[EXECVE] Task {}: execute_binary failed", task.get_id());
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
    let argv_strings = match parse_string_array_from_userspace(task, argv_ptr, 256, MAX_PATH_LENGTH) {
        Ok(args) => args,
        Err(_) => return usize::MAX, // argv parsing error
    };
    
    let envp_strings = match parse_string_array_from_userspace(task, envp_ptr, 256, MAX_PATH_LENGTH) {
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
    let pid = trapframe.get_arg(0) as i32;
    let status_ptr = trapframe.get_arg(1) as *mut i32;
    let _options = trapframe.get_arg(2) as i32; // Not used in this implementation

    // Loop until a child exits or an error occurs
    loop {
        if pid == -1 {
            // Wait for any child process
            for child_pid in task.get_children().clone() {
                match task.wait(child_pid) {
                    Ok(status) => {
                        // Child has exited, return the status
                        if status_ptr != core::ptr::null_mut() {
                            let status_ptr = task.vm_manager.translate_vaddr(status_ptr as usize).unwrap() as *mut i32;
                            unsafe {
                                *status_ptr = status;
                            }
                        }
                        trapframe.increment_pc_next(task);
                        return child_pid;
                    },
                    Err(error) => {
                        match error {
                            WaitError::ChildNotExited(_) => continue,
                            _ => {
                                trapframe.increment_pc_next(task);
                                return usize::MAX;
                            },
                        }
                    }
                }
            }
            
            // No child has exited yet, block until one does
            let parent_waker = get_parent_waitpid_waker(task.get_id());
            parent_waker.wait(task.get_id(), trapframe);
            // Continue the loop to re-check after waking up
            continue;
        }
        
        // Wait for specific child process
        match task.wait(pid as usize) {
            Ok(status) => {
                // Child has exited, return the status
                if status_ptr != core::ptr::null_mut() {
                    let status_ptr = task.vm_manager.translate_vaddr(status_ptr as usize).unwrap() as *mut i32;
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
                    },
                    WaitError::ChildTaskNotFound(_) => {
                        trapframe.increment_pc_next(task);
                        crate::print!("Child task with PID {} not found", pid);
                        return usize::MAX;
                    },
                    WaitError::ChildNotExited(_) => {
                        // If the child task is not exited, we need to wait for it
                        let child_waker = get_waitpid_waker(pid as usize);
                        child_waker.wait(task.get_id(), trapframe);
                        assert_eq!(mytask().unwrap().get_id(), task.get_id());
                        // Continue the loop to re-check after waking up
                        continue;
                    },
                }
            }
        }
    }
}

pub fn sys_getpid(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    task.get_id() as usize
}

pub fn sys_getppid(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    task.get_parent_id().unwrap_or(task.get_id()) as usize
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

