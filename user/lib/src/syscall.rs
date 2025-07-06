use crate::arch::*;

#[derive(Debug, Clone, Copy)]
pub enum Syscall {
    Invalid = 0,
    Exit = 1,
    Clone = 2,
    Execve = 3,
    ExecveABI = 4,
    Waitpid = 5,
    Kill = 6,
    Getpid = 7,
    Getppid = 8,
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
    Ftruncate = 25,
    Truncate = 26,
    // Filesystem operations
    Mkfile = 30,
    Mkdir = 31,
    // Mount operations
    Mount = 32,
    Umount = 33,
    PivotRoot = 34,
    // Change directory
    Chdir = 35,
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

pub fn syscall4(syscall: Syscall, arg1: usize, arg2: usize, arg3: usize, arg4: usize) -> usize {
    arch_syscall4(syscall, arg1, arg2, arg3, arg4)
}

pub fn syscall5(syscall: Syscall, arg1: usize, arg2: usize, arg3: usize, arg4: usize, arg5: usize) -> usize {
    arch_syscall5(syscall, arg1, arg2, arg3, arg4, arg5)
}
