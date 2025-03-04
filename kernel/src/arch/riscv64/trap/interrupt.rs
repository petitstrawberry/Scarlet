use crate::arch::Arch;
use crate::sched::scheduler::get_scheduler;

pub fn arch_interrupt_handler(arch: &mut Arch, cause: usize) {
    match cause {
        5 => {
            let scheduler = get_scheduler();
            scheduler.schedule(arch);
        }
        _ => {
            loop {}
        }
    }
}