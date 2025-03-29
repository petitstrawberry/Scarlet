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
use crate::task::syscall::{sys_brk, sys_sbrk};

#[macro_use]
mod macros;

syscall_table! {
    Invalid = 0 => |_: &mut Trapframe| {
        0
    },
    Brk = 12 => sys_brk,
    Sbrk = 13 => sys_sbrk,
}
