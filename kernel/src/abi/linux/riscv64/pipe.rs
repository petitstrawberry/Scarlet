//! Linux RISC-V 64 pipe syscalls (minimum implementation)
//!

use crate::{
    abi::linux::riscv64::{
        LinuxRiscv64Abi, errno,
        fs::{FD_CLOEXEC, O_CLOEXEC, O_NONBLOCK},
    },
    arch::Trapframe,
    ipc::UnidirectionalPipe,
    object::capability::selectable::Selectable,
    task::mytask,
};

/// Minimal sys_pipe2 implementation for Linux ABI (returns 0 on success, -1 on error)
pub fn sys_pipe2(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return errno::to_result(errno::EIO),
    };

    let pipefd_user = trapframe.get_arg(0);
    let flags = trapframe.get_arg(1) as i32;

    trapframe.increment_pc_next(task);

    // Validate flags: Linux pipe2 only accepts O_CLOEXEC and O_NONBLOCK.
    let allowed = O_CLOEXEC | O_NONBLOCK;
    if flags & !allowed != 0 {
        return errno::to_result(errno::EINVAL);
    }

    let pipefd_ptr = match task.vm_manager.translate_to_kva(pipefd_user) {
        Some(ptr) => ptr as *mut u32,
        None => return errno::to_result(errno::EFAULT),
    };

    let (read_end, write_end) = UnidirectionalPipe::create_pair(4096);

    let read_handle = match task.handle_table.insert(read_end) {
        Ok(h) => h,
        Err(_) => return errno::to_result(errno::ENFILE),
    };
    let write_handle = match task.handle_table.insert(write_end) {
        Ok(h) => h,
        Err(_) => {
            let _ = task.handle_table.remove(read_handle);
            return errno::to_result(errno::ENFILE);
        }
    };

    let read_fd = match abi.allocate_fd(read_handle as u32) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = task.handle_table.remove(read_handle);
            let _ = task.handle_table.remove(write_handle);
            return errno::to_result(errno::EMFILE);
        }
    };
    let write_fd = match abi.allocate_fd(write_handle as u32) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = abi.remove_fd(read_fd);
            let _ = task.handle_table.remove(read_handle);
            let _ = task.handle_table.remove(write_handle);
            return errno::to_result(errno::EMFILE);
        }
    };

    let mut status_flags: u32 = 0;
    if (flags & O_NONBLOCK) != 0 {
        status_flags |= O_NONBLOCK as u32;
    }

    if abi.set_file_status_flags(read_fd, status_flags).is_err() {
        let _ = abi.remove_fd(write_fd);
        let _ = abi.remove_fd(read_fd);
        let _ = task.handle_table.remove(write_handle);
        let _ = task.handle_table.remove(read_handle);
        return errno::to_result(errno::EMFILE);
    }
    if abi.set_file_status_flags(write_fd, status_flags).is_err() {
        let _ = abi.remove_fd(write_fd);
        let _ = abi.remove_fd(read_fd);
        let _ = task.handle_table.remove(write_handle);
        let _ = task.handle_table.remove(read_handle);
        return errno::to_result(errno::EMFILE);
    }

    if (flags & O_CLOEXEC) != 0 {
        let _ = abi.set_fd_flags(read_fd, FD_CLOEXEC);
        let _ = abi.set_fd_flags(write_fd, FD_CLOEXEC);
    }

    if let Some(obj) = task.handle_table.get(read_handle) {
        if let Some(sel) = obj.as_selectable() {
            sel.set_nonblocking((flags & O_NONBLOCK) != 0);
        }
    }
    if let Some(obj) = task.handle_table.get(write_handle) {
        if let Some(sel) = obj.as_selectable() {
            sel.set_nonblocking((flags & O_NONBLOCK) != 0);
        }
    }

    unsafe {
        core::ptr::write_unaligned(pipefd_ptr, read_fd as u32);
        core::ptr::write_unaligned(pipefd_ptr.add(1), write_fd as u32);
    }

    0
}
