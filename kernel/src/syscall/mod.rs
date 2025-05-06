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
use crate::task::syscall::{sys_brk, sys_clone, sys_execve, sys_exit, sys_getpid, sys_getppid, sys_putchar, sys_sbrk, sys_waitpid};

#[macro_use]
mod macros;

syscall_table! {
    Invalid = 0 => |_: &mut Trapframe| {
        0
    },
    Exit = 1 => sys_exit,
    Clone = 2 => sys_clone,
    Execve = 3 => sys_execve,
    Waitpid = 4 => sys_waitpid,
    Getpid = 6 => sys_getpid,
    Getppid = 7 => sys_getppid,
    Brk = 12 => sys_brk,
    Sbrk = 13 => sys_sbrk,
    Putchar = 16 => sys_putchar,
}
