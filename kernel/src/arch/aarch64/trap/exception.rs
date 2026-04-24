//! AArch64 exception handler
//!
//! Strategy follows RISC-V implementation: simple cause-based matching
//! with lazy page mapping on page faults.

use core::arch::asm;
use core::panic;

use crate::abi::syscall_dispatcher;
use crate::arch::{Trapframe, get_cpu};
use crate::object::capability::memory_mapping::{AccessKind, AccessOp};
use crate::sched::scheduler::get_scheduler;
use crate::task::mytask;
use crate::{early_println, println};

use super::emulator;

/// Get ESR_EL1 value
fn get_esr_el1() -> u64 {
    let val: u64;
    unsafe { asm!("mrs {}, esr_el1", out(reg) val) };
    val
}

/// Get FAR_EL1 value  
fn get_far_el1() -> u64 {
    let val: u64;
    unsafe { asm!("mrs {}, far_el1", out(reg) val) };
    val
}

/// Get SCTLR_EL1 value
fn get_sctlr_el1() -> u64 {
    let val: u64;
    unsafe { asm!("mrs {}, sctlr_el1", out(reg) val) };
    val
}

/// Exception Class (ESR_EL1[31:26])
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum ExceptionClass {
    Unknown = 0x00,
    TrappedSystemReg = 0x08,
    FpSimdAccess = 0x07,
    SvcAarch64 = 0x15,
    InstructionAbortLowerEl = 0x20,
    InstructionAbortSameEl = 0x21,
    DataAbortLowerEl = 0x24,
    DataAbortSameEl = 0x25,
    WatchpointLowerEl = 0x34,
    SoftwareStep = 0x32,
    Other = 0xFF,
}

impl From<u64> for ExceptionClass {
    fn from(val: u64) -> Self {
        let ec = ((val >> 26) & 0x3f) as u8;
        match ec {
            0x00 => ExceptionClass::Unknown,
            0x07 => ExceptionClass::FpSimdAccess,
            0x08 => ExceptionClass::TrappedSystemReg,
            0x15 => ExceptionClass::SvcAarch64,
            0x20 => ExceptionClass::InstructionAbortLowerEl,
            0x21 => ExceptionClass::InstructionAbortSameEl,
            0x24 => ExceptionClass::DataAbortLowerEl,
            0x25 => ExceptionClass::DataAbortSameEl,
            0x32 => ExceptionClass::SoftwareStep,
            0x34 => ExceptionClass::WatchpointLowerEl,
            _ => ExceptionClass::Other,
        }
    }
}

/// Main exception handler
pub fn arch_exception_handler(trapframe: &mut Trapframe, trap_kind: usize) {
    // Disable single-step while handling exceptions to prevent re-entrancy
    unsafe {
        let mut mdscr: u64;
        core::arch::asm!("mrs {}, mdscr_el1", out(reg) mdscr);
        if mdscr & 1 != 0 {
            mdscr &= !1u64;
            core::arch::asm!("msr mdscr_el1, {}", in(reg) mdscr);
            core::arch::asm!("isb");
        }
    }

    let ec = ExceptionClass::from(trapframe.esr_el1);
    let esr = trapframe.esr_el1;

    match ec {
        ExceptionClass::FpSimdAccess => {
            if crate::arch::user_fpu_enabled() {
                let cpu_id = get_cpu().get_cpuid();
                let task = get_scheduler().get_current_task(cpu_id).unwrap();
                task.vcpu.lock().fpu_used = true;
                crate::arch::fpu::set_user_fpu_enabled(true);
                unsafe {
                    task.vcpu.lock().fpu.restore();
                }
                return;
            }

            print_trap_info(trapframe, esr);
            panic!("FP/SIMD is disabled by build config or DTB");
        }

        ExceptionClass::SvcAarch64 => match syscall_dispatcher(trapframe) {
            Ok(ret) => {
                trapframe.set_return_value(ret);
            }
            Err(msg) => {
                println!("Syscall error: {}", msg);
                trapframe.set_return_value(usize::MAX);
                trapframe.increment_pc_next(mytask().unwrap());
            }
        },

        ExceptionClass::InstructionAbortLowerEl => {
            let vaddr = trapframe.elr as usize;
            handle_instruction_fault(trapframe, vaddr);
        }

        ExceptionClass::DataAbortLowerEl => {
            let far = get_far_el1() as usize;
            let is_write = (esr >> 6) & 1 == 1;
            handle_data_fault(trapframe, far, is_write);
        }

        ExceptionClass::InstructionAbortSameEl => {
            let far = get_far_el1();
            print_trap_info(trapframe, esr);
            crate::println!("Kernel instruction abort at FAR={:#x}", far);
            loop {
                unsafe { asm!("wfi") }
            }
        }

        ExceptionClass::DataAbortSameEl => {
            let far = get_far_el1();
            print_trap_info(trapframe, esr);
            crate::println!("Kernel data abort at FAR={:#x}", far);
            loop {
                unsafe { asm!("wfi") }
            }
        }

        ExceptionClass::WatchpointLowerEl => {
            let far = get_far_el1();
            let wp = (esr >> 5) & 0xF;
            crate::println!(
                "[watchpoint] EL0 write to {:#x} at PC={:#x} WP={} x0={:#x} x1={:#x} x2={:#x} x8={:#x} x19={:#x} x20={:#x} LR={:#x}",
                far,
                trapframe.elr,
                wp,
                trapframe.regs.reg[0],
                trapframe.regs.reg[1],
                trapframe.regs.reg[2],
                trapframe.regs.reg[8],
                trapframe.regs.reg[19],
                trapframe.regs.reg[20],
                trapframe.regs.reg[30],
            );
            disable_user_watchpoint();
        }

        ExceptionClass::SoftwareStep => {
            let lock_kva = unsafe { SINGLESTEP_LOCK_KVA };
            if lock_kva == 0 {
                return;
            }
            let lock_val = unsafe { *(lock_kva as *const u32) };

            static STEP_COUNT: spin::Mutex<usize> = spin::Mutex::new(0);
            let mut sc = STEP_COUNT.lock();
            *sc += 1;

            let step_num = *sc;
            drop(sc);

            if step_num % 1000 == 0 {
                crate::println!(
                    "[step #{}] tick LOCK={:#x} PC={:#x}",
                    step_num,
                    lock_val,
                    trapframe.elr,
                );
            }

            if lock_val != 0x307 && lock_val != 0 {
                crate::println!(
                    "[step #{}] *** LOCK={:#x} PC={:#x} x0={:#x} x1={:#x} x2={:#x} x3={:#x} x19={:#x} x20={:#x} LR={:#x}",
                    step_num,
                    lock_val,
                    trapframe.elr,
                    trapframe.regs.reg[0],
                    trapframe.regs.reg[1],
                    trapframe.regs.reg[2],
                    trapframe.regs.reg[3],
                    trapframe.regs.reg[19],
                    trapframe.regs.reg[20],
                    trapframe.regs.reg[30],
                );
                trapframe.spsr &= !(1 << 21);
                unsafe {
                    let mut mdscr: u64;
                    core::arch::asm!("mrs {}, mdscr_el1", out(reg) mdscr);
                    mdscr &= !1;
                    core::arch::asm!("msr mdscr_el1, {}", in(reg) mdscr);
                    SINGLESTEP_LOCK_KVA = 0;
                }
                return;
            }

            if step_num >= 30000 {
                crate::println!(
                    "[step] safety limit ({}), disabling SS (KVA kept for trap check)",
                    step_num
                );
                trapframe.spsr &= !(1 << 21);
                unsafe {
                    let mut mdscr: u64;
                    core::arch::asm!("mrs {}, mdscr_el1", out(reg) mdscr);
                    mdscr &= !1;
                    core::arch::asm!("msr mdscr_el1, {}", in(reg) mdscr);
                }
                return;
            }

            // Re-enable MDSCR.SS so the next user instruction also steps.
            // It was cleared at exception entry to prevent re-entrancy in
            // other exception handlers; here we restore it for continued tracing.
            unsafe {
                let mut mdscr: u64;
                core::arch::asm!("mrs {}, mdscr_el1", out(reg) mdscr);
                mdscr |= 1;
                core::arch::asm!("msr mdscr_el1, {}", in(reg) mdscr);
            }
        }

        // Unknown (EC=0) or Trapped system register (EC=8):
        // cortex-a57 traps PAC/LSE/RCpc instructions here.
        ExceptionClass::Unknown | ExceptionClass::TrappedSystemReg => {
            if emulator::try_emulate_instruction(trapframe) {
                return;
            }
            print_trap_info(trapframe, esr);
            crate::println!(
                "[trap] unhandled: ESR={:#x} EC={:?} FAR={:#x} ELR={:#x}",
                esr,
                ec,
                get_far_el1(),
                trapframe.elr,
            );
            loop {
                unsafe { asm!("wfi") }
            }
        }

        _ => {
            let ec_raw = ((esr >> 26) & 0x3f) as u8;

            // EC=0 or EC=8 in the catch-all (shouldn't normally happen, but handle defensively)
            if ec_raw == 0x00 || ec_raw == 0x08 {
                if emulator::try_emulate_instruction(trapframe) {
                    return;
                }
            }

            // BRK from userspace (EC=0x30 or EC=0x3C)
            if ec_raw == 0x30 || ec_raw == 0x3c {
                handle_brk(trapframe, esr);
                return;
            }

            print_trap_info(trapframe, esr);
            crate::println!(
                "[trap] unhandled: kind={} ESR={:#x} FAR={:#x} ELR={:#x}",
                trap_kind,
                esr,
                get_far_el1(),
                trapframe.elr,
            );
            loop {
                unsafe { asm!("wfi") }
            }
        }
    }
}

fn handle_brk(trapframe: &mut Trapframe, esr: u64) {
    let imm = (esr & 0xFFFF) as u32;
    let task = mytask().unwrap();

    if imm == 0xb001 {
        let x0 = trapframe.regs.reg[0];
        let x1 = trapframe.regs.reg[1];
        let lr = trapframe.regs.reg[30];
        let tpidr = trapframe.tpidr_el0;
        let tpidrro = trapframe.tpidrro_el0;
        crate::println!("[darwin] dyld halt: x0={:#x} x1={:#x} LR={:#x}", x0, x1, lr);
        crate::println!(
            "[darwin]   x19={:#x} (lock ptr) x2={:#x} (actual_owner)",
            trapframe.regs.reg[19],
            trapframe.regs.reg[2]
        );
        crate::println!(
            "[darwin]   TPIDR_EL0={:#x} TPIDRRO_EL0={:#x}",
            tpidr,
            tpidrro
        );

        // Read expected lock owner from [TPIDRRO_EL0 + 0x18]
        if tpidrro > 0x1000 {
            if let Some(kva) = task.vm_manager.translate_to_kva(tpidrro as usize + 0x18) {
                let owner_val = unsafe { *(kva as *const u32) };
                crate::println!(
                    "[darwin]   [TPIDRRO+0x18]={:#x} (expected lock owner)",
                    owner_val
                );
            }
        }

        // Dump 16 bytes of TLS context for lock owner analysis
        if tpidrro > 0x1000 {
            if let Some(kva) = task.vm_manager.translate_to_kva(tpidrro as usize) {
                let tls_bytes = unsafe { core::slice::from_raw_parts(kva as *const u8, 64) };
                crate::print!("[darwin]   TLS[0..64]=");
                for (i, b) in tls_bytes.iter().enumerate() {
                    if i % 8 == 0 {
                        crate::print!(" ");
                    }
                    crate::print!("{:02x}", b);
                }
                crate::println!("");
            }
        }

        if x1 > 0x1000 {
            if let Some(kva) = task.vm_manager.translate_to_kva(x1) {
                let bytes = unsafe { core::slice::from_raw_parts(kva as *const u8, 256) };
                crate::print!("[darwin] halt msg=\"");
                for &b in bytes.iter() {
                    if b >= 0x20 && b < 0x7f {
                        crate::print!("{}", b as char);
                    } else {
                        break;
                    }
                }
                crate::println!("\"");
            } else {
                crate::println!("[darwin] halt msg: could not translate x1={:#x}", x1);
            }
        }

        if x0 > 0x1000 && x0 != x1 {
            if let Some(kva) = task.vm_manager.translate_to_kva(x0) {
                let bytes = unsafe { core::slice::from_raw_parts(kva as *const u8, 256) };
                crate::print!("[darwin] halt x0-str=\"");
                for &b in bytes.iter() {
                    if b >= 0x20 && b < 0x7f {
                        crate::print!("{}", b as char);
                    } else {
                        break;
                    }
                }
                crate::println!("\"");
            }
        }

        trapframe.increment_pc_next(task);
        task.exit(128 + 6);
        return;
    }

    let x0 = trapframe.regs.reg[0];
    let x1 = trapframe.regs.reg[1];
    let lr = trapframe.regs.reg[30];
    let x8 = trapframe.regs.reg[8];
    crate::println!("[darwin] dyld BRK #{:#x} at ELR={:#x}", imm, trapframe.elr);
    crate::println!(
        "[darwin]   x0={:#x} x1={:#x} LR={:#x} x8={:#x}",
        x0,
        x1,
        lr,
        x8
    );

    let task = mytask().unwrap();

    crate::println!(
        "[darwin]   TPIDRRO_EL0={:#x} tpidr_el0={:#x}",
        trapframe.tpidrro_el0,
        trapframe.tpidr_el0
    );
    let tpidrro = trapframe.tpidrro_el0 as usize;
    if tpidrro > 0x1000 {
        if let Some(kva) = task.vm_manager.translate_to_kva(tpidrro) {
            let slot0 = unsafe { core::ptr::read(kva as *const u64) };
            let slot3 = unsafe { core::ptr::read((kva as *const u64).add(3)) };
            crate::println!("[darwin]   TLS[0]={:#x} TLS[3]={:#x}", slot0, slot3);
        }
    }

    let str_ptr = x1;
    if str_ptr > 0x1000 {
        if let Some(kva) = task.vm_manager.translate_to_kva(str_ptr) {
            let bytes = unsafe { core::slice::from_raw_parts(kva as *const u8, 200) };
            crate::print!("[darwin]   msg=\"");
            for &b in bytes.iter() {
                if b >= 0x20 && b < 0x7f {
                    crate::print!("{}", b as char);
                } else {
                    break;
                }
            }
            crate::println!("\"");
        }
    }

    trapframe.increment_pc_next(task);
}

/// Handle instruction page fault (like RISC-V cause 12)
fn handle_instruction_fault(trapframe: &mut Trapframe, vaddr: usize) {
    let task = get_scheduler()
        .get_current_task(get_cpu().get_cpuid())
        .unwrap();

    let access = AccessKind {
        op: AccessOp::Instruction,
        vaddr,
        size: None,
    };

    match task.vm_manager.lazy_map_page_with(access) {
        Ok(_) => (),
        Err(_) => {
            print_trap_info(trapframe, get_esr_el1());
            panic!(
                "Failed to map page for instruction fault at vaddr: {:#x}",
                vaddr
            );
        }
    }
}

/// Handle data page fault (like RISC-V cause 13/15)
fn handle_data_fault(trapframe: &mut Trapframe, vaddr: usize, is_write: bool) {
    let esr = get_esr_el1();
    let dfsc = esr & 0x3f;
    if is_write && (dfsc >= 0x0d && dfsc <= 0x0f) {
        if handle_lock_watch_fault(trapframe, vaddr) {
            return;
        }
    }

    let task = get_scheduler()
        .get_current_task(get_cpu().get_cpuid())
        .unwrap();

    let op = if is_write {
        AccessOp::Store
    } else {
        AccessOp::Load
    };

    let access = AccessKind {
        op,
        vaddr,
        size: None,
    };

    let esr = get_esr_el1();
    let dfsc = esr & 0x3f;

    if dfsc >= 0x0d && dfsc <= 0x0f {
        early_println!(
            "[PF] PERMISSION FAULT: vaddr={:#x} write={} PC={:#x} DFSC={:#x}",
            vaddr,
            is_write,
            trapframe.get_current_pc(),
            dfsc
        );
        if let Some(map) = task.vm_manager.search_memory_map(vaddr) {
            early_println!(
                "[PF] vmarea=[{:#x}..{:#x}] perms={:#x} (R={} W={} X={} U={})",
                map.vmarea.start,
                map.vmarea.end,
                map.permissions,
                map.permissions & 0x1 != 0,
                map.permissions & 0x2 != 0,
                map.permissions & 0x4 != 0,
                map.permissions & 0x8 != 0,
            );
        } else {
            early_println!("[PF] No mapping for vaddr={:#x}", vaddr);
        }
    }

    match task.vm_manager.lazy_map_page_with(access) {
        Ok(_) => (),
        Err(e) => {
            print_trap_info(trapframe, get_esr_el1());
            if let Some(task) = get_scheduler().get_current_task(get_cpu().get_cpuid()) {
                early_println!(
                    "Task {} (PID {}) data fault at vaddr: {:#x} (write={}) from PC: {:#x}",
                    task.name.read(),
                    task.get_id(),
                    vaddr,
                    is_write,
                    trapframe.get_current_pc()
                );
            }
            panic!(
                "Failed to map page for data fault at vaddr: {:#x} (write={}) from PC: {:#x}",
                vaddr,
                is_write,
                trapframe.get_current_pc()
            );
        }
    }
}

/// Print trap information for debugging
fn print_trap_info(trapframe: &Trapframe, esr: u64) {
    let far = get_far_el1();
    let ec = (esr >> 26) & 0x3f;
    let fsc = esr & 0x3f;

    crate::println!("=== Trap Info ===");
    crate::println!("ESR_EL1: {:#018x} (EC={:#x}, FSC={:#x})", esr, ec, fsc);
    crate::println!("FAR_EL1: {:#018x}", far);
    crate::println!("ELR_EL1: {:#018x}", trapframe.elr);
    crate::println!(
        "x0={:#x} x1={:#x} x2={:#x} x3={:#x}",
        trapframe.regs.reg[0],
        trapframe.regs.reg[1],
        trapframe.regs.reg[2],
        trapframe.regs.reg[3]
    );
    crate::println!(
        "x4={:#x} x5={:#x} x6={:#x} x7={:#x}",
        trapframe.regs.reg[4],
        trapframe.regs.reg[5],
        trapframe.regs.reg[6],
        trapframe.regs.reg[7]
    );
    crate::println!("TPIDR_EL0={:#x}", trapframe.tpidr_el0);
    crate::println!(
        "x8={:#x} x9={:#x}",
        trapframe.regs.reg[8],
        trapframe.regs.reg[9]
    );
}

/// Set a hardware write watchpoint on a 4-byte user-space address.
/// EC=0x34 exception fires when EL0 writes to `addr`.
pub fn enable_user_write_watchpoint(addr: u64) {
    unsafe {
        let dfr0: u64;
        asm!("mrs {}, id_aa64dfr0_el1", out(reg) dfr0);
        let wrps = (dfr0 >> 20) & 0xF;
        let brps = (dfr0 >> 12) & 0xF;
        println!(
            "[watchpoint] ID_AA64DFR0={:#x} WRPs={} BRPs={}",
            dfr0, wrps, brps
        );

        asm!("msr oslar_el1, xzr");
        asm!("msr dbgwcr0_el1, xzr");
        asm!("isb");
        asm!("msr dbgwvr0_el1, {}", in(reg) addr);
        // E=1, PAC=00 (any EL), LSC=11 (any access), BAS=00001111 (4 bytes at bits[12:5])
        let wcr: u64 = (0xFu64 << 5) | (0b11 << 3) | (0b00 << 1) | 1; // = 0x1F9
        asm!("msr dbgwcr0_el1, {}", in(reg) wcr);
        let mut mdscr: u64;
        asm!("mrs {}, mdscr_el1", out(reg) mdscr);
        mdscr |= 1 << 15; // MDE=1
        mdscr |= 1 << 13; // KDE=1
        asm!("msr mdscr_el1, {}", in(reg) mdscr);
        asm!("isb");

        let verify_wvr: u64;
        let verify_wcr: u64;
        let verify_mdscr: u64;
        asm!("mrs {}, dbgwvr0_el1", out(reg) verify_wvr);
        asm!("mrs {}, dbgwcr0_el1", out(reg) verify_wcr);
        asm!("mrs {}, mdscr_el1", out(reg) verify_mdscr);
        println!(
            "[watchpoint] set addr={:#x} WVR={:#x} WCR={:#x} MDSCR={:#x}",
            addr, verify_wvr, verify_wcr, verify_mdscr
        );
    }
}

/// Disable the user watchpoint (clear DBGWCR0_EL1).
pub fn disable_user_watchpoint() {
    unsafe {
        asm!("msr dbgwcr0_el1, xzr");
        asm!("isb");
    }
}

/// Cached kernel virtual address of the lock being traced.
/// Non-zero means tracing is active. Checked in arch_user_trap_handler
/// on every exception/IRQ entry to catch lock corruption.
static mut SINGLESTEP_LOCK_KVA: usize = 0;

/// Check the traced lock value on every trap entry. Called from
/// arch_user_trap_handler BEFORE the specific handler dispatch.
pub fn check_traced_lock(trapframe: &Trapframe, trap_kind: usize) {
    let kva = unsafe { SINGLESTEP_LOCK_KVA };
    if kva == 0 {
        return;
    }
    let lock_val = unsafe { *(kva as *const u32) };
    if lock_val == 0x307 || lock_val == 0 {
        return;
    }

    let lock_user_va = trapframe.regs.reg[19];
    let current_kva =
        crate::task::mytask().and_then(|t| t.vm_manager.translate_to_kva(lock_user_va));
    let current_phys =
        crate::task::mytask().and_then(|t| t.vm_manager.translate_to_phys(lock_user_va));
    let (cached_val, current_val) = {
        let cv = unsafe { *(kva as *const u32) };
        let curv = current_kva.map(|ck| unsafe { *(ck as *const u32) });
        (cv, curv)
    };

    crate::println!(
        "[lock_trace] LOCK={:#x} kind={} PC={:#x} x0={:#x} x2={:#x} x19={:#x} LR={:#x}",
        lock_val,
        trap_kind,
        trapframe.elr,
        trapframe.regs.reg[0],
        trapframe.regs.reg[2],
        trapframe.regs.reg[19],
        trapframe.regs.reg[30],
    );
    crate::println!(
        "[lock_trace] cached_kva={:#x} current_kva={:?} current_phys={:?}",
        kva,
        current_kva,
        current_phys,
    );
    crate::println!(
        "[lock_trace] cached_read={:#x} current_read={:?} same_page={}",
        cached_val,
        current_val,
        current_kva == Some(kva),
    );
    unsafe {
        SINGLESTEP_LOCK_KVA = 0;
    }
}

static mut LOCK_WATCH_PAGE_VA: usize = 0;

pub fn enable_lock_page_watch(lock_user_va: usize) {
    let task = crate::task::mytask().unwrap();
    let page_va = lock_user_va & !0xfff;
    let kva = match task.vm_manager.translate_to_kva(lock_user_va) {
        Some(k) => k,
        None => return,
    };
    unsafe {
        if SINGLESTEP_LOCK_KVA != 0 {
            return;
        }
        SINGLESTEP_LOCK_KVA = kva;
    }
    let root_pt = match task.vm_manager.get_root_page_table() {
        Some(pt) => pt,
        None => return,
    };
    let pte = match root_pt.walk(page_va, false, task.vm_manager.get_asid()) {
        Some(pte) => pte,
        None => return,
    };
    if !pte.is_valid() {
        return;
    }
    let old = pte.entry;
    pte.entry = old & !(1u64 << 6);
    crate::arch::aarch64::clean_dcache_to_poc_range(
        &pte.entry as *const u64 as usize,
        core::mem::size_of::<u64>(),
    );
    unsafe {
        core::arch::asm!("tlbi vmalle1is", "dsb ish", "isb");
    }
    unsafe {
        LOCK_WATCH_PAGE_VA = page_va;
    }
    crate::println!(
        "[lock_watch] PTE {:#x} → RO (was {:#x}, now {:#x}) kva={:#x}",
        page_va,
        old,
        pte.entry,
        kva
    );
}

fn handle_lock_watch_fault(trapframe: &mut Trapframe, far: usize) -> bool {
    let page_va = unsafe { LOCK_WATCH_PAGE_VA };
    if page_va == 0 {
        return false;
    }
    if (far & !0xfff) != page_va {
        return false;
    }

    let lock_offset = 0x150; // lock is at page_base + 0x150
    let is_lock = (far & 0xfff) == lock_offset;

    crate::println!(
        "[lock_watch] WRITE FAULT: FAR={:#x} PC={:#x} lock_write={} x0={:#x} x2={:#x} x19={:#x}",
        far,
        trapframe.elr,
        is_lock,
        trapframe.regs.reg[0],
        trapframe.regs.reg[2],
        trapframe.regs.reg[19],
    );

    let task = crate::task::mytask().unwrap();
    let root_pt = match task.vm_manager.get_root_page_table() {
        Some(pt) => pt,
        None => return false,
    };
    let pte = match root_pt.walk(page_va, false, task.vm_manager.get_asid()) {
        Some(pte) => pte,
        None => return false,
    };
    let before = pte.entry;
    pte.entry |= 1u64 << 6;
    let after = pte.entry;
    crate::arch::aarch64::clean_dcache_to_poc_range(
        &pte.entry as *const u64 as usize,
        core::mem::size_of::<u64>(),
    );
    unsafe {
        core::arch::asm!("tlbi vmalle1is", "dsb ish", "isb");
    }
    crate::println!(
        "[lock_watch] restore RW: PTE before={:#x} after={:#x} ptr={:#x}",
        before,
        after,
        &pte.entry as *const u64 as usize
    );

    if is_lock {
        crate::println!(
            "[lock_watch] *** SMOKING GUN: write to lock from PC={:#x} ***",
            trapframe.elr
        );
        unsafe {
            LOCK_WATCH_PAGE_VA = 0;
        }
    }
    true
}

/// Enable software single-step and cache the lock's KVA for cheap per-step checking.
/// After each user instruction, the SoftwareStep handler reads the lock via the
/// cached pointer (no task lookup / page-table walk) and logs when the value changes.
pub fn enable_singlestep_lock_trace(lock_user_va: usize) {
    let task = crate::task::mytask().unwrap();
    let kva = match task.vm_manager.translate_to_kva(lock_user_va) {
        Some(k) => k,
        None => return,
    };
    unsafe {
        if SINGLESTEP_LOCK_KVA != 0 {
            return;
        } // already tracing
        SINGLESTEP_LOCK_KVA = kva;
    }
    crate::println!("[sstep] enabled, lock_kva={:#x}", kva);
}
