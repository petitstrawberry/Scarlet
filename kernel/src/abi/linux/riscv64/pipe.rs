//! Linux RISC-V 64 pipe syscalls (minimum implementation)
//!

use crate::{
    abi::linux::riscv64::LinuxRiscv64Abi,
    arch::Trapframe,
    ipc::UnidirectionalPipe,
    task::mytask,
};

/// Minimal sys_pipe2 implementation for Linux ABI (returns 0 on success, -1 on error)
pub fn sys_pipe2(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX,
    };
    trapframe.increment_pc_next(task);

    let pipefd_ptr = match task.vm_manager.translate_vaddr(trapframe.get_arg(0)) {
        Some(ptr) => ptr,
        None => return usize::MAX,
    };
    let pipefd = unsafe { &mut *(pipefd_ptr as *mut [u32; 2]) };

    let (read_end, write_end) = UnidirectionalPipe::create_pair(4096);
    let read_handle = match task.handle_table.insert(read_end) {
        Ok(h) => h,
        Err(_) => return usize::MAX,
    };
    let write_handle = match task.handle_table.insert(write_end) {
        Ok(h) => h,
        Err(_) => {
            task.handle_table.remove(read_handle);
            return usize::MAX;
        }
    };

    let read_fd = match abi.allocate_fd(read_handle as u32) {
        Ok(fd) => fd,
        Err(_) => {
            task.handle_table.remove(read_handle);
            task.handle_table.remove(write_handle);
            return usize::MAX;
        }
    };
    let write_fd = match abi.allocate_fd(write_handle as u32) {
        Ok(fd) => fd,
        Err(_) => {
            abi.remove_fd(read_fd);
            task.handle_table.remove(read_handle);
            task.handle_table.remove(write_handle);
            return usize::MAX;
        }
    };

    pipefd[0] = read_fd as u32;
    pipefd[1] = write_fd as u32;
    0
}
