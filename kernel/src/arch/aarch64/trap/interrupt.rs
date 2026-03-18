//! AArch64 interrupt trap handling

use crate::arch::{Trapframe, get_cpu};
use crate::interrupt::InterruptManager;

/// Handle an IRQ taken at EL1.
///
/// On QEMU virt (GICv2), the virtual timer arrives as a PPI (typically ID 27).
/// The generic InterruptManager path will acknowledge+EOI the interrupt, but the
/// kernel still needs to run the timer tick logic to advance scheduling.
pub fn arch_irq_handler(trapframe: &mut Trapframe, trap_kind: usize) {
    let cpu_id = get_cpu().get_cpuid() as u32;

    if trap_kind == 2 && crate::arch::interrupt::is_arch_timer_pending() {
        crate::timer::tick(trapframe);
        return;
    }

    let claimed =
        InterruptManager::with_manager(|mgr| mgr.claim_and_handle_external_interrupt(cpu_id));

    match claimed {
        Ok(Some(interrupt_id)) => {
            if interrupt_id == crate::drivers::pic::arm_generic_timer::CNTV_PPI_IRQ {
                crate::timer::tick(trapframe);
            }
        }
        Ok(None) => {
            if crate::arch::interrupt::is_arch_timer_pending() {
                crate::timer::tick(trapframe);
            }
        }
        Err(e) => {
            crate::println!(
                "[aarch64][irq] failed to claim/handle external interrupt: {}",
                e
            );
        }
    }
}
