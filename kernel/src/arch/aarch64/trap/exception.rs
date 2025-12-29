use core::arch::asm;
use core::sync::atomic::{AtomicUsize, Ordering};

use super::print_traplog;
use crate::abi::syscall_dispatcher;
use crate::arch::Trapframe;
use crate::arch::get_cpu;
use crate::interrupt::InterruptManager;
use crate::sched::scheduler::get_scheduler;
use crate::task::mytask;
use crate::timer;

/// AArch64 EC (Exception Class) values from ESR_EL1.
const EC_SVC64: u64 = 0x15;
const EC_INSN_ABORT_LOWER_EL: u64 = 0x20;
const EC_DATA_ABORT_LOWER_EL: u64 = 0x24;

// Bring-up aid: log only a small number of user writes to confirm forward progress,
// without reintroducing the per-SVC log flood.
static SVC_STREAMWRITE_LOG_BUDGET: AtomicUsize = AtomicUsize::new(32);

fn try_take_streamwrite_log_budget() -> bool {
    let mut budget = SVC_STREAMWRITE_LOG_BUDGET.load(Ordering::Relaxed);
    while budget > 0 {
        match SVC_STREAMWRITE_LOG_BUDGET.compare_exchange(
            budget,
            budget - 1,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return true,
            Err(new_budget) => budget = new_budget,
        }
    }
    false
}

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
            let syscall_nr = trapframe.get_syscall_number();
            
            match syscall_dispatcher(trapframe) {
                Ok(ret) => {
                    trapframe.set_return_value(ret);
                    if syscall_nr == 0xd {
                        let brk_before = mytask().map(|t| t.get_brk()).unwrap_or(0);
                        let brk_after = mytask().map(|t| t.get_brk()).unwrap_or(0);
                        crate::early_println!(
                            "[aarch64][svc][sbrk] ret={:#x} brk_before={:#x} brk_after={:#x}",
                            ret,
                            brk_before,
                            brk_after,
                        );
                    }
                }
                Err(_msg) => {
                    // Keep this path noisy for bring-up, but stay quiet on success.
                    // Follow the existing convention: successful syscalls advance PC
                    // inside the syscall implementation; on error, ensure we don't
                    // re-execute SVC forever.
                    crate::early_println!(
                        "[aarch64][svc][err] nr={:#x} epc={:#x} arg0={:#x} arg1={:#x} arg2={:#x}",
                        syscall_nr,
                        trapframe.epc,
                        trapframe.get_arg(0),
                        trapframe.get_arg(1),
                        trapframe.get_arg(2),
                    );
                    print_traplog(trapframe);
                    trapframe.set_return_value(usize::MAX); // -1
                    trapframe.increment_pc_next(mytask().unwrap());
                }
            }
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
                print_traplog(trapframe);
                panic!(
                    "[aarch64] NULL pointer access or zero stack pointer detected (ec={:#x} esr={:#x} far={:#x} epc={:#x} sp={:#x})",
                    ec,
                    esr,
                    far,
                    trapframe.epc,
                    trapframe.regs.reg[31]
                );
            }

            let task = get_scheduler()
                .get_current_task(get_cpu().get_cpuid())
                .expect("No current task for fault handling");

            let manager = &task.vm_manager;

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
                print_traplog(trapframe);
                panic!(
                    "[aarch64] Failed to map page for abort at vaddr={:#x} (ec={:#x} esr={:#x} far={:#x} epc={:#x}): {}",
                    fault_addr,
                    ec,
                    esr,
                    far,
                    trapframe.epc,
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
