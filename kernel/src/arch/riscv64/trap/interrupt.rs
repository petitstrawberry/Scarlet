use crate::arch::TrapFrame;
use crate::sched::scheduler::get_scheduler;

pub fn arch_interrupt_handler(trapframe: &mut TrapFrame, cause: usize) {
    match cause {
        5 => {
            let scheduler = get_scheduler();
            scheduler.schedule(trapframe);
        }
        _ => {
            loop {}
        }
    }
}