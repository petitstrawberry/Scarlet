//! FreeBSD RISC-V 64 memory management syscalls
//!

use crate::{
    abi::freebsd::riscv64::{errno::FreeBsdErrno, FreeBsdRiscv64Abi},
    arch::Trapframe,
    task::mytask,
};

/// FreeBSD mmap syscall (stub implementation)
/// TODO: Implement full mmap with proper memory mapping similar to Linux ABI
pub fn sys_mmap(_abi: &mut FreeBsdRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return FreeBsdErrno::ESRCH.as_error(),
    };

    let _addr = trapframe.get_arg(0);
    let length = trapframe.get_arg(1);
    let _prot = trapframe.get_arg(2);
    let _flags = trapframe.get_arg(3);
    let _fd = trapframe.get_arg(4) as isize;
    let _offset = trapframe.get_arg(5);

    trapframe.increment_pc_next(task);

    // Input validation
    if length == 0 {
        return FreeBsdErrno::EINVAL.as_error();
    }

    // Stub: For now, return ENOSYS (not implemented)
    // A full implementation would:
    // 1. Validate input parameters
    // 2. Allocate physical pages for anonymous mappings
    // 3. Create virtual memory mappings
    // 4. Handle file-backed mappings
    // 5. Set appropriate permissions
    FreeBsdErrno::ENOSYS.as_error()
}

/// FreeBSD munmap syscall (stub implementation)
/// TODO: Implement full munmap with proper memory unmapping
pub fn sys_munmap(_abi: &mut FreeBsdRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return FreeBsdErrno::ESRCH.as_error(),
    };

    let _addr = trapframe.get_arg(0);
    let _length = trapframe.get_arg(1);

    trapframe.increment_pc_next(task);

    // Stub: return success for now
    0
}

/// FreeBSD mprotect syscall (stub implementation)
/// TODO: Implement full mprotect with proper permission changes
pub fn sys_mprotect(_abi: &mut FreeBsdRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return FreeBsdErrno::ESRCH.as_error(),
    };

    let _addr = trapframe.get_arg(0);
    let _length = trapframe.get_arg(1);
    let _prot = trapframe.get_arg(2);

    trapframe.increment_pc_next(task);

    // Stub: return success for now
    0
}
