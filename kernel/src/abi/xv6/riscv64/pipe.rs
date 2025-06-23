use crate::{arch::Trapframe, ipc::UnidirectionalPipe, task::mytask};

pub fn sys_pipe(abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let pipefd_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0))
        .expect("Invalid pipefd pointer");
    let pipefd = unsafe { &mut *(pipefd_ptr as *mut [u32; 2]) };

    let (read_end, write_end) = UnidirectionalPipe::create_pair(4096);

    let read_handle = task.handle_table.insert(read_end).expect("Failed to insert read end");
    let write_handle = task.handle_table.insert(write_end).expect("Failed to insert write end");

    // Allocate XV6 file descriptors and store them in the array
    let read_fd = match abi.allocate_fd(read_handle as u32) {
        Ok(fd) => fd,
        Err(_) => return usize::MAX, // Too many open files
    };
    let write_fd = match abi.allocate_fd(write_handle as u32) {
        Ok(fd) => fd,
        Err(_) => {
            // Clean up the read_fd allocation if write_fd fails
            abi.remove_fd(read_fd);
            task.handle_table.remove(read_handle);
            return usize::MAX; // Too many open files
        }
    };

    pipefd[0] = read_fd as u32;
    pipefd[1] = write_fd as u32;

    0
}