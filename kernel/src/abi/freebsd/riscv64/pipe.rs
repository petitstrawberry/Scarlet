//! FreeBSD RISC-V 64 pipe syscalls
//!

use crate::{
    abi::freebsd::riscv64::{errno::FreeBsdErrno, FreeBsdRiscv64Abi},
    arch::Trapframe,
    ipc::UnidirectionalPipe,
    object::capability::selectable::Selectable,
    task::mytask,
};

/// FreeBSD pipe syscall
pub fn sys_pipe(abi: &mut FreeBsdRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return FreeBsdErrno::EIO.as_error(),
    };

    let pipefd_user = trapframe.get_arg(0);

    trapframe.increment_pc_next(task);

    let pipefd_ptr = match task.vm_manager.translate_vaddr(pipefd_user) {
        Some(ptr) => ptr as *mut u32,
        None => return FreeBsdErrno::EFAULT.as_error(),
    };

    let (read_end, write_end) = UnidirectionalPipe::create_pair(4096);

    let read_handle = match task.handle_table.insert(read_end) {
        Ok(h) => h,
        Err(_) => return FreeBsdErrno::ENFILE.as_error(),
    };
    let write_handle = match task.handle_table.insert(write_end) {
        Ok(h) => h,
        Err(_) => {
            let _ = task.handle_table.remove(read_handle);
            return FreeBsdErrno::ENFILE.as_error();
        }
    };

    let read_fd = match abi.allocate_fd(read_handle as u32) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = task.handle_table.remove(read_handle);
            let _ = task.handle_table.remove(write_handle);
            return FreeBsdErrno::EMFILE.as_error();
        }
    };
    let write_fd = match abi.allocate_fd(write_handle as u32) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = abi.remove_fd(read_fd);
            let _ = task.handle_table.remove(read_handle);
            let _ = task.handle_table.remove(write_handle);
            return FreeBsdErrno::EMFILE.as_error();
        }
    };

    unsafe {
        core::ptr::write_unaligned(pipefd_ptr, read_fd as u32);
        core::ptr::write_unaligned(pipefd_ptr.add(1), write_fd as u32);
    }

    0
}
