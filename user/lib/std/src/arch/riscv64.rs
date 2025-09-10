use core::arch::{asm, naked_asm};

use crate::{syscall::Syscall, task::exit, env};

#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_entry")]
#[unsafe(naked)]
pub extern "C" fn _entry() {
    unsafe {
        naked_asm!("
        .option norvc
        .option norelax
        .align 8
                j       _start
        ",
        );
    }
}

unsafe extern "Rust" {
    fn main() -> i32;
}

#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_start")]
pub fn _start(a0: usize, a1: usize) -> ! {
    // Get argc and argv from RISC-V calling convention registers
    // a0 = argc, a1 = argv (set by kernel's ScarletAbi)
    let argc = a0;
    let argv = a1 as *const *const u8;

    unsafe {
        // Calculate envp from stack layout:
        // Stack layout: argc | argv[] | envp[] | argv_strings | envp_strings
        // envp starts right after argv[] (which has argc+1 entries including NULL)
        
        // Handle NULL argv case - if argv is NULL, envp calculation is not safe
        let envp = if !argv.is_null() && argc > 0 {
            argv.add(argc + 1) as *const *const u8
        } else {
            core::ptr::null()
        };
        
        // Initialize environment before calling main
        env::init_env(argc, argv, envp);
        
        let ret = main();
        exit(ret as i32);
    }
}

pub fn arch_syscall0(syscall: Syscall) -> usize{
    let mut ret;
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

pub fn arch_syscall1(syscall: Syscall, arg1: usize) -> usize{
    let mut ret;
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

pub fn arch_syscall2(syscall: Syscall, arg1: usize, arg2: usize) -> usize{
    let mut ret;
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

pub fn arch_syscall3(syscall: Syscall, arg1: usize, arg2: usize, arg3: usize) -> usize {
    let mut ret;
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

pub fn arch_syscall4(syscall: Syscall, arg1: usize, arg2: usize, arg3: usize, arg4: usize) -> usize {
    let mut ret;
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

pub fn arch_syscall5(syscall: Syscall, arg1: usize, arg2: usize, arg3: usize, arg4: usize, arg5: usize) -> usize {
    let mut ret;
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

pub fn arch_syscall6(syscall: Syscall, arg1: usize, arg2: usize, arg3: usize, arg4: usize, arg5: usize, arg6: usize) -> usize {
    let mut ret;
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