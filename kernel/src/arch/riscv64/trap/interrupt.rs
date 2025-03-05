use crate::arch::Trapframe;
use crate::sched::scheduler::get_scheduler;

pub fn arch_interrupt_handler(trapframe: &mut Trapframe, cause: usize) {
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