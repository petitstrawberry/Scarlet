use crate::arch::Trapframe;
use crate::{print, println};

use super::mytask;

pub fn sys_brk(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let brk = trapframe.get_arg(0);
    match task.set_brk(brk) {
        Ok(_) => task.get_brk(),
        Err(_) => usize::MAX, /* -1 */
    }
}

pub fn sys_sbrk(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let increment = trapframe.get_arg(0); // Change to get_arg(0)
    let brk = task.get_brk();
    match task.set_brk(unsafe { brk.unchecked_add(increment) }) {
        Ok(_) => brk,
        Err(_) => usize::MAX, /* -1 */
    }
}

pub fn sys_putchar(trapframe: &mut Trapframe) -> usize {
    let c = trapframe.get_arg(0) as u32;
    if let Some(ch) = char::from_u32(c) {
        print!("{}", ch);
    } else {
        println!("[sys_putchar] invalid char: {}", c);
    }
    0
}