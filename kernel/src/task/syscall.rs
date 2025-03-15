use crate::arch::Trapframe;

use super::mytask;

pub fn sys_brk(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let brk = trapframe.get_arg(1);
    match task.set_brk(brk) {
        Ok(_) => task.get_brk(),
        Err(_) => usize::MAX,
    }
}

pub fn sys_sbrk(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let increment = trapframe.get_arg(1);
    let brk = task.get_brk();
    match task.set_brk(brk + increment) {
        Ok(_) => brk,
        Err(_) => usize::MAX,
    }
}