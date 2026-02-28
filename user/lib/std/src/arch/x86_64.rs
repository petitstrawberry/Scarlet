use core::arch::{asm, naked_asm};

use crate::{env, syscall::Syscall, task::exit};

#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_entry")]
#[unsafe(naked)]
pub extern "C" fn _entry() {
    naked_asm!(
        "
        .align 16
        jmp _start
        ",
    );
}

unsafe extern "Rust" {
    fn main() -> i32;
}

#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_start")]
pub fn _start(a0: usize, a1: usize) -> ! {
    let argc = a0;
    let argv = a1 as *const *const u8;

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
    let mut ret;
    let syscall_num = syscall as usize;
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") syscall_num => ret,
            clobber_abi("C"),
            options(nostack)
        );
    }
    ret
}

pub fn arch_syscall1(syscall: Syscall, arg1: usize) -> usize {
    let mut ret;
    unsafe {
        asm!(
            "int 0x80",
            in("rax") syscall as usize,
            inlateout("rdi") arg1 => ret,
            clobber_abi("C"),
            options(nostack)
        );
    }
    ret
}

pub fn arch_syscall2(syscall: Syscall, arg1: usize, arg2: usize) -> usize {
    let mut ret;
    unsafe {
        asm!(
            "int 0x80",
            in("rax") syscall as usize,
            inlateout("rdi") arg1 => ret,
            in("rsi") arg2,
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
            "int 0x80",
            in("rax") syscall as usize,
            inlateout("rdi") arg1 => ret,
            in("rsi") arg2,
            in("rdx") arg3,
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
    let mut ret;
    unsafe {
        asm!(
            "int 0x80",
            in("rax") syscall as usize,
            inlateout("rdi") arg1 => ret,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
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
    let mut ret;
    unsafe {
        asm!(
            "int 0x80",
            in("rax") syscall as usize,
            inlateout("rdi") arg1 => ret,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
            in("r8") arg5,
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
    let mut ret;
    unsafe {
        asm!(
            "int 0x80",
            in("rax") syscall as usize,
            inlateout("rdi") arg1 => ret,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
            in("r8") arg5,
            in("r9") arg6,
            clobber_abi("C"),
            options(nostack)
        );
    }
    ret
}

#[inline]
pub fn arch_tls_pointer() -> usize {
    let fsbase;
    unsafe {
        asm!(
            "rdgsbase {}",
            out(reg) fsbase,
            options(nostack, pure, readonly)
        );
    }
    fsbase
}

pub fn arch_set_tls_pointer(ptr: usize) {
    crate::syscall::syscall1(crate::syscall::Syscall::SetTls, ptr);
}
