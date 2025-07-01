use alloc::string::{String, ToString};
use crate::{
    arch::{get_cpu, Trapframe}, 
    fs::FileType, 
    library::std::string::cstring_to_string,
    sched::scheduler::get_scheduler, 
    task::{mytask, CloneFlags, WaitError}
};

/// VFS v2 helper function for path absolutization
/// TODO: Move this to a shared helper module when VFS v2 provides public API
fn to_absolute_path_v2(task: &crate::task::Task, path: &str) -> Result<String, ()> {
    if path.starts_with('/') {
        Ok(path.to_string())
    } else {
        let cwd = task.cwd.clone().ok_or(())?;
        let mut absolute_path = cwd;
        if !absolute_path.ends_with('/') {
            absolute_path.push('/');
        }
        absolute_path.push_str(path);
        // Simple normalization (removes "//", ".", etc.)
        let mut components = alloc::vec::Vec::new();
        for comp in absolute_path.split('/') {
            match comp {
                "" | "." => {},
                ".." => { components.pop(); },
                _ => components.push(comp),
            }
        }
        Ok("/".to_string() + &components.join("/"))
    }
}

/// Helper function to replace the missing get_path_str function
/// TODO: This should be moved to a shared helper when VFS v2 provides public API
fn get_path_str_v2(ptr: *const u8) -> Result<String, ()> {
    const MAX_PATH_LENGTH: usize = 128;
    cstring_to_string(ptr, MAX_PATH_LENGTH).map(|(s, _)| s).map_err(|_| ())
}

pub fn sys_fork(_abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, trapframe: &mut Trapframe) -> usize {
    let parent_task = mytask().unwrap();
    
    trapframe.increment_pc_next(parent_task); /* Increment the program counter */

    /* Save the trapframe to the task before cloning */
    parent_task.vcpu.store(trapframe);
    
    /* Clone the task */
    match parent_task.clone_task(CloneFlags::default()) {
        Ok(mut child_task) => {
            let child_id = child_task.get_id();
            child_task.vcpu.regs.reg[10] = 0; /* Set the return value (a0) to 0 in the child proc */
            get_scheduler().add_task(child_task, get_cpu().get_cpuid());
            /* Return the child task ID as pid to the parent proc */
            child_id
        },
        Err(_) => {
            usize::MAX /* Return -1 on error */
        }
    }
}

pub fn sys_exit(_abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    task.vcpu.store(trapframe);
    let exit_code = trapframe.get_arg(0) as i32;
    task.exit(exit_code);
    get_scheduler().schedule(get_cpu());
    trapframe.get_arg(0) as usize
}

pub fn sys_wait(_abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let status_ptr = trapframe.get_arg(0) as *mut i32;

    for pid in task.get_children().clone() {
        match task.wait(pid) {
            Ok(status) => {
                // If the child proc is exited, we can return the status
                if status_ptr != core::ptr::null_mut() {
                    let status_ptr = task.vm_manager.translate_vaddr(status_ptr as usize).unwrap() as *mut i32;
                    unsafe {
                        *status_ptr = status;
                    }
                }
                trapframe.increment_pc_next(task);
                return pid;
            },
            Err(error) => {
                match error {
                    WaitError::ChildNotExited(_) => continue,
                    _ => {
                        return trapframe.get_return_value();
                    },
                }
            }
        }
    }
    return trapframe.get_return_value();
}

pub fn sys_kill(_abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, _trapframe: &mut Trapframe) -> usize {
    // Implement the kill syscall
    // This is a placeholder implementation
    0
}

pub fn sys_sbrk(_abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let increment = trapframe.get_arg(0);
    let brk = task.get_brk();
    trapframe.increment_pc_next(task);
    match task.set_brk(unsafe { brk.unchecked_add(increment) }) {
        Ok(_) => brk,
        Err(_) => usize::MAX, /* -1 */
    }
}

pub fn sys_chdir(_abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    
    let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0) as usize).unwrap() as *const u8;
    let path = match get_path_str_v2(path_ptr) {
        Ok(p) => match to_absolute_path_v2(&task, &p) {
            Ok(abs_path) => abs_path,
            Err(_) => return usize::MAX,
        },
        Err(_) => return usize::MAX, /* -1 */
    };

    // Try to open the file
    let file = match task.vfs.as_ref() {
        Some(vfs) => vfs.open(&path, 0),
        None => return usize::MAX, // VFS not initialized
    };
    if file.is_err() {
        return usize::MAX; // -1
    }
    let kernel_obj = file.unwrap();
    let file_handle = kernel_obj.as_file().unwrap();
    // Check if the file is a directory
    if file_handle.metadata().unwrap().file_type != FileType::Directory {
        return usize::MAX; // -1
    }

    task.cwd = Some(path); // Update the current working directory

    0
}

pub fn sys_getpid(_abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    task.get_id()
}