use core::arch::asm;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::abi::syscall_dispatcher;
use crate::arch::Trapframe;
use crate::arch::get_cpu;
use crate::interrupt::InterruptManager;
use crate::sched::scheduler::get_scheduler;
use crate::task::mytask;
use crate::timer;
use crate::{early_println, println};

/// AArch64 EC (Exception Class) values from ESR_EL1.
const EC_SVC64: u64 = 0x15;
const EC_INSN_ABORT_LOWER_EL: u64 = 0x20;
const EC_DATA_ABORT_LOWER_EL: u64 = 0x24;

// Keep early bring-up logs bounded.
static EXCEPTION_LOG_BUDGET: AtomicUsize = AtomicUsize::new(128);

// Abort (page fault) diagnostics are important and can be drowned out by frequent IRQs.
// Keep a separate budget so we can reliably print a few detailed abort logs.
static ABORT_LOG_BUDGET: AtomicUsize = AtomicUsize::new(32);

pub fn arch_exception_handler(trapframe: &mut Trapframe) {
    let esr: u64;
    unsafe { asm!("mrs {0}, esr_el1", out(reg) esr, options(nostack)); }

    let ec = (esr >> 26) & 0x3f;

    let should_log = EXCEPTION_LOG_BUDGET.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
        if v == 0 { None } else { Some(v - 1) }
    }).is_ok();

    if should_log {
        crate::early_println!("[aarch64] Exception handler invoked: EC={:#x}", ec);
    }

    match ec {
        // IRQ and some non-synchronous events report EC=0.
        // Our trampoline vector currently funnels all exception classes through
        // the same entry path, so we do best-effort demux here.
        0 => {
            // 1) External interrupts via the registered controller (e.g. GIC)
            // IMPORTANT: claim_and_handle completes (EOI) before we proceed.
            // This is needed because timer::tick() may context-switch and not return.
            let cpu_id = get_cpu().get_cpuid() as u32;
            
            if should_log {
                early_println!("[aarch64] EC=0, attempting to access InterruptManager...");
            }
            
            let claimed = InterruptManager::with_manager(|mgr| {
                if should_log {
                    early_println!("[aarch64] Inside InterruptManager, claiming interrupt...");
                }
                mgr.claim_and_handle_external_interrupt(cpu_id).ok().flatten()
            });
            
            if should_log {
                early_println!("[aarch64] InterruptManager access complete, claimed={:?}", claimed);
            }

            if let Some(id) = claimed {
                if id == crate::drivers::pic::arm_generic_timer::CNTP_PPI_IRQ {
                    if should_log {
                        early_println!("[aarch64] Timer interrupt (PPI 30) claimed, calling tick");
                    }
                    timer::tick(trapframe);
                }
                return;
            }

            // 2) Local timer pending (best-effort fallback for non-GIC wiring)
            if crate::drivers::pic::arm_generic_timer::ArmGenericTimer::is_timer_pending() {
                if should_log {
                    early_println!("[aarch64] Local timer pending, calling tick");
                }
                timer::tick(trapframe);
                return;
            }

            // If nothing was pending/claimed, treat as unhandled.
            if should_log {
                early_println!(
                    "[aarch64] exception: EC=0 (irq/unknown) esr={:#x} epc={:#x}",
                    esr,
                    trapframe.epc
                );
            }
        }
        EC_SVC64 => {
            if should_log {
                early_println!(
                    "[aarch64] exception: SVC64 esr={:#x} epc={:#x} nr(x8)={:#x}",
                    esr,
                    trapframe.epc,
                    trapframe.get_syscall_number(),
                );
            }
            match syscall_dispatcher(trapframe) {
                Ok(ret) => {
                    trapframe.set_return_value(ret);
                    trapframe.increment_pc_next(mytask().unwrap());
                }
                Err(msg) => {
                    println!("Syscall error: {}", msg);
                    trapframe.set_return_value(usize::MAX);
                    trapframe.increment_pc_next(mytask().unwrap());
                }
            }
        }
        EC_INSN_ABORT_LOWER_EL | EC_DATA_ABORT_LOWER_EL => {
            let far: usize;
            unsafe { asm!("mrs {0}, far_el1", out(reg) far, options(nostack)); }

            let abort_should_log = ABORT_LOG_BUDGET.fetch_update(
                Ordering::Relaxed,
                Ordering::Relaxed,
                |v| if v == 0 { None } else { Some(v - 1) },
            ).is_ok();

            let wnr = ((esr >> 6) & 0x1) != 0;

            if abort_should_log {
                early_println!(
                    "[aarch64] exception: {} abort esr={:#x} far={:#x} epc={:#x} wnr={} sp={:#x}",
                    if ec == EC_INSN_ABORT_LOWER_EL { "insn" } else { "data" },
                    esr,
                    far,
                    trapframe.epc,
                    wnr as u8,
                    trapframe.regs.reg[31],
                );
            }

            if far == 0 || trapframe.regs.reg[31] == 0 {
                panic!("[aarch64] NULL pointer access or zero stack pointer detected");
            }

            let task = get_scheduler()
                .get_current_task(get_cpu().get_cpuid())
                .expect("No current task for fault handling");
            let manager = &mut task.vm_manager;

            if abort_should_log {
                let asid = manager.get_asid();
                let cpu_ttbr0 = get_cpu().get_ttbr0();
                let cpu_kernel_ttbr0 = get_cpu().get_kernel_ttbr0();

                let dfsc = (esr & 0x3f) as u8;
                early_println!(
                    "[aarch64] fault_ctx: asid={} far={:#x} dfsc={:#x} cpu.ttbr0={:#x} cpu.kernel_ttbr0={:#x}",
                    asid,
                    far,
                    dfsc,
                    cpu_ttbr0,
                    cpu_kernel_ttbr0,
                );

                match manager.search_memory_map(far) {
                    Some(m) => {
                        let paddr = m.get_paddr(far).unwrap_or(0);
                        early_println!(
                            "[aarch64] fault_vma: vm={:#x}-{:#x} pm={:#x}-{:#x} perms={:#x} shared={} paddr={:#x}",
                            m.vmarea.start,
                            m.vmarea.end,
                            m.pmarea.start,
                            m.pmarea.end,
                            m.permissions,
                            m.is_shared as u8,
                            paddr,
                        );
                    }
                    None => {
                        early_println!("[aarch64] fault_vma: NOT FOUND for far={:#x}", far);
                    }
                }

                if let Some(root_pt) = manager.get_root_page_table() {
                    let root_ttbr0 = root_pt.get_val_for_ttbr(asid);
                    early_println!(
                        "[aarch64] fault_root: root_pt={:#x} root_ttbr0={:#x}",
                        root_pt as *mut _ as usize,
                        root_ttbr0,
                    );
                } else {
                    early_println!("[aarch64] fault_root: no root page table (asid={})", asid);
                }
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
                vaddr: far,
                size: None,
            };

            if let Err(e) = manager.lazy_map_page_with(access) {
                early_println!("[aarch64] Fault: ec={:#x}, esr={:#x}, far={:#x}", ec, esr, far);
                early_println!("[aarch64] trapframe: {:#x?}", trapframe);
                panic!("Failed to lazy-map page for fault: {}", e);
            }

            if abort_should_log {
                let asid = manager.get_asid();
                let page_vaddr = far & !(crate::environment::PAGE_SIZE - 1);
                if let Some(root_pt) = manager.get_root_page_table() {
                    let pte_val = root_pt
                        .walk(page_vaddr, false, asid)
                        .map(|pte| pte.entry)
                        .unwrap_or(0);
                    early_println!(
                        "[aarch64] fault_map: vaddr={:#x} pte={:#x}",
                        page_vaddr,
                        pte_val
                    );
                }

                // Hardware probe: does this EL0 address translate with the *user* TTBR0?
                // We only run this for data aborts (not instruction aborts) to avoid
                // any unexpected nesting/edge cases while the faulting instruction is
                // being demand-paged.
                let is_data_abort = matches!(ec, 0x24 | 0x25);
                if is_data_abort {
                    // We temporarily install cpu.ttbr0 into TTBR0_EL1 and use AT S1E0R/W.
                    let user_ttbr0 = get_cpu().get_ttbr0();
                    let mut par_el1: u64 = 0;
                    unsafe {
                        let saved_ttbr0: u64;
                        core::arch::asm!(
                            "mrs {saved_ttbr0}, ttbr0_el1",
                            saved_ttbr0 = out(reg) saved_ttbr0
                        );
                        core::arch::asm!(
                            "msr ttbr0_el1, {user_ttbr0}",
                            "isb",
                            user_ttbr0 = in(reg) user_ttbr0,
                        );
                        if ((esr >> 6) & 0x1) != 0 {
                            core::arch::asm!("at s1e0w, {addr}", addr = in(reg) (far as u64));
                        } else {
                            core::arch::asm!("at s1e0r, {addr}", addr = in(reg) (far as u64));
                        }
                        core::arch::asm!("mrs {par_el1}, par_el1", par_el1 = out(reg) par_el1);
                        core::arch::asm!(
                            "msr ttbr0_el1, {saved_ttbr0}",
                            "isb",
                            saved_ttbr0 = in(reg) saved_ttbr0,
                        );
                    }
                    early_println!(
                        "[aarch64] fault_at: far={:#x} user_ttbr0={:#x} par_el1={:#x}",
                        far,
                        user_ttbr0,
                        par_el1,
                    );
                }
            }
        }
        _ => {
            early_println!("[aarch64] Unhandled exception: ec={:#x}, esr={:#x}", ec, esr);
            early_println!("[aarch64] trapframe: {:#x?}", trapframe);
            panic!("Unhandled AArch64 exception");
        }
    }
}
