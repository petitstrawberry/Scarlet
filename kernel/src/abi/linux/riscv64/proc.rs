use crate::{
    abi::linux::riscv64::LinuxRiscv64Abi, arch::{get_cpu, Trapframe}, sched::scheduler::get_scheduler, task::{mytask, CloneFlags}
};

// /// VFS v2 helper function for path absolutization
// /// TODO: Move this to a shared helper module when VFS v2 provides public API
// fn to_absolute_path_v2(task: &crate::task::Task, path: &str) -> Result<String, ()> {
//     if path.starts_with('/') {
//         Ok(path.to_string())
//     } else {
//         let cwd = task.cwd.clone().ok_or(())?;
//         let mut absolute_path = cwd;
//         if !absolute_path.ends_with('/') {
//             absolute_path.push('/');
//         }
//         absolute_path.push_str(path);
//         // Simple normalization (removes "//", ".", etc.)
//         let mut components = alloc::vec::Vec::new();
//         for comp in absolute_path.split('/') {
//             match comp {
//                 "" | "." => {},
//                 ".." => { components.pop(); },
//                 _ => components.push(comp),
//             }
//         }
//         Ok("/".to_string() + &components.join("/"))
//     }
// }

// /// Helper function to replace the missing get_path_str function
// /// TODO: This should be moved to a shared helper when VFS v2 provides public API
// fn get_path_str_v2(ptr: *const u8) -> Result<String, ()> {
//     const MAX_PATH_LENGTH: usize = 128;
//     cstring_to_string(ptr, MAX_PATH_LENGTH).map(|(s, _)| s).map_err(|_| ())
// }

// pub fn sys_fork(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
//     let parent_task = mytask().unwrap();
    
//     trapframe.increment_pc_next(parent_task); /* Increment the program counter */

//     /* Save the trapframe to the task before cloning */
//     parent_task.vcpu.store(trapframe);
    
//     /* Clone the task */
//     match parent_task.clone_task(CloneFlags::default()) {
//         Ok(mut child_task) => {
//             let child_id = child_task.get_id();
//             child_task.vcpu.regs.reg[10] = 0; /* Set the return value (a0) to 0 in the child proc */
//             get_scheduler().add_task(child_task, get_cpu().get_cpuid());
//             /* Return the child task ID as pid to the parent proc */
//             child_id
//         },
//         Err(_) => {
//             usize::MAX /* Return -1 on error */
//         }
//     }
// }

pub fn sys_set_tid_address(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let tid_ptr = trapframe.get_arg(0) as *mut i32;
    
    // Store the TID address in the task's TID address field
    // TODO: Implement a proper TID management system
    
    // Increment the program counter
    trapframe.increment_pc_next(task);

    // Return 0 on success
    0
}

pub fn sys_exit(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    task.vcpu.store(trapframe);
    let exit_code = trapframe.get_arg(0) as i32;
    task.exit(exit_code);
    get_scheduler().schedule(get_cpu());
    
    usize::MAX // -1 (If exit is successful, this will not be reached)
}

pub fn sys_exit_group(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    task.vcpu.store(trapframe);
    let exit_code = trapframe.get_arg(0) as i32;
    // task.exit_group(exit_code);
    // For now, we just exit the current task
    task.exit(exit_code);

    get_scheduler().schedule(get_cpu());
    usize::MAX // -1 (If exit is successful, this will not be reached)
}

pub fn sys_set_robust_list(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let _robust_list_ptr = trapframe.get_arg(0) as *mut u8;
    
    // Store the robust list pointer in the task's robust list field
    // TODO: Implement a proper robust list management system

    // Increment the program counter
    trapframe.increment_pc_next(task);

    // Return 0 on success
    0
}

// pub fn sys_wait(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
//     let task = mytask().unwrap();
//     let status_ptr = trapframe.get_arg(0) as *mut i32;

//     for pid in task.get_children().clone() {
//         match task.wait(pid) {
//             Ok(status) => {
//                 // If the child proc is exited, we can return the status
//                 if status_ptr != core::ptr::null_mut() {
//                     let status_ptr = task.vm_manager.translate_vaddr(status_ptr as usize).unwrap() as *mut i32;
//                     unsafe {
//                         *status_ptr = status;
//                     }
//                 }
//                 trapframe.increment_pc_next(task);
//                 return pid;
//             },
//             Err(error) => {
//                 match error {
//                     WaitError::ChildNotExited(_) => continue,
//                     _ => {
//                         return trapframe.get_return_value();
//                     },
//                 }
//             }
//         }
//     }
    
//     // No child has exited yet, block until one does
//     // xv6's wait() is equivalent to waitpid(-1), so we use the parent waker
//     let parent_waker = get_parent_waker(task.get_id());
//     parent_waker.wait(task, trapframe);
// }

pub fn sys_kill(_abi: &mut LinuxRiscv64Abi, _trapframe: &mut Trapframe) -> usize {
    // Implement the kill syscall
    // This syscall is not yet implemented. Returning ENOSYS error code (-1).
    usize::MAX
}

pub fn sys_sbrk(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let increment = trapframe.get_arg(0);
    let brk = task.get_brk();
    trapframe.increment_pc_next(task);
    match task.set_brk(unsafe { brk.unchecked_add(increment) }) {
        Ok(_) => brk,
        Err(_) => usize::MAX, /* -1 */
    }
}

pub fn sys_brk(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let new_brk = trapframe.get_arg(0);
    trapframe.increment_pc_next(task);
    
    match task.set_brk(new_brk) {
        Ok(_) => new_brk,
        Err(_) => usize::MAX, /* -1 */
    }
}

// pub fn sys_chdir(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
//     let task = mytask().unwrap();
//     trapframe.increment_pc_next(task);
    
//     let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0) as usize).unwrap() as *const u8;
//     let path = match get_path_str_v2(path_ptr) {
//         Ok(p) => match to_absolute_path_v2(&task, &p) {
//             Ok(abs_path) => abs_path,
//             Err(_) => return usize::MAX,
//         },
//         Err(_) => return usize::MAX, /* -1 */
//     };

//     // Try to open the file
//     let file = match task.vfs.as_ref() {
//         Some(vfs) => vfs.open(&path, 0),
//         None => return usize::MAX, // VFS not initialized
//     };
//     if file.is_err() {
//         return usize::MAX; // -1
//     }
//     let kernel_obj = file.unwrap();
//     let file_handle = kernel_obj.as_file().unwrap();
//     // Check if the file is a directory
//     if file_handle.metadata().unwrap().file_type != FileType::Directory {
//         return usize::MAX; // -1
//     }

//     task.cwd = Some(path); // Update the current working directory

//     0
// }

pub fn sys_getpid(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    task.get_id()
}

pub fn sys_getppid(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    task.get_parent_id().unwrap_or(1) // Return parent PID or 1 if none
}

pub fn sys_setpgid(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let _pid = trapframe.get_arg(0);
    let _pgid = trapframe.get_arg(1);
    trapframe.increment_pc_next(task);
    0 // Always succeed
}

pub fn sys_getpgid(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let _pid = trapframe.get_arg(0);
    trapframe.increment_pc_next(task);
    task.get_id() // Return current task ID as process group ID
}

pub fn sys_prlimit64(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let _pid = trapframe.get_arg(0) as i32;
    let _resource = trapframe.get_arg(1);
    let _new_rlim_ptr = trapframe.get_arg(2);
    let old_rlim_ptr = trapframe.get_arg(3);

    trapframe.increment_pc_next(task);

    // If old_rlim is requested, write some reasonable default values
    if old_rlim_ptr != 0 {
        if let Some(old_rlim_paddr) = task.vm_manager.translate_vaddr(old_rlim_ptr) {
            unsafe {
                // Write a simple rlimit structure with high limits
                // struct rlimit { rlim_t rlim_cur; rlim_t rlim_max; }
                let rlimit = old_rlim_paddr as *mut [u64; 2];
                *rlimit = [
                    0xFFFFFFFF, // rlim_cur - current limit (high value)
                    0xFFFFFFFF, // rlim_max - maximum limit (high value)
                ];
            }
        }
    }

    0 // Always succeed
}

pub fn sys_getuid(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    0 // Return 0 for the root user (UID 0)
}

pub fn sys_geteuid(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    0 // Return 0 for the root user (EUID 0)
}

pub fn sys_getgid(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    0 // Return 0 for the root group (GID 0)
}

pub fn sys_getegid(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    0 // Return 0 for the root group (EGID 0)
}

/// Linux utsname structure for uname system call
/// This structure must match Linux's struct utsname layout
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UtsName {
    /// System name (e.g., "Linux")
    pub sysname: [u8; 65],
    /// Node name (hostname)
    pub nodename: [u8; 65], 
    /// Release (kernel version)
    pub release: [u8; 65],
    /// Version (kernel build info)
    pub version: [u8; 65],
    /// Machine (hardware architecture)
    pub machine: [u8; 65],
    /// Domain name (GNU extension)
    pub domainname: [u8; 65],
}

impl UtsName {
    /// Create a new UtsName with Scarlet system information
    pub fn new() -> Self {
        let mut uts = UtsName {
            sysname: [0; 65],
            nodename: [0; 65],
            release: [0; 65],
            version: [0; 65],
            machine: [0; 65],
            domainname: [0; 65],
        };

        // System name - identify as Linux for compatibility
        let sysname = b"Linux";
        uts.sysname[..sysname.len()].copy_from_slice(sysname);

        // Node name (hostname)
        let nodename = b"scarlet";
        uts.nodename[..nodename.len()].copy_from_slice(nodename);

        // Release (kernel version)
        let release = b"6.1.0-scarlet_linux_abi_module";
        uts.release[..release.len()].copy_from_slice(release);

        // Version (build info)
        let version = b"#1 SMP Scarlet";
        uts.version[..version.len()].copy_from_slice(version);

        // Machine (architecture)
        let machine = b"riscv64";
        uts.machine[..machine.len()].copy_from_slice(machine);

        // Domain name
        let domainname = b"(none)";
        uts.domainname[..domainname.len()].copy_from_slice(domainname);

        uts
    }
}

/// Linux uname system call implementation
/// 
/// Returns system information including system name, hostname, kernel version,
/// and hardware architecture. This provides compatibility with Linux applications
/// that query system information.
/// 
/// # Arguments
/// - buf: Pointer to utsname structure to fill
/// 
/// # Returns
/// - 0 on success
/// - usize::MAX on error (-1 in Linux)
pub fn sys_uname(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let buf_ptr = trapframe.get_arg(0);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Translate user space pointer
    let buf_vaddr = match task.vm_manager.translate_vaddr(buf_ptr) {
        Some(addr) => addr as *mut UtsName,
        None => return usize::MAX, // Invalid address
    };

    if buf_vaddr.is_null() {
        return usize::MAX; // NULL pointer
    }

    // Create and copy system information
    let uts = UtsName::new();
    unsafe {
        *buf_vaddr = uts;
    }

    0 // Success
}

/// Linux sys_clone implementation for RISC-V64 ABI
/// 
/// RISC-V64 follows the x86-64 argument order:
/// long clone(unsigned long flags, void *stack, int *parent_tid, int *child_tid, unsigned long tls);
///
/// Arguments:
/// - flags: clone flags (CLONE_VM, CLONE_FS, etc.)
/// - stack: child stack pointer (NULL to duplicate parent stack)
/// - parent_tid: pointer to store parent TID (for CLONE_PARENT_SETTID)
/// - child_tid: pointer to store child TID (for CLONE_CHILD_SETTID/CLONE_CHILD_CLEARTID)
/// - tls: TLS (Thread Local Storage) pointer
pub fn sys_clone(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let parent_task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let flags = trapframe.get_arg(0);
    let child_stack = trapframe.get_arg(1);
    let parent_tid_ptr = trapframe.get_arg(2) as *mut i32;
    let child_tid_ptr = trapframe.get_arg(3) as *mut i32;
    let tls = trapframe.get_arg(4);

    crate::println!("sys_clone: flags=0x{:x}, child_stack=0x{:x}, parent_tid_ptr={:p}, child_tid_ptr={:p}, tls={:x}", 
        flags, child_stack, parent_tid_ptr, child_tid_ptr, tls);

    trapframe.increment_pc_next(parent_task);
    parent_task.vcpu.store(trapframe);
    
    match parent_task.clone_task(CloneFlags::default()) {
        Ok(mut child_task) => {
            let child_id = child_task.get_id();
            child_task.vcpu.regs.reg[10] = 0; // a0 = 0 in child
            get_scheduler().add_task(child_task, get_cpu().get_cpuid());
            child_id
        },
        Err(_) => usize::MAX,
    }
}

/// Linux sys_setgid implementation (syscall 144)
///
/// Set group ID. This is a stub implementation that always succeeds.
///
/// Arguments:
/// - gid: group ID to set
///
/// Returns:
/// - 0 on success
pub fn sys_setgid(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX - 1, // -EPERM
    };

    let _gid = trapframe.get_arg(0);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Always succeed - group ID is ignored in this stub
    0
}

/// Linux sys_setuid implementation (syscall 146)
///
/// Set user ID. This is a stub implementation that always succeeds.
///
/// Arguments:
/// - uid: user ID to set
///
/// Returns:
/// - 0 on success
pub fn sys_setuid(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX - 1, // -EPERM
    };

    let _uid = trapframe.get_arg(0);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Always succeed - user ID is ignored in this stub
    0
}

///
/// Wait for process to change state (wait4 system call).
/// This is a stub implementation that returns immediately.
///
/// Arguments:
/// Wait for process to change state (wait4 system call).
/// 
/// This is a Linux-compatible implementation that waits for child processes
/// to exit and returns their process ID and exit status.
///
/// # Arguments
/// - pid: process ID to wait for
///   * -1: wait for any child process
///   * >0: wait for specific child process
///   * 0 or <-1: wait for process group (not implemented)
/// - wstatus: pointer to store status information (can be null)
/// - options: wait options (currently ignored - TODO: implement WNOHANG, WUNTRACED)
/// - rusage: pointer to resource usage structure (can be null, currently ignored)
///
/// # Returns
/// - On success: process ID of child that changed state
/// - On error: negated error code (e.g., usize::MAX - 9 for -ECHILD)
///
/// # Errors
/// - ECHILD: no child processes or specified child is not our child
/// - EFAULT: invalid address for wstatus pointer
/// - ENOSYS: unsupported operation (process groups)
/// - EPERM: no current task context
pub fn sys_wait4(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    use crate::task::{get_parent_waitpid_waker, WaitError};

    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX - 1, // -EPERM
    };

    let pid = trapframe.get_arg(0) as isize;
    let wstatus = trapframe.get_arg(1) as *mut i32;
    let _options = trapframe.get_arg(2); // TODO: Handle WNOHANG, WUNTRACED, etc.
    let _rusage = trapframe.get_arg(3); // TODO: Implement resource usage tracking

    // Check if the task has any children
    if task.get_children().is_empty() {
        trapframe.increment_pc_next(task);
        return usize::MAX - 9; // -ECHILD (no child processes)
    }

    crate::println!("sys_wait4: pid={}, wstatus={:p}, options={:x}, rusage={:x}", pid, wstatus, _options, _rusage);

    // Loop until a child exits or an error occurs
    loop {
        if pid == -1 {
            // Wait for any child process
            for child_pid in task.get_children().clone() {
                match task.wait(child_pid) {
                    Ok(status) => {
                        // Child has exited, return the status
                        if wstatus != core::ptr::null_mut() {
                            match task.vm_manager.translate_vaddr(wstatus as usize) {
                                Some(phys_addr) => {
                                    let status_ptr = phys_addr as *mut i32;
                                    unsafe {
                                        *status_ptr = status;
                                    }
                                }
                                None => {
                                    // Invalid address, return EFAULT
                                    trapframe.increment_pc_next(task);
                                    return usize::MAX - 13; // -EFAULT
                                }
                            }
                        }
                        trapframe.increment_pc_next(task);
                        return child_pid;
                    },
                    Err(error) => {
                        match error {
                            WaitError::NoSuchChild(_) => {
                                // This child is not our child
                                continue;
                            },
                            WaitError::ChildTaskNotFound(_) => {
                                // Child task not found in scheduler, continue with other children
                                continue;
                            },
                            WaitError::ChildNotExited(_) => {
                                // Child not exited yet, continue with other children
                                continue;
                            },
                        }
                    }
                }
            }
            
            // No child has exited yet, block until one does
            // Use parent waker for waitpid(-1) semantics
            let parent_waker = get_parent_waitpid_waker(task.get_id());
            parent_waker.wait(task.get_id(), get_cpu());
            crate::println!("Woke up from waitpid for any child");
            // Continue the loop to re-check after waking up
            continue;
        } else if pid > 0 {
            // Wait for specific child process
            let child_pid = pid as usize;
            
            // Check if this is actually our child
            if !task.get_children().contains(&child_pid) {
                trapframe.increment_pc_next(task);
                return usize::MAX - 9; // -ECHILD (not our child)
            }

            match task.wait(child_pid) {
                Ok(status) => {
                    // Child has exited, return the status
                    if wstatus != core::ptr::null_mut() {
                        match task.vm_manager.translate_vaddr(wstatus as usize) {
                            Some(phys_addr) => {
                                let status_ptr = phys_addr as *mut i32;
                                unsafe {
                                    *status_ptr = status;
                                }
                            }
                            None => {
                                // Invalid address, return EFAULT
                                trapframe.increment_pc_next(task);
                                return usize::MAX - 13; // -EFAULT
                            }
                        }
                    }
                    trapframe.increment_pc_next(task);
                    return child_pid;
                }
                Err(error) => {
                    match error {
                        WaitError::NoSuchChild(_) => {
                            trapframe.increment_pc_next(task);
                            return usize::MAX - 9; // -ECHILD
                        },
                        WaitError::ChildTaskNotFound(_) => {
                            trapframe.increment_pc_next(task);
                            return usize::MAX - 9; // -ECHILD
                        },
                        WaitError::ChildNotExited(_) => {
                            // Child not exited yet, wait for it
                            use crate::task::get_waitpid_waker;
                            let child_waker = get_waitpid_waker(child_pid);
                            child_waker.wait(task.get_id(), get_cpu());
                            crate::println!("Woke up from waitpid for child {}", child_pid);
                            // Continue the loop to re-check after waking up
                            continue;
                        },
                    }
                }
            }
        } else {
            // pid <= 0 && pid != -1: wait for process group (not implemented)
            trapframe.increment_pc_next(task);
            return usize::MAX - 37; // -ENOSYS (function not implemented)
        }
    }
}