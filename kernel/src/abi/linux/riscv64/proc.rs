use alloc::string::{String, ToString};
use crate::{
    abi::linux::riscv64::LinuxRiscv64Abi, arch::{get_cpu, Trapframe}, fs::FileType, library::std::string::cstring_to_string, sched::scheduler::get_scheduler, task::{get_parent_waker, mytask, CloneFlags, WaitError}
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
}

pub fn sys_exit_group(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    task.vcpu.store(trapframe);
    let exit_code = trapframe.get_arg(0) as i32;
    // task.exit_group(exit_code);
    // For now, we just exit the current task
    task.exit(exit_code);

    get_scheduler().schedule(get_cpu());
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

// pub fn sys_getpid(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
//     let task = mytask().unwrap();
//     trapframe.increment_pc_next(task);
//     task.get_id()
// }

pub fn sys_getuid(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    0 // Return 0 for the root user (UID 0)
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