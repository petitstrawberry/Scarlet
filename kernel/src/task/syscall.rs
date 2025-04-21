use alloc::task;

use crate::arch::{get_cpu, Trapframe};
use crate::print;
use crate::sched::scheduler::get_scheduler;

use super::mytask;

pub fn sys_brk(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let brk = trapframe.get_arg(0);
    trapframe.epc += 4;
    match task.set_brk(brk) {
        Ok(_) => task.get_brk(),
        Err(_) => usize::MAX, /* -1 */
    }
}

pub fn sys_sbrk(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let increment = trapframe.get_arg(0);
    let brk = task.get_brk();
    trapframe.epc += 4;
    match task.set_brk(unsafe { brk.unchecked_add(increment) }) {
        Ok(_) => brk,
        Err(_) => usize::MAX, /* -1 */
    }
}

pub fn sys_putchar(trapframe: &mut Trapframe) -> usize {
    let c = trapframe.get_arg(0) as u32;
    trapframe.epc += 4;
    if let Some(ch) = char::from_u32(c) {
        print!("{}", ch);
    } else {
        return usize::MAX; // -1
    }
    0
}

pub fn sys_exit(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let exit_code = trapframe.get_arg(0) as i32;
    task.exit(exit_code);
    get_scheduler().schedule(get_cpu());
    trapframe.get_arg(0) as usize
}

pub fn sys_clone(trapframe: &mut Trapframe) -> usize {
    let parent_task = mytask().unwrap();
    
    trapframe.epc += 4; /* Increment the program counter */

    /* Save the trapframe to the task before cloning */
    parent_task.vcpu.store(trapframe);
    
    /* Clone the task */
    match parent_task.clone_task() {
        Ok(mut child_task) => {
            let child_id = child_task.get_id();
            child_task.vcpu.regs.reg[10] = 0; /* Set the return value to 0 in the child task */
            get_scheduler().add_task(child_task, get_cpu().get_cpuid());
            /* Return the child task ID to the parent task */
            child_id
        },
        Err(_) => {
            usize::MAX /* Return -1 on error */
        }
    }
}
