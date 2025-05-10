use crate::arch::*;

#[derive(Debug, Clone, Copy)]
pub enum Syscall {
    Invalid = 0,
    Exit = 1,
    Clone = 2,
    Execve = 3,
    Waitpid = 4,
    Kill = 5,
    Getpid = 6,
    Getppid = 7,
    Brk = 12,
    Sbrk = 13,
    // BASIC I/O
    Putchar = 16,
    Getchar = 17,
    // File operations
    Open = 20,
    Close = 21,
    Read = 22,
    Write = 23,
    Lseek = 24,
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
