use crate::{
    abi::linux::generic::LinuxAbi,
    arch::{Trapframe, get_cpu},
    sched::scheduler::get_scheduler,
    task::{CloneFlags, mytask},
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

// pub fn sys_fork(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
//     let parent_task = mytask().unwrap();

//     trapframe.increment_pc_next(parent_task); /* Increment the program counter */
//     /* Save the trapframe to the task before cloning */
//     parent_task.vcpu.lock().store(trapframe);

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

pub fn sys_set_tid_address(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let tid_ptr = trapframe.get_arg(0);

    let tid_opt = (tid_ptr != 0).then_some(tid_ptr);
    abi.thread_state_mut().clear_child_tid_ptr = tid_opt;

    trapframe.increment_pc_next(task);

    // Return current task namespace ID (Linux TID visible to user space)
    task.get_namespace_id()
}

pub fn sys_exit(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    task.vcpu.lock().store(trapframe);
    let exit_code = trapframe.get_arg(0) as i32;

    task.exit(exit_code);
    get_scheduler().schedule(trapframe);
    usize::MAX
}

pub fn sys_exit_group(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    task.vcpu.lock().store(trapframe);
    let exit_code = trapframe.get_arg(0) as i32;
    task.exit(exit_code);
    get_scheduler().schedule(trapframe);
    usize::MAX
}

pub fn sys_set_robust_list(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let head = trapframe.get_arg(0);
    let len = trapframe.get_arg(1);

    let head_opt = (head != 0).then_some(head);
    let state = abi.thread_state_mut();
    state.robust_list_head = head_opt;
    state.robust_list_len = len as usize;

    trapframe.increment_pc_next(task);

    0
}

// pub fn sys_wait(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
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

#[allow(dead_code)]
pub fn sys_kill(_abi: &mut LinuxAbi, _trapframe: &mut Trapframe) -> usize {
    // Implement the kill syscall
    // This syscall is not yet implemented. Returning ENOSYS error code (-1).
    usize::MAX
}

#[allow(dead_code)]
pub fn sys_sbrk(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let increment = trapframe.get_arg(0) as isize; // Treat as signed increment
    let current_brk = task.get_brk();
    trapframe.increment_pc_next(task);

    // Handle increment of 0 (query current brk)
    if increment == 0 {
        return current_brk;
    }

    let new_brk = if increment > 0 {
        current_brk.checked_add(increment as usize)
    } else {
        // Handle negative increment (decrease brk)
        current_brk.checked_sub((-increment) as usize)
    };

    let new_brk = match new_brk {
        Some(brk) => brk,
        None => {
            // Overflow/underflow
            use super::errno;
            return errno::to_result(errno::ENOMEM);
        }
    };

    match task.set_brk(new_brk) {
        Ok(_) => {
            let new_actual = task.get_brk();
            // crate::println!("[brk] sbrk inc={} old={:#x} new={:#x}", increment, current_brk, new_actual);
            new_actual
        }
        Err(_) => {
            use super::errno;
            // crate::println!("[brk] sbrk fail inc={} old={:#x}", increment, current_brk);
            errno::to_result(errno::ENOMEM)
        }
    }
}

pub fn sys_brk(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let new_brk = trapframe.get_arg(0);
    trapframe.increment_pc_next(task);

    // If new_brk is 0, just return current brk (query current brk)
    if new_brk == 0 {
        return task.get_brk();
    }

    let _old = task.get_brk();
    match task.set_brk(new_brk) {
        Ok(_) => {
            let actual = task.get_brk();
            // crate::println!("[brk] brk req={:#x} old={:#x} -> {:#x}", new_brk, old, actual);
            actual
        }
        Err(_) => {
            let cur = task.get_brk();
            // crate::println!("[brk] brk fail req={:#x} keep={:#x}", new_brk, cur);
            cur
        }
    }
}

// pub fn sys_chdir(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
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
//     let file = match task.vfs.read().clone() {
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

pub fn sys_getpid(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    // Return TGID for Linux semantics; fallback to Task ID if unset
    let tgid = _abi.thread_state().tgid;
    trapframe.increment_pc_next(task);
    if tgid != 0 { tgid } else { task.get_id() }
}

pub fn sys_getppid(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    task.get_parent_id().unwrap_or(1) // Return parent PID or 1 if none
}

/// Linux gettid system call implementation
/// Returns the calling thread ID (TID). For now, this equals Scarlet Task ID.
pub fn sys_gettid(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    task.get_id()
}

/// Linux prctl system call (syscall 167)
///
/// Operations on a process or thread. This is a stub implementation
/// that returns success for common operations.
///
/// Arguments:
///   - arg0: option (PR_* operation)
///   - arg1-arg4: operation-specific arguments
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) for unsupported operations
pub fn sys_prctl(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let option = trapframe.get_arg(0) as i32;
    let _arg2 = trapframe.get_arg(1);
    let _arg3 = trapframe.get_arg(2);
    let _arg4 = trapframe.get_arg(3);
    let _arg5 = trapframe.get_arg(4);

    trapframe.increment_pc_next(task);

    crate::println!(
        "[stub] sys_prctl: option={}, arg2={:#x}, arg3={:#x}, arg4={:#x}, arg5={:#x}",
        option,
        _arg2,
        _arg3,
        _arg4,
        _arg5
    );

    // Common PR_* operations (from include/uapi/linux/prctl.h)
    // For now, just return success for all operations
    // Specific operations can be implemented as needed
    0
}

pub fn sys_setpgid(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let _pid = trapframe.get_arg(0);
    let _pgid = trapframe.get_arg(1);
    trapframe.increment_pc_next(task);
    0 // Always succeed
}

pub fn sys_getpgid(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let _pid = trapframe.get_arg(0);
    trapframe.increment_pc_next(task);
    task.get_id() // Return current task ID as process group ID
}

pub fn sys_prlimit64(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let _pid = trapframe.get_arg(0) as i32;
    let _resource = trapframe.get_arg(1);
    let _new_rlim_ptr = trapframe.get_arg(2);
    let old_rlim_ptr = trapframe.get_arg(3);

    trapframe.increment_pc_next(task);

    // If old_rlim is requested, write some reasonable default values
    if old_rlim_ptr != 0 {
        if let Some(old_rlim_paddr) = task.vm_manager.translate_to_kva(old_rlim_ptr) {
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

pub fn sys_getuid(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    0 // Return 0 for the root user (UID 0)
}

pub fn sys_geteuid(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    0 // Return 0 for the root user (EUID 0)
}

pub fn sys_getgid(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    0 // Return 0 for the root group (GID 0)
}

pub fn sys_getegid(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
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

        let machine = if cfg!(target_arch = "riscv64") {
            b"riscv64"
        } else if cfg!(target_arch = "aarch64") {
            b"aarch64"
        } else {
            b"unknown"
        };
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
pub fn sys_uname(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let buf_ptr = trapframe.get_arg(0);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Translate user space pointer
    let buf_vaddr = match task.vm_manager.translate_to_kva(buf_ptr) {
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

/// Linux sys_clone implementation
///
/// clone argument order (Linux ABI):
/// long clone(unsigned long flags, void *stack, int *parent_tid, unsigned long tls, int *child_tid);
///
/// Arguments:
/// - flags: clone flags (CLONE_VM, CLONE_FS, etc.)
/// - stack: child stack pointer (NULL to duplicate parent stack)
/// - parent_tid: pointer to store parent TID (for CLONE_PARENT_SETTID)
/// - child_tid: pointer to store child TID (for CLONE_CHILD_SETTID/CLONE_CHILD_CLEARTID)
/// - tls: TLS (Thread Local Storage) pointer
pub fn sys_clone(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let parent_task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let flags = trapframe.get_arg(0);
    let child_stack = trapframe.get_arg(1);
    let parent_tid_ptr = trapframe.get_arg(2) as *mut i32; // a2
    let tls = trapframe.get_arg(3); //a3 (TLS)
    let child_tid_ptr = trapframe.get_arg(4) as *mut i32; // a4

    let parent_tid_opt = (!parent_tid_ptr.is_null()).then_some(parent_tid_ptr as usize);
    let child_tid_opt = (!child_tid_ptr.is_null()).then_some(child_tid_ptr as usize);
    {
        let state = abi.thread_state_mut();
        state.parent_tid_ptr = parent_tid_opt;
        state.child_tid_ptr = child_tid_opt;
        if (flags & CLONE_CHILD_CLEARTID) != 0 {
            state.clear_child_tid_ptr = child_tid_opt;
        }
        if (flags & CLONE_SETTLS) != 0 {
            state.tls_pointer = Some(tls);
        }
    }

    // Linux clone flags
    const CLONE_VM: usize = 0x00000100;
    const CLONE_FS: usize = 0x00000200;
    const CLONE_FILES: usize = 0x00000400;
    // Thread-related flags (accepted but not fully implemented yet)
    #[allow(dead_code)]
    const CLONE_SIGHAND: usize = 0x00000800;
    const CLONE_THREAD: usize = 0x00010000;
    #[allow(dead_code)]
    const CLONE_SETTLS: usize = 0x00080000;
    /// Set child's TID at child_tid_ptr in child's memory
    #[allow(dead_code)]
    const CLONE_CHILD_SETTID: usize = 0x01000000;
    #[allow(dead_code)]
    const CLONE_PARENT_SETTID: usize = 0x00100000;
    #[allow(dead_code)]
    const CLONE_CHILD_CLEARTID: usize = 0x00200000;

    // Accept CLONE_THREAD/CLONE_SIGHAND for minimal thread support.
    // Note: signal handler sharing and full thread group semantics are partial.
    // Stash CLONE_THREAD intent so on_task_cloned can initialize child's TGID.
    {
        let state = abi.thread_state_mut();
        state.pending_clone_is_thread = (flags & CLONE_THREAD) != 0;
    }

    trapframe.increment_pc_next(parent_task);
    parent_task.vcpu.lock().store(trapframe);

    // Map Linux clone flags to Scarlet CloneFlags
    let mut cflags = CloneFlags::new();
    if (flags & CLONE_VM) != 0 {
        cflags.set(crate::task::CloneFlagsDef::Vm);
    }
    if (flags & CLONE_FS) != 0 {
        cflags.set(crate::task::CloneFlagsDef::Fs);
    }
    if (flags & CLONE_FILES) != 0 {
        cflags.set(crate::task::CloneFlagsDef::Files);
    }
    if (flags & CLONE_THREAD) != 0 {
        cflags.set(crate::task::CloneFlagsDef::Thread);
    }

    let ret = match parent_task.clone_task(cflags) {
        Ok(mut child_task) => {
            child_task.vcpu.lock().iregs.reg[10] = 0; // a0 = 0 in child
            // If child_stack is provided, set child's user SP
            if child_stack != 0 {
                child_task.vcpu.lock().set_sp(child_stack);
            }
            // If CLONE_SETTLS requested, set tp (x4) to tls for child
            #[allow(non_snake_case)]
            const CLONE_SETTLS: usize = 0x00080000;
            if (flags & CLONE_SETTLS) != 0 {
                child_task.vcpu.lock().iregs.reg[4] = tls; // x4 = tp
            }

            let scheduler = get_scheduler();
            let cpu_id = get_cpu().get_cpuid();
            let parent_id = parent_task.get_id();

            // Add child and get allocated ID
            let child_id = scheduler.add_task(child_task, cpu_id);

            // Establish parent-child relationship now that both have valid IDs
            if let Some(child) = scheduler.get_task_by_id(child_id) {
                child.set_parent_id(parent_id);
            }
            if let Some(parent) = scheduler.get_task_by_id(parent_id) {
                parent.add_child(child_id);
            }

            // Do not modify user pthread list; musl manages linkage. No safety-net writes.
            // Handle parent TID store when CLONE_PARENT_SETTID is requested
            if (flags & CLONE_PARENT_SETTID) != 0 && !parent_tid_ptr.is_null() {
                if let Some(paddr) = parent_task
                    .vm_manager
                    .translate_to_kva(parent_tid_ptr as usize)
                {
                    unsafe {
                        *(paddr as *mut i32) = child_id as i32;
                    }
                }
            }
            // IMPORTANT: Only write child TID when CLONE_CHILD_SETTID is set.
            // For CLONE_CHILD_CLEARTID, the pointer is a futex lock to clear on exit.
            if (flags & CLONE_CHILD_SETTID) != 0 && !child_tid_ptr.is_null() {
                if let Some(paddr) = get_scheduler()
                    .get_task_by_id(child_id)
                    .unwrap()
                    .vm_manager
                    .translate_to_kva(child_tid_ptr as usize)
                {
                    unsafe {
                        *(paddr as *mut i32) = child_id as i32;
                    }
                }
            }
            child_id
        }
        Err(_) => usize::MAX,
    };

    // Clear pending flag in parent after clone completes
    abi.thread_state_mut().pending_clone_is_thread = false;
    ret
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
pub fn sys_setgid(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
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
pub fn sys_setuid(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
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
pub fn sys_wait4(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    use crate::task::{WaitError, get_parent_waitpid_waker};

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

    // Minimal implementation; no verbose logging.

    // Loop until a child exits or an error occurs
    loop {
        if pid == -1 {
            // Wait for any child process
            for child_pid in task.get_children().clone() {
                match task.wait(child_pid) {
                    Ok(status) => {
                        // Child has exited, return the status
                        if wstatus != core::ptr::null_mut() {
                            match task.vm_manager.translate_to_kva(wstatus as usize) {
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
                                // This child is not our child
                                continue;
                            }
                            WaitError::ChildTaskNotFound(_) => {
                                // Child task not found in scheduler, continue with other children
                                continue;
                            }
                            WaitError::ChildNotExited(_) => {
                                // Child not exited yet, continue with other children
                                continue;
                            }
                        }
                    }
                }
            }

            // No child has exited yet, block until one does
            // Use parent waker for waitpid(-1) semantics
            let parent_waker = get_parent_waitpid_waker(task.get_id());
            parent_waker.wait(task.get_id(), task.get_trapframe());
            // Woken by child exit; re-check children.
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
                        match task.vm_manager.translate_to_kva(wstatus as usize) {
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
                        }
                        WaitError::ChildTaskNotFound(_) => {
                            trapframe.increment_pc_next(task);
                            return usize::MAX - 9; // -ECHILD
                        }
                        WaitError::ChildNotExited(_) => {
                            // Child not exited yet, wait for it
                            use crate::task::get_waitpid_waker;
                            let child_waker = get_waitpid_waker(child_pid);
                            child_waker.wait(task.get_id(), task.get_trapframe());
                            // Woken by specific child exit; re-check.
                            // Continue the loop to re-check after waking up
                            continue;
                        }
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

/// Linux sys_membarrier implementation (syscall 283)
///
/// Memory barrier system call for ensuring memory ordering between threads.
/// This is a stub implementation that always succeeds.
///
/// Arguments:
/// - cmd: membarrier command (various MEMBARRIER_CMD_* constants)
/// - flags: flags for the command (usually 0)
/// - cpu_id: CPU ID for per-CPU barriers (usually unused)
///
/// Returns:
/// - 0 on success
/// - Negative error code on failure
///
/// Note: This is a no-op stub. On a real system with multiple cores,
/// this would issue appropriate memory barrier instructions to ensure
/// memory ordering visibility across CPUs. For single-core or simple
/// multi-core systems without complex memory reordering, this stub
/// should be sufficient for most applications.
pub fn sys_membarrier(_abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX - 1, // -EPERM
    };

    let _cmd = trapframe.get_arg(0);
    let _flags = trapframe.get_arg(1);
    let _cpu_id = trapframe.get_arg(2);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Issue a memory fence to ensure all memory operations are visible
    // This is a basic implementation - real membarrier has multiple modes
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

    // Always succeed - stub implementation
    0
}

/// Linux sys_memfd_create - Create an anonymous file descriptor for memory mapping
///
/// Creates a new anonymous file descriptor that can be used for memory mapping.
/// This is primarily used by Wayland clients for shared memory.
///
/// Arguments:
/// - abi: LinuxAbi context
/// - trapframe: Trapframe containing syscall arguments
///   - arg0: uname (name for the memfd - can be NULL)
///   - arg1: flags (MFD_CLOEXEC, MFD_ALLOW_SEALING, etc.)
///
/// Returns:
/// - New file descriptor on success
/// - usize::MAX (Linux -1) on error
pub fn sys_memfd_create(abi: &mut LinuxAbi, trapframe: &mut Trapframe) -> usize {
    use crate::ipc::SharedMemory;
    use crate::object::KernelObject;

    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };

    let _uname_ptr = trapframe.get_arg(0);
    let flags = trapframe.get_arg(1) as u32;

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Parse flags (Linux memfd_create flags)
    const MFD_CLOEXEC: u32 = 0x0001;
    const MFD_ALLOW_SEALING: u32 = 0x0002;
    const MFD_SEAL_SEAL: u32 = 0x0004;

    let _cloexec = (flags & MFD_CLOEXEC) != 0;
    let _allow_sealing = (flags & MFD_ALLOW_SEALING) != 0;
    let _seal = (flags & MFD_SEAL_SEAL) != 0;

    // Create shared memory object (size 0 initially, will be resized by ftruncate)
    // Default size for Wayland SHM pools
    const DEFAULT_SHM_SIZE: usize = crate::environment::PAGE_SIZE; // one page as starting point

    let shm = match SharedMemory::new(DEFAULT_SHM_SIZE, 0x3 /* READ | WRITE */) {
        Ok(shm) => shm,
        Err(e) => {
            crate::early_println!("[sys_memfd_create] Failed to create shared memory: {:?}", e);
            return usize::MAX;
        }
    };

    // Insert into handle table
    let handle = match task
        .handle_table
        .insert(KernelObject::SharedMemory(alloc::sync::Arc::new(shm)))
    {
        Ok(h) => h,
        Err(_) => {
            crate::early_println!("[sys_memfd_create] Failed to insert handle into table");
            return usize::MAX;
        }
    };

    // Allocate fd for the shared memory
    let fd = match abi.allocate_fd(handle) {
        Ok(fd) => fd,
        Err(_) => {
            // Clean up on error
            let _ = task.handle_table.remove(handle);
            crate::early_println!("[sys_memfd_create] Failed to allocate fd");
            return usize::MAX;
        }
    };

    // crate::early_println!(
    //     "[sys_memfd_create] Created memfd: fd={}, handle={}",
    //     fd,
    //     handle
    // );

    fd
}
