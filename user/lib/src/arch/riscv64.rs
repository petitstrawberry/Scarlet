use core::arch::{asm, naked_asm};

use crate::{syscall::Syscall, task::exit};

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
pub fn _start() {
    let ret = unsafe { main() };
    exit(ret as i32);
}

pub fn arch_syscall0(syscall: Syscall) -> usize{
    let mut ret: usize;
    unsafe {
        asm!(
        "mv a7, {syscall}
        ecall",
        syscall = in(reg) syscall as usize,
        out("a0") ret,
        options(nostack)
        );
    }
    ret
}

pub fn arch_syscall1(syscall: Syscall, arg1: usize) -> usize{
    let mut ret: usize;
    unsafe {
        asm!(
        "mv a7, {syscall}
        mv a0, {arg1}
        ecall",
        syscall = in(reg) syscall as usize,
        arg1 = in(reg) arg1,
        out("a0") ret,
        options(nostack)
        );
    }
    ret
}

pub fn arch_syscall2(syscall: Syscall, arg1: usize, arg2: usize) -> usize{
    let mut ret: usize;
    unsafe {
        asm!(
        "mv a7, {syscall}
        mv a0, {arg1}
        mv a1, {arg2}
        ecall",
        syscall = in(reg) syscall as usize,
        arg1 = in(reg) arg1,
        arg2 = in(reg) arg2,
        out("a0") ret,
        options(nostack)
        );
    }
    ret
}

pub fn arch_syscall3(syscall: Syscall, arg1: usize, arg2: usize, arg3: usize) -> usize{
    let mut ret: usize;
    unsafe {
        asm!(
        "mv a7, {syscall}
        mv a0, {arg1}
        mv a1, {arg2}
        mv a2, {arg3}
        ecall",
        syscall = in(reg) syscall as usize,
        arg1 = in(reg) arg1,
        arg2 = in(reg) arg2,
        arg3 = in(reg) arg3,
        out("a0") ret,
        options(nostack)
        );
    }
    ret
}

pub fn arch_syscall4(syscall: Syscall, arg1: usize, arg2: usize, arg3: usize, arg4: usize) -> usize {
    let mut ret: usize;
    unsafe {
        asm!(
        "mv a7, {syscall}
        mv a0, {arg1}
        mv a1, {arg2}
        mv a2, {arg3}
        mv a3, {arg4}
        ecall",
        syscall = in(reg) syscall as usize,
        arg1 = in(reg) arg1,
        arg2 = in(reg) arg2,
        arg3 = in(reg) arg3,
        arg4 = in(reg) arg4,
        out("a0") ret,
        options(nostack)
        );
    }
    ret
}

pub fn arch_syscall5(syscall: Syscall, arg1: usize, arg2: usize, arg3: usize, arg4: usize, arg5: usize) -> usize {
    let mut ret: usize;
    unsafe {
        asm!(
        "mv a7, {syscall}
        mv a0, {arg1}
        mv a1, {arg2}
        mv a2, {arg3}
        mv a3, {arg4}
        mv a4, {arg5}
        ecall",
        syscall = in(reg) syscall as usize,
        arg1 = in(reg) arg1,
        arg2 = in(reg) arg2,
        arg3 = in(reg) arg3,
        arg4 = in(reg) arg4,
        arg5 = in(reg) arg5,
        out("a0") ret,
        options(nostack)
        );
    }
    ret
}