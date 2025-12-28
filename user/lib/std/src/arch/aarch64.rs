use core::arch::{asm, naked_asm};

use crate::{env, syscall::Syscall, task::exit};

#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_entry")]
#[unsafe(naked)]
pub extern "C" fn _entry() {
    unsafe {
        naked_asm!(
            "
        .align 8
                b       _start
        ",
        );
    }
}

unsafe extern "Rust" {
    fn main() -> i32;
}

#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_start")]
pub fn _start(x0: usize, x1: usize) -> ! {
    // x0 = argc, x1 = argv (set by kernel's ScarletAbi)
    let argc = x0;
    let argv = x1 as *const *const u8;

    unsafe {
        let envp = if !argv.is_null() && argc > 0 {
            argv.add(argc + 1) as *const *const u8
        } else {
            core::ptr::null()
        };

        env::init_env(argc, argv, envp);

        let ret = main();
        exit(ret as i32);
    }
}

pub fn arch_syscall0(syscall: Syscall) -> usize {
    let mut ret: usize;
    unsafe {
        asm!(
            "svc #0",
            in("x8") syscall as usize,
            lateout("x0") ret,
            clobber_abi("C"),
            options(nostack)
        );
    }
    ret
}

pub fn arch_syscall1(syscall: Syscall, arg1: usize) -> usize {
    let mut ret: usize;
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

pub fn arch_syscall2(syscall: Syscall, arg1: usize, arg2: usize) -> usize {
    let mut ret: usize;
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

pub fn arch_syscall3(syscall: Syscall, arg1: usize, arg2: usize, arg3: usize) -> usize {
    let mut ret: usize;
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

pub fn arch_syscall4(
    syscall: Syscall,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
) -> usize {
    let mut ret: usize;
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

pub fn arch_syscall5(
    syscall: Syscall,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
) -> usize {
    let mut ret: usize;
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

pub fn arch_syscall6(
    syscall: Syscall,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
    arg6: usize,
) -> usize {
    let mut ret: usize;
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
