use core::arch::asm;

use scarlet_abi::Syscall;

pub fn syscall0(syscall: Syscall) -> usize {
    let ret;
    // SAFETY: Scarlet Native RISC-V syscalls use a7 for the syscall number,
    // a0-a5 for arguments, and return through a0. The inline asm does not
    // access memory or the stack beyond the declared ABI clobbers.
    unsafe {
        asm!(
            "ecall",
            in("a7") syscall as usize,
            out("a0") ret,
            clobber_abi("C"),
            options(nostack)
        );
    }
    ret
}

pub fn syscall1(syscall: Syscall, arg1: usize) -> usize {
    let ret;
    // SAFETY: See `syscall0`; this additionally places arg1 in a0.
    unsafe {
        asm!(
            "ecall",
            in("a7") syscall as usize,
            inlateout("a0") arg1 => ret,
            clobber_abi("C"),
            options(nostack)
        );
    }
    ret
}

pub fn syscall2(syscall: Syscall, arg1: usize, arg2: usize) -> usize {
    let ret;
    // SAFETY: See `syscall0`; this additionally places arg1-arg2 in a0-a1.
    unsafe {
        asm!(
            "ecall",
            in("a7") syscall as usize,
            inlateout("a0") arg1 => ret,
            in("a1") arg2,
            clobber_abi("C"),
            options(nostack)
        );
    }
    ret
}

pub fn syscall3(syscall: Syscall, arg1: usize, arg2: usize, arg3: usize) -> usize {
    let ret;
    // SAFETY: See `syscall0`; this additionally places arg1-arg3 in a0-a2.
    unsafe {
        asm!(
            "ecall",
            in("a7") syscall as usize,
            inlateout("a0") arg1 => ret,
            in("a1") arg2,
            in("a2") arg3,
            clobber_abi("C"),
            options(nostack)
        );
    }
    ret
}

pub fn syscall4(syscall: Syscall, arg1: usize, arg2: usize, arg3: usize, arg4: usize) -> usize {
    let ret;
    // SAFETY: See `syscall0`; this additionally places arg1-arg4 in a0-a3.
    unsafe {
        asm!(
            "ecall",
            in("a7") syscall as usize,
            inlateout("a0") arg1 => ret,
            in("a1") arg2,
            in("a2") arg3,
            in("a3") arg4,
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
    let ret;
    // SAFETY: See `syscall0`; this additionally places arg1-arg5 in a0-a4.
    unsafe {
        asm!(
            "ecall",
            in("a7") syscall as usize,
            inlateout("a0") arg1 => ret,
            in("a1") arg2,
            in("a2") arg3,
            in("a3") arg4,
            in("a4") arg5,
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
    let ret;
    // SAFETY: See `syscall0`; this additionally places arg1-arg6 in a0-a5.
    unsafe {
        asm!(
            "ecall",
            in("a7") syscall as usize,
            inlateout("a0") arg1 => ret,
            in("a1") arg2,
            in("a2") arg3,
            in("a3") arg4,
            in("a4") arg5,
            in("a5") arg6,
            clobber_abi("C"),
            options(nostack)
        );
    }
    ret
}
