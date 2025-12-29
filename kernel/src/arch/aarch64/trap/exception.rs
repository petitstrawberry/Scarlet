use core::arch::asm;

use super::print_traplog;
use crate::arch::Trapframe;
use crate::arch::get_cpu;
use crate::interrupt::InterruptManager;
use crate::sched::scheduler::get_scheduler;
use crate::timer;

/// AArch64 EC (Exception Class) values from ESR_EL1.
const EC_SVC64: u64 = 0x15;
const EC_INSN_ABORT_LOWER_EL: u64 = 0x20;
const EC_DATA_ABORT_LOWER_EL: u64 = 0x24;

pub fn arch_exception_handler(trapframe: &mut Trapframe) {
    let esr: u64;
    unsafe { asm!("mrs {0}, esr_el1", out(reg) esr, options(nostack)); }

    let ec = (esr >> 26) & 0x3f;

    match ec {
        // IRQ and some non-synchronous events report EC=0.
        // Our trampoline vector currently funnels all exception classes through
        // the same entry path, so we do best-effort demux here.
        0 => {
            // 1) External interrupts via the registered controller (e.g. GIC)
            // IMPORTANT: claim_and_handle completes (EOI) before we proceed.
            // This is needed because timer::tick() may context-switch and not return.
            let cpu_id = get_cpu().get_cpuid() as u32;

            let claimed = InterruptManager::with_manager(|mgr| {
                mgr.claim_and_handle_external_interrupt(cpu_id).ok().flatten()
            });

            if let Some(id) = claimed {
                if id == crate::drivers::pic::arm_generic_timer::CNTP_PPI_IRQ {
                    timer::tick(trapframe);
                }
                return;
            }

            // 2) Local timer pending (best-effort fallback for non-GIC wiring)
            if crate::drivers::pic::arm_generic_timer::ArmGenericTimer::is_timer_pending() {
                timer::tick(trapframe);
                return;
            }
        }
        EC_SVC64 => {
            print_traplog(trapframe);
            panic!(
                "[aarch64] syscall (SVC64) is disabled: esr={:#x} epc={:#x} nr(x8)={:#x}",
                esr,
                trapframe.epc,
                trapframe.get_syscall_number(),
            );
        }
        EC_INSN_ABORT_LOWER_EL | EC_DATA_ABORT_LOWER_EL => {
            let far: usize;
            unsafe { asm!("mrs {0}, far_el1", out(reg) far, options(nostack)); }

            // For instruction aborts, FAR_EL1 is expected to match ELR_EL1, but during bring-up
            // we occasionally observe mismatches in logs. Use EPC (ELR_EL1) as the authoritative
            // fault VA for instruction abort handling.
            let fault_addr: usize = if ec == EC_INSN_ABORT_LOWER_EL {
                trapframe.epc as usize
            } else {
                far
            };

            if fault_addr == 0 || trapframe.regs.reg[31] == 0 {
                panic!("[aarch64] NULL pointer access or zero stack pointer detected");
            }

            let task = get_scheduler()
                .get_current_task(get_cpu().get_cpuid())
                .expect("No current task for fault handling");

            let manager = &task.vm_manager;
            if manager.search_memory_map(fault_addr).is_none() {
                print_traplog(trapframe);
                panic!(
                    "[aarch64] VMA not found for fault_addr={:#x} (ec={:#x} esr={:#x} far={:#x} epc={:#x})",
                    fault_addr,
                    ec,
                    esr,
                    far,
                    trapframe.epc,
                );
            }

            use crate::object::capability::memory_mapping::{AccessKind, AccessOp};

            let op = if ec == EC_INSN_ABORT_LOWER_EL {
                AccessOp::Instruction
            } else {
                // Best-effort: treat as Store if WnR is set, else Load.
                let wnr = ((esr >> 6) & 0x1) != 0;
                if wnr {
                    AccessOp::Store
                } else {
                    AccessOp::Load
                }
            };

            let access = AccessKind {
                op,
                vaddr: fault_addr,
                size: None,
            };
            if let Err(e) = manager.lazy_map_page_with(access) {
                panic!(
                    "[aarch64] Failed to map page for abort at vaddr={:#x} (ec={:#x} esr={:#x}): {}",
                    fault_addr,
                    ec,
                    esr,
                    e
                );
            }
        }
        _ => {
            print_traplog(trapframe);
            panic!("Unhandled AArch64 exception: ec={:#x} esr={:#x}", ec, esr);
        }
    }
}
