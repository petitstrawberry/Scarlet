use crate::abi::freebsd::riscv64::{errno::FreeBsdErrno, FreeBsdRiscv64Abi};
use crate::arch::Trapframe;
use crate::task::mytask;

/// sys_write - write to a file descriptor
pub fn sys_write(abi: &mut FreeBsdRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return FreeBsdErrno::ESRCH.as_error(),
    };

    let fd = trapframe.get_arg(0);
    let buf_ptr = match task.vm_manager.translate_vaddr(trapframe.get_arg(1)) {
        Some(p) => p as *const u8,
        None => return FreeBsdErrno::EFAULT.as_error(),
    };
    let count = trapframe.get_arg(2);

    // Increment PC to avoid infinite loop if write fails
    trapframe.increment_pc_next(task);

    // Get handle from file descriptor
    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => return FreeBsdErrno::EBADF.as_error(),
    };

    // Get the kernel object from handle table
    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return FreeBsdErrno::EBADF.as_error(),
    };

    let stream = match kernel_obj.as_stream() {
        Some(s) => s,
        None => return FreeBsdErrno::EBADF.as_error(),
    };

    // Write to the stream
    let buffer = unsafe { core::slice::from_raw_parts(buf_ptr, count) };
    match stream.write(buffer) {
        Ok(written) => written,
        Err(_) => FreeBsdErrno::EIO.as_error(),
    }
}

/// sys_read - read from a file descriptor
pub fn sys_read(abi: &mut FreeBsdRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return FreeBsdErrno::ESRCH.as_error(),
    };

    let fd = trapframe.get_arg(0);
    let buf_ptr = match task.vm_manager.translate_vaddr(trapframe.get_arg(1)) {
        Some(p) => p as *mut u8,
        None => return FreeBsdErrno::EFAULT.as_error(),
    };
    let count = trapframe.get_arg(2);

    // Increment PC to avoid infinite loop if read fails
    trapframe.increment_pc_next(task);

    // Get handle from file descriptor
    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => return FreeBsdErrno::EBADF.as_error(),
    };

    // Get the kernel object from handle table
    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return FreeBsdErrno::EBADF.as_error(),
    };

    let stream = match kernel_obj.as_stream() {
        Some(s) => s,
        None => return FreeBsdErrno::EBADF.as_error(),
    };

    // Read from the stream
    let buffer = unsafe { core::slice::from_raw_parts_mut(buf_ptr, count) };
    match stream.read(buffer) {
        Ok(bytes_read) => bytes_read,
        Err(_) => FreeBsdErrno::EIO.as_error(),
    }
}

/// sys_open - open a file (simplified stub implementation)
pub fn sys_open(abi: &mut FreeBsdRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return FreeBsdErrno::ESRCH.as_error(),
    };

    let _path_ptr = trapframe.get_arg(0);
    let _flags = trapframe.get_arg(1);
    let _mode = trapframe.get_arg(2);

    // Increment PC
    trapframe.increment_pc_next(task);

    // For now, return ENOSYS (not implemented)
    // A full implementation would need to:
    // 1. Read path string from user space
    // 2. Resolve the path in the VFS
    // 3. Open the file with the correct flags
    // 4. Create a handle and allocate a file descriptor
    FreeBsdErrno::ENOSYS.as_error()
}

/// sys_close - close a file descriptor
pub fn sys_close(abi: &mut FreeBsdRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return FreeBsdErrno::ESRCH.as_error(),
    };

    let fd = trapframe.get_arg(0);

    // Increment PC
    trapframe.increment_pc_next(task);

    // Get handle from file descriptor
    let handle = match abi.remove_fd(fd) {
        Some(h) => h,
        None => return FreeBsdErrno::EBADF.as_error(),
    };

    // Remove from handle table
    match task.handle_table.remove(handle) {
        Some(_) => 0,
        None => FreeBsdErrno::EBADF.as_error(),
    }
}
