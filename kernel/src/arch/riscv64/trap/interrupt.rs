use crate::arch::Trapframe;
use crate::sched::scheduler::get_scheduler;
use crate::interrupt::InterruptManager;

/// RISC-V S-mode interrupt causes
const SUPERVISOR_SOFTWARE_INTERRUPT: usize = 1;
const SUPERVISOR_TIMER_INTERRUPT: usize = 5;
const SUPERVISOR_EXTERNAL_INTERRUPT: usize = 9;

pub fn arch_interrupt_handler(trapframe: &mut Trapframe, cause: usize) {
    match cause {
        SUPERVISOR_SOFTWARE_INTERRUPT => handle_software_interrupt(),
        SUPERVISOR_TIMER_INTERRUPT => handle_timer_interrupt(trapframe),
        SUPERVISOR_EXTERNAL_INTERRUPT => handle_external_interrupt(trapframe),
        _ => handle_unknown_interrupt(trapframe, cause),
    }
}

/// Handle software interrupt (IPI)
/// TODO: Implement inter-processor interrupt handling
fn handle_software_interrupt() {
    crate::early_println!("[interrupt] Software interrupt received - TODO: implement IPI");
    // TODO: CLINT software interrupt handling
    // TODO: Inter-processor interrupt (IPI) support
}

/// Handle timer interrupt from CLINT
fn handle_timer_interrupt(trapframe: &mut Trapframe) {    
    // Call the existing scheduler
    let scheduler = get_scheduler();
    scheduler.schedule(trapframe);
}

/// Handle external interrupt from PLIC
fn handle_external_interrupt(trapframe: &mut Trapframe) {
    let cpu_id = trapframe.get_cpuid() as u32;

    // Claim and handle external interrupt through PLIC
    match InterruptManager::with_manager(|mgr| {
        mgr.claim_and_handle_external_interrupt(cpu_id)
    }) {
        Ok(Some(interrupt_id)) => {
            crate::early_println!("[interrupt] Handled external interrupt {} on CPU {}", interrupt_id, cpu_id);
        }
        Ok(None) => {
            crate::early_println!("[interrupt] No pending external interrupt on CPU {}", cpu_id);
        }
        Err(e) => {
            crate::early_println!("[interrupt] Failed to handle external interrupt: {}", e);
        }
    }
}

/// Handle unknown interrupt
fn handle_unknown_interrupt(trapframe: &mut Trapframe, cause: usize) {
    crate::early_println!("[interrupt] Unknown interrupt trapframe: {:x?}", trapframe);
    panic!("Unknown interrupt cause: {}", cause);
}