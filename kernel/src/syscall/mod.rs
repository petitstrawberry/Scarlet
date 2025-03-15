
use crate::arch::Trapframe;

#[macro_use]
mod macros;

syscall_table! {
    Invalid = 0 => |_: &mut Trapframe| {
        0
    },
}
