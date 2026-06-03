use core::arch::asm;

use scarlet_abi::Syscall;

pub fn syscall0(syscall: Syscall) -> usize {
    let ret: usize;
    // SAFETY: Scarlet Native AArch64 syscalls use x8 for the syscall number,
    // x0-x5 for arguments, and return through x0. The inline asm does not
    // access memory or the stack beyond the declared ABI clobbers.
    unsafe {
        asm!(
            "svc #0",
            in("x8") syscall as usize,
            out("x0") ret,
            clobber_abi("C"),
            options(nostack)
        );
    }
    ret
}

pub fn syscall1(syscall: Syscall, arg1: usize) -> usize {
    let ret: usize;
    // SAFETY: See `syscall0`; this additionally places arg1 in x0.
    unsafe {
        asm!(
            "svc #0",
            in("x8") syscall as usize,
            inlateout("x0") arg1 => ret,
            clobber_abi("C"),
            options(nostack)
        );
    }
    ret
}

pub fn syscall2(syscall: Syscall, arg1: usize, arg2: usize) -> usize {
    let ret: usize;
    // SAFETY: See `syscall0`; this additionally places arg1-arg2 in x0-x1.
    unsafe {
        asm!(
            "svc #0",
            in("x8") syscall as usize,
            inlateout("x0") arg1 => ret,
            in("x1") arg2,
            clobber_abi("C"),
            options(nostack)
        );
    }
    ret
}

pub fn syscall3(syscall: Syscall, arg1: usize, arg2: usize, arg3: usize) -> usize {
    let ret: usize;
    // SAFETY: See `syscall0`; this additionally places arg1-arg3 in x0-x2.
    unsafe {
        asm!(
            "svc #0",
            in("x8") syscall as usize,
            inlateout("x0") arg1 => ret,
            in("x1") arg2,
            in("x2") arg3,
            clobber_abi("C"),
            options(nostack)
        );
    }
    ret
}

pub fn syscall4(syscall: Syscall, arg1: usize, arg2: usize, arg3: usize, arg4: usize) -> usize {
    let ret: usize;
    // SAFETY: See `syscall0`; this additionally places arg1-arg4 in x0-x3.
    unsafe {
        asm!(
            "svc #0",
            in("x8") syscall as usize,
            inlateout("x0") arg1 => ret,
            in("x1") arg2,
            in("x2") arg3,
            in("x3") arg4,
            clobber_abi("C"),
            options(nostack)
        );
    }
    ret
}

pub fn syscall5(
    syscall: Syscall,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
) -> usize {
    let ret: usize;
    // SAFETY: See `syscall0`; this additionally places arg1-arg5 in x0-x4.
    unsafe {
        asm!(
            "svc #0",
            in("x8") syscall as usize,
            inlateout("x0") arg1 => ret,
            in("x1") arg2,
            in("x2") arg3,
            in("x3") arg4,
            in("x4") arg5,
            clobber_abi("C"),
            options(nostack)
        );
    }
    ret
}

pub fn syscall6(
    syscall: Syscall,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
    arg6: usize,
) -> usize {
    let ret: usize;
    // SAFETY: See `syscall0`; this additionally places arg1-arg6 in x0-x5.
    unsafe {
        asm!(
            "svc #0",
            in("x8") syscall as usize,
            inlateout("x0") arg1 => ret,
            in("x1") arg2,
            in("x2") arg3,
            in("x3") arg4,
            in("x4") arg5,
            in("x5") arg6,
            clobber_abi("C"),
            options(nostack)
        );
    }
    ret
}
