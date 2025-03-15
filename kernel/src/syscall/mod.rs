
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
