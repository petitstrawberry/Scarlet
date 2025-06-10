use crate::{arch::{get_cpu, Trapframe}, fs::{helper::get_path_str, File, FileType, VfsManager}, sched::scheduler::get_scheduler, task::{mytask, WaitError}};

pub fn sys_fork(trapframe: &mut Trapframe) -> usize {
    let parent_task = mytask().unwrap();
    
    trapframe.increment_pc_next(parent_task); /* Increment the program counter */

    /* Save the trapframe to the task before cloning */
    parent_task.vcpu.store(trapframe);
    
    /* Clone the task */
    match parent_task.clone_task() {
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

pub fn sys_exit(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    task.vcpu.store(trapframe);
    let exit_code = trapframe.get_arg(0) as i32;
    task.exit(exit_code);
    get_scheduler().schedule(get_cpu());
    trapframe.get_arg(0) as usize
}

pub fn sys_wait(trapframe: &mut Trapframe) -> usize {
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

pub fn sys_pipe(trapframe: &mut Trapframe) -> usize {
    // Implement the pipe syscall
    // This is a placeholder implementation
    0
}

pub fn sys_kill(trapframe: &mut Trapframe) -> usize {
    // Implement the kill syscall
    // This is a placeholder implementation
    0
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

pub fn sys_chdir(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    
    let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0) as usize).unwrap() as *const u8;
    let path = match get_path_str(path_ptr) {
        Ok(p) => VfsManager::to_absolute_path(&task, &p).unwrap(),
        Err(_) => return usize::MAX, /* -1 */
    };

    // Try to open the file
    let file: Result<File, _> = match task.vfs.as_ref() {
        Some(vfs) => vfs.open(&path, 0),
        None => return usize::MAX, // VFS not initialized
    };
    if file.is_err() {
        return usize::MAX; // -1
    }
    let file = file.unwrap();
    // Check if the file is a directory
    if file.metadata().unwrap().file_type != FileType::Directory {
        return usize::MAX; // -1
    }

    task.cwd = Some(path); // Update the current working directory

    0
}