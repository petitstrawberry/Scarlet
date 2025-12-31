//! AArch64 interrupt trap handling

use crate::arch::{Trapframe, get_cpu};
use crate::interrupt::InterruptManager;

/// Handle an IRQ taken at EL1.
///
/// On QEMU virt (GICv2), the EL1 physical timer arrives as a PPI (typically ID 30).
/// The generic InterruptManager path will acknowledge+EOI the interrupt, but the
/// kernel still needs to run the timer tick logic to advance scheduling.
pub fn arch_irq_handler(trapframe: &mut Trapframe) {
    let cpu_id = get_cpu().get_cpuid() as u32;

    let claimed = InterruptManager::with_manager(|mgr| mgr.claim_and_handle_external_interrupt(cpu_id));

    match claimed {
        Ok(Some(interrupt_id)) => {
            if interrupt_id == crate::drivers::pic::arm_generic_timer::CNTP_PPI_IRQ {
                crate::timer::tick(trapframe);
            }
        }
        Ok(None) => {
            // Fallback: if the architectural timer is pending, run tick.
            // (This can help when the IRQ path didn't surface through the external controller.)
            if crate::drivers::pic::arm_generic_timer::ArmGenericTimer::is_timer_pending() {
                crate::timer::tick(trapframe);
            }
        }
        Err(e) => {
            crate::early_println!("[aarch64][irq] failed to claim/handle external interrupt: {}", e);
        }
    }
}
