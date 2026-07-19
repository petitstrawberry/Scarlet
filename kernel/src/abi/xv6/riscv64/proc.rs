use crate::{
    arch::Trapframe,
    fs::FileType,
    ipc::{
        Event, EventManager,
        event::{EventPriority, ProcessControlType},
    },
    library::std::string::cstring_to_string,
    sched::scheduler::get_task_by_id,
    task::{CloneFlags, WaitError, get_parent_waitpid_waker, mytask},
};
use alloc::string::{String, ToString};

/// VFS v2 helper function for path absolutization using VfsManager
fn to_absolute_path_v2(task: &crate::task::Task, path: &str) -> Result<String, ()> {
    if path.starts_with('/') {
        Ok(path.to_string())
    } else {
        let vfs = task.vfs.read().clone().ok_or(())?;
        Ok(vfs.resolve_path_to_absolute(path))
    }
}

/// Helper function to replace the missing get_path_str function
/// TODO: This should be moved to a shared helper when VFS v2 provides public API
fn get_path_str_v2(ptr: *const u8) -> Result<String, ()> {
    const MAX_PATH_LENGTH: usize = 128;
    cstring_to_string(ptr, MAX_PATH_LENGTH)
        .map(|(s, _)| s)
        .map_err(|_| ())
}

pub fn sys_fork(
    _abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi,
    trapframe: &mut Trapframe,
) -> usize {
    let parent_task = mytask().unwrap();

    trapframe.increment_pc_next(&parent_task); /* Increment the program counter */

    /* Save the trapframe to the task before cloning */
    parent_task.vcpu.lock().store(trapframe);

    /* Clone the task */
    match parent_task.clone_task(CloneFlags::default()) {
        Ok(mut child_task) => {
            child_task.vcpu.lock().iregs.reg[10] = 0; /* Set the return value (a0) to 0 in the child proc */
            crate::sched::scheduler::apply_fork_child_diagnostic_affinity(
                &mut child_task,
                crate::arch::get_cpu().get_cpuid(),
            );

            let cpu_id = crate::sched::scheduler::select_cpu_for_task(&child_task);
            let parent_id = parent_task.get_id();

            // Register first, complete parent/child metadata, then enqueue.
            // A remote CPU may run the child immediately after enqueue via IPI.
            let child_id = crate::sched::scheduler::register_task(child_task);

            // Establish parent-child ownership before enqueueing. The task
            // protocol rejects an exiting parent and retries init atomically.
            if let Some(child) = get_task_by_id(child_id) {
                let _ = parent_task.adopt_registered_child(&child);
            }

            // Get namespace-local ID to return to user space (this is the PID visible to xv6 programs)
            let child_namespace_id = get_task_by_id(child_id)
                .map(|t| t.get_namespace_id())
                .unwrap_or(0);

            crate::sched::scheduler::enqueue_task(child_id, cpu_id);

            /* Return the child task namespace ID as pid to the parent proc */
            child_namespace_id
        }
        Err(_) => {
            usize::MAX /* Return -1 on error */
        }
    }
}

pub fn sys_exit(
    _abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi,
    trapframe: &mut Trapframe,
) -> usize {
    let task = mytask().unwrap();
    task.vcpu.lock().store(trapframe);
    let exit_code = trapframe.get_arg(0) as i32;
    task.request_deferred_exit(exit_code);
    usize::MAX // -1 (If exit is successful, this will not be reached)
}

pub fn sys_wait(
    _abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi,
    trapframe: &mut Trapframe,
) -> usize {
    let task = mytask().unwrap();
    let status_ptr = trapframe.get_arg(0) as *mut i32;

    // Loop until a child exits or an error occurs
    loop {
        // Wait for any child process
        for child_pid in task.get_children().clone() {
            match task.wait(child_pid) {
                Ok(status) => {
                    // Child has exited, return the status
                    if status_ptr != core::ptr::null_mut() {
                        let status_ptr = task
                            .vm_manager
                            .translate_to_kva(status_ptr as usize)
                            .unwrap() as *mut i32;
                        unsafe {
                            *status_ptr = status;
                        }
                    }
                    trapframe.increment_pc_next(&task);
                    return child_pid;
                }
                Err(error) => match error {
                    WaitError::ChildNotExited(_) => continue,
                    _ => {
                        trapframe.increment_pc_next(&task);
                        return usize::MAX;
                    }
                },
            }
        }

        // No child has exited yet, block until one does
        let parent_waker = get_parent_waitpid_waker(task.get_id());
        parent_waker.wait_owned(task.get_id(), trapframe);
        // Continue the loop to re-check after waking up
        continue;
    }
}

pub fn sys_kill(
    _abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi,
    trapframe: &mut Trapframe,
) -> usize {
    let task = mytask().unwrap();
    let pid = trapframe.get_arg(0) as usize;
    let signal = trapframe.get_arg(1) as i32;

    trapframe.increment_pc_next(&task);

    // For xv6 compatibility, only signal 9 (SIGKILL) is implemented for now
    if signal != 9 {
        return usize::MAX; // -1 (unsupported signal)
    }

    let Some(target_task_id) = task.get_namespace().resolve_global_id(pid) else {
        return usize::MAX;
    };

    if target_task_id == task.get_id() {
        task.request_deferred_exit(9);
        return 0;
    }

    let Ok(target_task_id) = u32::try_from(target_task_id) else {
        return usize::MAX;
    };
    let event = Event::direct_process_control(
        target_task_id,
        ProcessControlType::Kill,
        EventPriority::High,
        true,
    );
    EventManager::get_manager()
        .send_event(event)
        .map(|()| 0)
        .unwrap_or(usize::MAX)
}

pub fn sys_sbrk(
    _abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi,
    trapframe: &mut Trapframe,
) -> usize {
    let task = mytask().unwrap();
    let increment = trapframe.get_arg(0);
    let brk = task.get_brk();
    trapframe.increment_pc_next(&task);
    match task.set_brk(unsafe { brk.unchecked_add(increment) }) {
        Ok(_) => brk,
        Err(_) => usize::MAX, /* -1 */
    }
}

pub fn sys_chdir(
    _abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi,
    trapframe: &mut Trapframe,
) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(&task);

    let path_ptr = task
        .vm_manager
        .translate_to_kva(trapframe.get_arg(0) as usize)
        .unwrap() as *const u8;
    let path = match get_path_str_v2(path_ptr) {
        Ok(p) => match to_absolute_path_v2(&task, &p) {
            Ok(abs_path) => abs_path,
            Err(_) => return usize::MAX,
        },
        Err(_) => return usize::MAX, /* -1 */
    };

    // Try to open the file
    let file = match task.vfs.read().clone() {
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

    // Update the current working directory via VfsManager
    if let Some(vfs) = task.vfs.read().clone() {
        let _ = vfs.set_cwd_by_path(&path);
    }

    0
}

pub fn sys_getpid(
    _abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi,
    trapframe: &mut Trapframe,
) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(&task);
    // Return namespace-local ID (this is the PID visible to xv6 programs)
    task.get_namespace_id()
}

pub fn sys_sleep(
    _abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi,
    trapframe: &mut Trapframe,
) -> usize {
    let ticks = trapframe.get_arg(0) as u64;
    let task = mytask().unwrap();

    // Increment PC before sleeping to avoid infinite loop
    trapframe.increment_pc_next(&task);

    // Call the blocking sleep method - this will return when sleep completes
    task.sleep(trapframe, ticks);

    // Set return value to 0 for successful sleep
    0
}
