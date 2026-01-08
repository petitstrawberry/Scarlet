use crate::abi::freebsd::riscv64::{errno::FreeBsdErrno, FreeBsdRiscv64Abi};
use crate::arch::Trapframe;
use crate::task::mytask;

/// sys_exit - exit the current task
pub fn sys_exit(_abi: &mut FreeBsdRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let exit_code = trapframe.get_arg(0) as i32;
    crate::println!("FreeBSD sys_exit called with code: {}", exit_code);
    
    if let Some(task) = mytask() {
        task.exit(exit_code);
    }
    
    // This should not be reached as the task will exit
    0
}

/// sys_getpid - get process ID
pub fn sys_getpid(_abi: &mut FreeBsdRiscv64Abi, _trapframe: &mut Trapframe) -> usize {
    if let Some(task) = mytask() {
        task.get_id()
    } else {
        FreeBsdErrno::ESRCH.as_error()
    }
}

/// sys_getppid - get parent process ID
pub fn sys_getppid(_abi: &mut FreeBsdRiscv64Abi, _trapframe: &mut Trapframe) -> usize {
    if let Some(task) = mytask() {
        if let Some(parent_id) = task.get_parent_id() {
            parent_id
        } else {
            0 // No parent, return 0
        }
    } else {
        FreeBsdErrno::ESRCH.as_error()
    }
}

/// sys_getuid - get user ID
pub fn sys_getuid(_abi: &mut FreeBsdRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return FreeBsdErrno::ESRCH.as_error(),
    };

    trapframe.increment_pc_next(task);

    // Stub: return 0 (root)
    0
}

/// sys_geteuid - get effective user ID
pub fn sys_geteuid(_abi: &mut FreeBsdRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return FreeBsdErrno::ESRCH.as_error(),
    };

    trapframe.increment_pc_next(task);

    // Stub: return 0 (root)
    0
}

/// sys_getgid - get group ID
pub fn sys_getgid(_abi: &mut FreeBsdRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return FreeBsdErrno::ESRCH.as_error(),
    };

    trapframe.increment_pc_next(task);

    // Stub: return 0 (root)
    0
}

/// sys_getegid - get effective group ID
pub fn sys_getegid(_abi: &mut FreeBsdRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return FreeBsdErrno::ESRCH.as_error(),
    };

    trapframe.increment_pc_next(task);

    // Stub: return 0 (root)
    0
}

/// sys_brk - change data segment size
pub fn sys_brk(_abi: &mut FreeBsdRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return FreeBsdErrno::ESRCH.as_error(),
    };

    let addr = trapframe.get_arg(0);

    trapframe.increment_pc_next(task);

    // If addr is 0, return current brk
    if addr == 0 {
        return task.brk.unwrap_or(0x60000000);
    }

    // Set new brk
    task.brk = Some(addr);
    addr
}
