//! System call interface module.
//! 
//! This module provides the system call interface for the Scarlet kernel.
//! It defines the system call table and the functions that handle various system
//! calls.
//! User programs can invoke these system calls to request services from the kernel.
//! 
//! ## System Call Table
//! 
//! The system call table is a mapping between system call numbers and their
//! corresponding handler functions. Each entry in the table is defined using the
//! `syscall_table!` macro.
//! 

use crate::arch::Trapframe;
use crate::fs::syscall::{sys_close, sys_ftruncate, sys_lseek, sys_open, sys_read, sys_truncate, sys_write};
use crate::task::syscall::{sys_brk, sys_clone, sys_execve, sys_execve_abi, sys_exit, sys_getchar, sys_getpid, sys_getppid, sys_putchar, sys_sbrk, sys_waitpid};

#[macro_use]
mod macros;

syscall_table! {
    Invalid = 0 => |_: &mut Trapframe| {
        0
    },
    Exit = 1 => sys_exit,
    Clone = 2 => sys_clone,
    Execve = 3 => sys_execve,
    ExecveABI = 4 => sys_execve_abi,
    Waitpid = 5 => sys_waitpid,
    Getpid = 7 => sys_getpid,
    Getppid = 8 => sys_getppid,
    Brk = 12 => sys_brk,
    Sbrk = 13 => sys_sbrk,
    // BASIC I/O
    Putchar = 16 => sys_putchar,
    Getchar = 17 => sys_getchar,
    // File operations
    Open = 20 => sys_open,
    Close = 21 => sys_close,
    Read = 22 => sys_read,
    Write = 23 => sys_write,
    Lseek = 24 => sys_lseek,
    Ftruncate = 25 => sys_ftruncate,
    Truncate = 26 => sys_truncate,
}
