use alloc::sync::Arc;

use crate::{arch::Trapframe, ipc::UnidirectionalPipe, object::KernelObject, task::mytask};

pub fn sys_pipe(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let pipefd_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0))
        .expect("Invalid pipefd pointer");
    let pipefd = unsafe { &mut *(pipefd_ptr as *mut [u32; 2]) };

    let (read_end, write_end) = UnidirectionalPipe::create_pair(4096);

    let read_handle = task.handle_table.insert(read_end).expect("Failed to insert read end");
    let write_handle = task.handle_table.insert(write_end).expect("Failed to insert write end");

    pipefd[0] = read_handle as u32;
    pipefd[1] = write_handle as u32;

    0
}