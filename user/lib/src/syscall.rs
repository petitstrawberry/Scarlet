use crate::arch::*;

#[derive(Debug, Clone, Copy)]
pub enum Syscall {
    Invalid = 0,
    Exit = 1,
    Clone = 2,
    Execve = 3,
    Brk = 12,
    Sbrk = 13,
    Putchar = 16,
}

pub fn syscall0(syscall: Syscall) -> usize {
    arch_syscall0(syscall)
}

pub fn syscall1(syscall: Syscall, arg1: usize) -> usize {
    arch_syscall1(syscall, arg1)
}

pub fn syscall2(syscall: Syscall, arg1: usize, arg2: usize) -> usize {
    arch_syscall2(syscall, arg1, arg2)
}

pub fn syscall3(syscall: Syscall, arg1: usize, arg2: usize, arg3: usize) -> usize {
    arch_syscall3(syscall, arg1, arg2, arg3)
}
