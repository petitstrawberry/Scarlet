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

fn decode_aarch64_leaf_pte(entry: u64) -> (u8, u8, u8, u8, u8, u8, u8, u8, usize) {
    // Best-effort decoder for stage-1 leaf page descriptors (4KB granule).
    // Returns: (valid, desc_type_bit1, attr, ap, sh, af, pxn, uxn, out_pa)
    let valid = (entry & 0x1) as u8;
    let desc_type_bit1 = ((entry >> 1) & 0x1) as u8;
    let attr = ((entry >> 2) & 0x7) as u8; // AttrIndx[2:0]
    let ap = ((entry >> 6) & 0x3) as u8; // AP[2:1]
    let sh = ((entry >> 8) & 0x3) as u8; // SH[1:0]
    let af = ((entry >> 10) & 0x1) as u8; // AF
    let pxn = ((entry >> 53) & 0x1) as u8;
    let uxn = ((entry >> 54) & 0x1) as u8;
    let out_pa = (((entry >> 12) & 0xffff_fffff) as usize) << 12;
    (valid, desc_type_bit1, attr, ap, sh, af, pxn, uxn, out_pa)
}

pub fn arch_exception_handler(trapframe: &mut Trapframe) {
    let esr: u64;
    unsafe { asm!("mrs {0}, esr_el1", out(reg) esr, options(nostack)); }

    let ec = (esr >> 26) & 0x3f;

    let should_log = EXCEPTION_LOG_BUDGET.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
        if v == 0 { None } else { Some(v - 1) }
    }).is_ok();

    // if should_log {
    //     crate::early_println!("[aarch64] Exception handler invoked: EC={:#x}", ec);
    // }

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

            // For instruction aborts, FAR_EL1 is expected to match ELR_EL1, but during bring-up
            // we occasionally observe mismatches in logs. Use EPC (ELR_EL1) as the authoritative
            // fault VA for instruction abort handling.
            let fault_addr: usize = if ec == EC_INSN_ABORT_LOWER_EL {
                trapframe.epc as usize
            } else {
                far
            };

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

            if fault_addr == 0 || trapframe.regs.reg[31] == 0 {
                panic!("[aarch64] NULL pointer access or zero stack pointer detected");
            }

            let task = get_scheduler()
                .get_current_task(get_cpu().get_cpuid())
                .expect("No current task for fault handling");
            // Keep borrows of task.vm_manager short so we can still call task methods below.
            let maybe_autogrow: Option<(usize, usize)> = {
                let manager = &task.vm_manager;
                let mut out: Option<(usize, usize)> = None;

                if abort_should_log {
                    let asid = manager.get_asid();
                    let cpu_ttbr0 = get_cpu().get_ttbr0();
                    let cpu_kernel_ttbr0 = get_cpu().get_kernel_ttbr0();
                    let cpu_ttbr1_pre = get_cpu().get_scratch();

                    let ttbr0_el1: u64;
                    let ttbr1_el1: u64;
                    unsafe {
                        asm!("mrs {0}, ttbr0_el1", out(reg) ttbr0_el1, options(nostack));
                        asm!("mrs {0}, ttbr1_el1", out(reg) ttbr1_el1, options(nostack));
                    }

                    // ESR_EL1 layout: [31:26]=EC, [24:0]=ISS.
                    // For instruction/data abort, ISS contains IFSC/DFSC in bits [5:0]
                    // and has a useful S1PTW bit indicating fault during a stage-1 table walk.
                    let iss = (esr & 0x01ff_ffff) as u32;
                    let fsc = (iss & 0x3f) as u8;
                    let s1ptw = ((iss >> 7) & 0x1) as u8;
                    early_println!(
                        "[aarch64] fault_ctx: asid={} far={:#x} iss={:#x} fsc={:#x} s1ptw={} ttbr0_pre={:#x} ttbr1_pre={:#x} kernel_ttbr0={:#x} ttbr0_after_swap={:#x} ttbr1_after_swap={:#x}",
                        asid,
                        far,
                        iss,
                        fsc,
                        s1ptw,
                        cpu_ttbr0,
                        cpu_ttbr1_pre,
                        cpu_kernel_ttbr0,
                        ttbr0_el1,
                        ttbr1_el1,
                    );

                    // Additional signal: is the leaf PTE already present BEFORE we attempt lazy mapping?
                    let page_vaddr = fault_addr & !(crate::environment::PAGE_SIZE - 1);
                    if let Some(root_pt) = manager.get_root_page_table() {
                        let pte_before = root_pt
                            .walk(page_vaddr, false, asid)
                            .map(|pte| pte.entry)
                            .unwrap_or(0);
                        early_println!(
                            "[aarch64] fault_pte_before: vaddr={:#x} pte={:#x}",
                            page_vaddr,
                            pte_before
                        );

                        if pte_before != 0 {
                            let (valid, typ1, attr, ap, sh, af, pxn, uxn, out_pa) =
                                decode_aarch64_leaf_pte(pte_before);
                            early_println!(
                                "[aarch64] fault_pte_fields: valid={} typ1={} attr={} ap={:#b} sh={:#b} af={} pxn={} uxn={} out_pa={:#x}",
                                valid,
                                typ1,
                                attr,
                                ap,
                                sh,
                                af,
                                pxn,
                                uxn,
                                out_pa,
                            );
                        }

                        // Dump the raw page-table walk entries for additional bring-up diagnostics.
                        // This helps validate that intermediate table descriptors match what the HW walker expects.
                        let v = page_vaddr;
                        let i0 = (v >> 39) & 0x1ff;
                        let i1 = (v >> 30) & 0x1ff;
                        let i2 = (v >> 21) & 0x1ff;
                        let i3 = (v >> 12) & 0x1ff;
                        unsafe {
                            let mut pt: *const crate::arch::aarch64::vm::mmu::armv8_4k::PageTable =
                                root_pt as *const _;
                            let mut e0: u64 = 0;
                            let mut e1: u64 = 0;
                            let mut e2: u64 = 0;
                            let mut e3: u64 = 0;
                            let mut ok_mask: u8 = 0;

                            if !pt.is_null() {
                                // Level 0
                                e0 = (*pt).entries[i0].entry;
                                ok_mask |= 1;
                                if (e0 & 1) != 0 && (e0 & 0x3) == 0x3 {
                                    let p1 = ((e0 >> 12) & 0xfffffffff) as usize;
                                    pt = (p1 << 12) as *const _;
                                    if !pt.is_null() {
                                        // Level 1
                                        e1 = (*pt).entries[i1].entry;
                                        ok_mask |= 2;
                                        if (e1 & 1) != 0 && (e1 & 0x3) == 0x3 {
                                            let p2 = ((e1 >> 12) & 0xfffffffff) as usize;
                                            pt = (p2 << 12) as *const _;
                                            if !pt.is_null() {
                                                // Level 2
                                                e2 = (*pt).entries[i2].entry;
                                                ok_mask |= 4;
                                                if (e2 & 1) != 0 && (e2 & 0x3) == 0x3 {
                                                    let p3 = ((e2 >> 12) & 0xfffffffff) as usize;
                                                    pt = (p3 << 12) as *const _;
                                                    if !pt.is_null() {
                                                        // Level 3 (leaf)
                                                        e3 = (*pt).entries[i3].entry;
                                                        ok_mask |= 8;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            early_println!(
                                "[aarch64] ptwalk: vaddr={:#x} idx=[{:#x},{:#x},{:#x},{:#x}] e0={:#x} e1={:#x} e2={:#x} e3={:#x} ok={:#x}",
                                v,
                                i0,
                                i1,
                                i2,
                                i3,
                                e0,
                                e1,
                                e2,
                                e3,
                                ok_mask
                            );
                        }
                    }

                    match manager.search_memory_map(fault_addr) {
                        Some(m) => {
                            let paddr = m.get_paddr(fault_addr).unwrap_or(0);
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
                            early_println!("[aarch64] fault_vma: NOT FOUND for fault_addr={:#x} (far={:#x} epc={:#x})", fault_addr, far, trapframe.epc);
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

                // If the VMA is missing, try a minimal, conservative auto-grow for a user data access.
                // This is primarily to keep user bring-up moving in early stages when brk/heap
                // management is incomplete.
                if manager.search_memory_map(fault_addr).is_none() && ec == EC_DATA_ABORT_LOWER_EL {
                    let page_vaddr = fault_addr & !(crate::environment::PAGE_SIZE - 1);
                    let prev_last = page_vaddr.wrapping_sub(1);
                    if prev_last < page_vaddr {
                        if let Some(prev_map) = manager.search_memory_map(prev_last) {
                            if prev_map.vmarea.end == prev_last {
                                let is_store = wnr;
                                let has_write = (prev_map.permissions & 0x2) != 0;
                                if !is_store || has_write {
                                    out = Some((page_vaddr, prev_map.permissions));
                                }
                            }
                        }
                    }
                }
                out
            };

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

            if let Some((page_vaddr, perms)) = maybe_autogrow {
                let _ = task.allocate_pages(page_vaddr, 1, perms);
            }

            let manager = &task.vm_manager;
            if let Err(e) = manager.lazy_map_page_with(access) {
                early_println!(
                    "[aarch64] Fault: ec={:#x}, esr={:#x}, fault_addr={:#x} (far={:#x} epc={:#x})",
                    ec,
                    esr,
                    fault_addr,
                    far,
                    trapframe.epc
                );
                early_println!("[aarch64] trapframe: {:#x?}", trapframe);
                panic!("Failed to lazy-map page for fault: {}", e);
            }

            if abort_should_log {
                let asid = task.vm_manager.get_asid();
                let page_vaddr = fault_addr & !(crate::environment::PAGE_SIZE - 1);
                if let Some(root_pt) = task.vm_manager.get_root_page_table() {
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

                // NOTE: Hardware translation probe via AT/PAR_EL1 is temporarily disabled.
                // It can hang QEMU during early bring-up, and we can diagnose TTBR/PTE issues
                // sufficiently via the logs above.
            }
        }
        _ => {
            early_println!("[aarch64] Unhandled exception: ec={:#x}, esr={:#x}", ec, esr);
            early_println!("[aarch64] trapframe: {:#x?}", trapframe);
            panic!("Unhandled AArch64 exception");
        }
    }
}
