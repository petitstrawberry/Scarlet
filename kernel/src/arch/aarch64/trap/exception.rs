//! AArch64 exception handler
//!
//! Strategy follows RISC-V implementation: simple cause-based matching
//! with lazy page mapping on page faults.

use core::arch::asm;
use core::panic;

use crate::abi::syscall_dispatcher;
use crate::arch::{Trapframe, get_cpu};
use crate::object::capability::memory_mapping::{AccessKind, AccessOp};
use crate::println;
use crate::sched::scheduler::{current_task, schedule};
use crate::task::mytask;

const USER_PAGE_FAULT_EXIT_STATUS: i32 = 139;
const USER_ILLEGAL_INSTRUCTION_EXIT_STATUS: i32 = 132;

/// Get CurrentEL value
fn get_current_el() -> u64 {
    let val: u64;
    unsafe { asm!("mrs {}, CurrentEL", out(reg) val) };
    val
}

fn current_el_number() -> u64 {
    // CurrentEL encodes the exception level in bits [3:2] as EL<<2.
    // Return the EL number (0-3).
    (get_current_el() >> 2) & 0x3
}

fn get_hcr_el2() -> u64 {
    if current_el_number() != 2 {
        return 0;
    }

    let val: u64;
    unsafe { asm!("mrs {}, hcr_el2", out(reg) val) };
    val
}

fn trap_from_user(trapframe: &Trapframe) -> bool {
    // SPSR_EL1.M[3:0] records the previous exception level/mode.
    // EL0t is encoded as 0b0000.
    trapframe.spsr & 0xf == 0
}

/// Get DAIF value
fn get_daif() -> u64 {
    let val: u64;
    unsafe { asm!("mrs {}, daif", out(reg) val) };
    val
}

/// Get ESR_EL1 value
fn get_esr_el1() -> u64 {
    let val: u64;
    unsafe { asm!("mrs {}, esr_el1", out(reg) val) };
    val
}

/// Exception Class (ESR_EL1[31:26])
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum ExceptionClass {
    Unknown = 0x00,
    /// Trapped FP/SIMD access (typically because CPACR_EL1.FPEN traps EL0).
    FpSimdAccess = 0x07,
    SvcAarch64 = 0x15,
    /// Trapped SVE access (typically because CPACR_EL1.ZEN traps EL0).
    SveAccess = 0x19,
    InstructionAbortLowerEl = 0x20,
    InstructionAbortSameEl = 0x21,
    DataAbortLowerEl = 0x24,
    DataAbortSameEl = 0x25,
    Other = 0xFF,
}

impl From<u64> for ExceptionClass {
    fn from(val: u64) -> Self {
        let ec = ((val >> 26) & 0x3f) as u8;
        match ec {
            0x00 => ExceptionClass::Unknown,
            0x07 => ExceptionClass::FpSimdAccess,
            0x15 => ExceptionClass::SvcAarch64,
            0x19 => ExceptionClass::SveAccess,
            0x20 => ExceptionClass::InstructionAbortLowerEl,
            0x21 => ExceptionClass::InstructionAbortSameEl,
            0x24 => ExceptionClass::DataAbortLowerEl,
            0x25 => ExceptionClass::DataAbortSameEl,
            _ => ExceptionClass::Other,
        }
    }
}

/// Main exception handler
pub fn arch_exception_handler(trapframe: &mut Trapframe, trap_kind: usize) {
    let ec = ExceptionClass::from(trapframe.esr_el1);
    let esr = trapframe.esr_el1;

    let ec_byte = ((esr >> 26) & 0x3f) as u64;
    crate::breadcrumb::drop(0x4500 | ec_byte, trap_kind as u64, esr);

    // Decode useful fields for Data/Instruction aborts.
    // ISS layout differs by EC, but WnR(bit 6) and DFSC/IFSC(bits 5:0) are consistent
    // for abort classes.
    let iss = esr & 0x01ff_ffff;
    let _fsc = (iss & 0x3f) as u8;
    let _wnr = ((iss >> 6) & 0x1) as u8;

    let kind_str = match trap_kind {
        0 => "Sync",
        1 => "IRQ",
        2 => "FIQ",
        3 => "SError",
        _ => "UnknownKind",
    };

    if trap_kind == 3 {
        print_trap_info(trapframe, esr);
        crate::println!(
            "[trap] asynchronous SError: ESR={:#x} ELR={:#x} CurrentEL=EL{} SPSR={:#x} DAIF={:#x} HCR_EL2={:#x}",
            esr,
            trapframe.elr,
            current_el_number(),
            trapframe.spsr,
            get_daif(),
            get_hcr_el2(),
        );
        crate::println!(
            "[trap] FAR_EL1 and the interrupted PC are not attributed to an asynchronous SError"
        );
        loop {
            // SAFETY: This CPU cannot safely resume after an uncorrected SError.
            unsafe { asm!("wfi") }
        }
    }

    match ec {
        // User tried to execute FP/SIMD while EL0 access is trapped.
        // Enable access for this task and restore its context, then retry.
        ExceptionClass::FpSimdAccess => {
            crate::breadcrumb::drop(
                crate::breadcrumb::FP_TRAP_ENTER,
                trapframe.elr,
                trapframe.spsr,
            );
            if !trap_from_user(trapframe) {
                crate::breadcrumb::drop(crate::breadcrumb::KFAULT, trapframe.elr, esr);
                print_trap_info(trapframe, esr);
                crate::println!(
                    "[trap] FP/SIMD access trapped in privileged context at ELR={:#x}",
                    trapframe.elr
                );
                loop {
                    unsafe { asm!("wfi") }
                }
            }

            if crate::arch::user_fpu_enabled() {
                let cpu_id = get_cpu().get_cpuid();
                crate::breadcrumb::drop(crate::breadcrumb::FP_TASK_LOOKUP, cpu_id as u64, 0);
                let task = current_task(cpu_id).unwrap();
                let task_id = task.get_id() as u64;
                crate::breadcrumb::drop(crate::breadcrumb::FP_TASK_FOUND, task_id, 0);

                let mut vcpu = task.vcpu.lock();
                vcpu.fpu_used = true;
                crate::breadcrumb::drop(crate::breadcrumb::FP_VCPU_LOCKED, task_id, 0);

                crate::arch::fpu::set_user_fpu_enabled(true);
                crate::breadcrumb::drop(
                    crate::breadcrumb::FP_ACCESS_ENABLED,
                    task_id,
                    crate::arch::fpu::is_fpu_enabled() as u64,
                );
                crate::breadcrumb::drop(crate::breadcrumb::FP_RESTORE_BEGIN, task_id, 0);
                unsafe {
                    vcpu.fpu.restore_control();
                }
                crate::breadcrumb::drop(crate::breadcrumb::FP_CONTROL_DONE, task_id, 0);
                crate::breadcrumb::drop(crate::breadcrumb::FP_VECTOR_BEGIN, task_id, 0);
                unsafe {
                    vcpu.fpu.restore_vectors();
                }
                crate::breadcrumb::drop(crate::breadcrumb::FP_VECTOR_DONE, task_id, 0);
                crate::breadcrumb::drop(crate::breadcrumb::FP_RESTORE_DONE, task_id, 0);
                return;
            }

            print_trap_info(trapframe, esr);
            panic!("FP/SIMD is disabled by build config or DTB");
        }

        ExceptionClass::SveAccess => {
            if !handle_sve_access_trap(trapframe) {
                let instr_at_elr = read_user_instruction(trapframe.elr as usize);
                let instr_before_elr =
                    read_user_instruction((trapframe.elr as usize).wrapping_sub(4));
                if trap_from_user(trapframe) {
                    crate::println!(
                        "[trap] unsupported SVE access trap at ELR={:#x}; instr@ELR={:?} instr@ELR-4={:?}; SVE context is not implemented",
                        trapframe.elr,
                        instr_at_elr,
                        instr_before_elr,
                    );
                    terminate_current_user_exception(
                        trapframe,
                        esr,
                        "unsupported SVE instruction",
                        trapframe.elr as usize,
                        false,
                        trapframe.elr,
                        "SVE context is not implemented",
                        USER_ILLEGAL_INSTRUCTION_EXIT_STATUS,
                    );
                    return;
                }

                print_trap_info(trapframe, esr);
                crate::println!(
                    "[trap] unsupported SVE access trap at ELR={:#x}; instr@ELR={:?} instr@ELR-4={:?}; SVE context is not implemented",
                    trapframe.elr,
                    instr_at_elr,
                    instr_before_elr,
                );
                loop {
                    unsafe { asm!("wfi") }
                }
            }
        }

        // SVC from AArch64 user mode (syscall)
        ExceptionClass::SvcAarch64 => match syscall_dispatcher(trapframe) {
            Ok(ret) => {
                trapframe.set_return_value(ret);
                crate::sched::scheduler::process_pending_events_before_user_return(trapframe);
            }
            Err(msg) => {
                println!("Syscall error: {}", msg);
                trapframe.set_return_value(usize::MAX);
                trapframe.increment_pc_next(&mytask().unwrap());
                crate::sched::scheduler::process_pending_events_before_user_return(trapframe);
            }
        },

        // Instruction abort from lower EL
        ExceptionClass::InstructionAbortLowerEl => {
            let vaddr = trapframe.elr as usize;
            handle_instruction_fault(trapframe, vaddr);
        }

        // Data abort from lower EL
        ExceptionClass::DataAbortLowerEl => {
            let far = trapframe.far_el1 as usize;
            let is_write = (esr >> 6) & 1 == 1; // WnR bit
            handle_data_fault(trapframe, far, is_write);
        }

        // Instruction abort from same EL (kernel bug)
        ExceptionClass::InstructionAbortSameEl => {
            crate::breadcrumb::drop(crate::breadcrumb::KFAULT, trapframe.far_el1, esr);
            let far = trapframe.far_el1;
            print_trap_info(trapframe, esr);
            crate::println!("Kernel instruction abort at FAR={:#x}", far);
            loop {
                unsafe { asm!("wfi") }
            }
        }

        // Data abort from same EL (kernel bug)
        ExceptionClass::DataAbortSameEl => {
            crate::breadcrumb::drop(crate::breadcrumb::KFAULT, trapframe.far_el1, esr);
            let far = trapframe.far_el1;
            print_trap_info(trapframe, esr);
            crate::println!("Kernel data abort at FAR={:#x}", far);
            loop {
                unsafe { asm!("wfi") }
            }
        }

        // Unknown or unhandled exception. User-origin exceptions are reported
        // and terminate the current task; kernel-origin exceptions stop here
        // because continuing would hide a real bring-up bug.
        _ => {
            if trap_from_user(trapframe) {
                terminate_current_user_exception(
                    trapframe,
                    esr,
                    "unhandled user exception",
                    trapframe.elr as usize,
                    false,
                    trapframe.elr,
                    "Unhandled lower-EL exception",
                    USER_ILLEGAL_INSTRUCTION_EXIT_STATUS,
                );
                return;
            }

            print_trap_info(trapframe, esr);

            crate::breadcrumb::drop(crate::breadcrumb::KFAULT, trapframe.far_el1, esr);
            crate::println!(
                "[trap] unhandled exception: kind={}({}) ESR={:#x} FAR={:#x} ELR={:#x} CurrentEL=EL{} SPSR={:#x} DAIF={:#x} HCR_EL2={:#x}",
                trap_kind,
                kind_str,
                esr,
                trapframe.far_el1,
                trapframe.elr,
                current_el_number(),
                trapframe.spsr,
                get_daif(),
                get_hcr_el2(),
            );

            loop {
                unsafe { asm!("wfi") }
            }
        }
    }
}

/// Handle a trapped SVE instruction without enabling general SVE execution.
///
/// Scarlet does not currently save/restore SVE Z/P/FFR state. Enabling SVE for
/// EL0 would therefore corrupt task state across context switches. Some AArch64
/// libgcc unwinder paths still use `CNTD Xt` to query the vector granule count,
/// so emulate that scalar query narrowly and leave all other SVE instructions
/// unsupported.
fn handle_sve_access_trap(trapframe: &mut Trapframe) -> bool {
    let instr_addr = trapframe.elr as usize;
    let instr = match read_user_instruction(instr_addr) {
        Some(instr) => instr,
        None => return false,
    };

    // CNTD Xt is encoded as 0x04e0e3e0 | Rt for the all-pattern variant used by
    // libgcc's AArch64 unwinder. Return the architectural minimum SVE vector
    // length expressed in doublewords: 128 bits / 64 bits = 2.
    if instr & 0xffff_ffe0 == 0x04e0_e3e0 {
        let rt = (instr & 0x1f) as usize;
        if rt < trapframe.regs.reg.len() {
            trapframe.regs.reg[rt] = 2;
        }
        trapframe.elr = trapframe.elr.wrapping_add(4);
        return true;
    }

    false
}

fn read_user_instruction(instr_addr: usize) -> Option<u32> {
    let (_kva, instr) = read_user_instruction_with_kva(instr_addr)?;
    Some(instr)
}

fn read_user_instruction_with_kva(instr_addr: usize) -> Option<(usize, u32)> {
    let task = current_task(get_cpu().get_cpuid())?;
    let instr_kva = task.vm_manager.translate_to_kva(instr_addr)?;
    Some((instr_kva, unsafe {
        core::ptr::read_unaligned(instr_kva as *const u32)
    }))
}

/// Handle instruction page fault (like RISC-V cause 12)
fn handle_instruction_fault(trapframe: &mut Trapframe, vaddr: usize) {
    let task = current_task(get_cpu().get_cpuid()).unwrap();
    let pc = trapframe.get_current_pc();

    let access = AccessKind {
        op: AccessOp::Instruction,
        vaddr,
        size: None,
    };

    match task.vm_manager.lazy_map_page_with(access) {
        Ok(_) => (),
        Err(e) => {
            terminate_current_user_fault(
                trapframe,
                get_esr_el1(),
                "instruction",
                vaddr,
                false,
                pc,
                e,
            );
        }
    }
}

/// Handle data page fault (like RISC-V cause 13/15)
fn handle_data_fault(trapframe: &mut Trapframe, vaddr: usize, is_write: bool) {
    crate::breadcrumb::drop(
        crate::breadcrumb::DATA_FAULT_ENTER,
        vaddr as u64,
        if is_write { 1 } else { 0 },
    );
    let task = current_task(get_cpu().get_cpuid()).unwrap();
    let pc = trapframe.get_current_pc();

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

    match task.vm_manager.lazy_map_page_with(access) {
        Ok(_) => {
            crate::breadcrumb::drop(crate::breadcrumb::DATA_FAULT_DONE, vaddr as u64, 0);
        }
        Err(e) => {
            terminate_current_user_fault(trapframe, get_esr_el1(), "data", vaddr, is_write, pc, e);
        }
    }
}

fn terminate_current_user_fault(
    trapframe: &mut Trapframe,
    esr: u64,
    fault_kind: &str,
    vaddr: usize,
    is_write: bool,
    pc: u64,
    reason: &'static str,
) {
    let event_kind = match fault_kind {
        "instruction" => "instruction fault",
        "data" => "data fault",
        other => other,
    };
    terminate_current_user_exception(
        trapframe,
        esr,
        event_kind,
        vaddr,
        is_write,
        pc,
        reason,
        USER_PAGE_FAULT_EXIT_STATUS,
    );
}

fn terminate_current_user_exception(
    trapframe: &mut Trapframe,
    esr: u64,
    event_kind: &str,
    vaddr: usize,
    is_write: bool,
    pc: u64,
    reason: &'static str,
    exit_status: i32,
) {
    print_trap_info(trapframe, esr);
    if let Some(task) = current_task(get_cpu().get_cpuid()) {
        println!(
            "Task {} (PID {}) caused {} at vaddr: {:#x} (write={}) from PC: {:#x}: {}",
            task.name.read(),
            task.get_id(),
            event_kind,
            vaddr,
            is_write,
            pc,
            reason
        );
        log_user_fault_memory_context(&task, trapframe, vaddr);
        log_user_code_context(&task, trapframe.elr as usize);
        crate::arch::log_user_backtrace(&task, trapframe);
        task.vcpu.lock().store(trapframe);
        task.exit_group(exit_status);
        schedule(trapframe);
        return;
    }

    panic!(
        "Unhandled {} at vaddr: {:#x} (write={}) from PC: {:#x}: {}",
        event_kind, vaddr, is_write, pc, reason
    );
}

fn log_user_fault_memory_context(task: &crate::task::Task, trapframe: &Trapframe, vaddr: usize) {
    let brk = task.get_brk();
    let text_size = task.text_size.load(core::sync::atomic::Ordering::SeqCst);
    let data_size = task.data_size.load(core::sync::atomic::Ordering::SeqCst);
    let vcpu_tls = task.vcpu.lock().get_tls_pointer();
    println!(
        "[fault] task memory: brk={:#x} text_size={:#x} data_size={:#x} trap_tls={:#x} vcpu_tls={:#x}",
        brk, text_size, data_size, trapframe.tpidr_el0, vcpu_tls
    );

    if let Some(map) = task.vm_manager.search_memory_map(vaddr) {
        println!(
            "[fault] map hit: va={:#x}-{:#x} pa={:#x}-{:#x} vm_start={:#x} perm={:#x} shared={} attr={:?} owner={}",
            map.vmarea.start,
            map.vmarea.end,
            map.pmarea.start,
            map.pmarea.end,
            map.vm_start,
            map.permissions,
            map.is_shared,
            map.memory_attribute,
            map.owner
                .as_ref()
                .map(|owner| owner.mmap_owner_name())
                .unwrap_or_else(|| alloc::string::String::from("none"))
        );
        return;
    }

    let (prev, next) = task.vm_manager.with_memmaps(|maps| {
        let prev = maps.range(..=vaddr).next_back().map(|(_, map)| {
            (
                map.vmarea.start,
                map.vmarea.end,
                map.vm_start,
                map.permissions,
            )
        });
        let next = maps.range(vaddr..).next().map(|(_, map)| {
            (
                map.vmarea.start,
                map.vmarea.end,
                map.vm_start,
                map.permissions,
            )
        });
        (prev, next)
    });

    match prev {
        Some((start, end, vm_start, permissions)) => println!(
            "[fault] previous map: va={:#x}-{:#x} vm_start={:#x} perm={:#x}",
            start, end, vm_start, permissions
        ),
        None => println!("[fault] previous map: none"),
    }

    match next {
        Some((start, end, vm_start, permissions)) => println!(
            "[fault] next map: va={:#x}-{:#x} vm_start={:#x} perm={:#x}",
            start, end, vm_start, permissions
        ),
        None => println!("[fault] next map: none"),
    }
}

fn log_user_code_context(task: &crate::task::Task, pc: usize) {
    match task.vm_manager.translate_to_kva(pc) {
        Some(kva) => {
            let w0 = unsafe { core::ptr::read_unaligned(kva as *const u32) };
            let w1 = unsafe { core::ptr::read_unaligned((kva + 4) as *const u32) };
            let w2 = unsafe { core::ptr::read_unaligned((kva + 8) as *const u32) };
            let w3 = unsafe { core::ptr::read_unaligned((kva + 12) as *const u32) };
            println!(
                "[fault] code words: pc={:#x} kva={:#x} words={:08x} {:08x} {:08x} {:08x}",
                pc, kva, w0, w1, w2, w3
            );
        }
        None => println!("[fault] code words: pc={:#x} unmapped", pc),
    }
}

/// Print trap information for debugging
fn print_trap_info(trapframe: &Trapframe, esr: u64) {
    let far = trapframe.far_el1;
    let ec = (esr >> 26) & 0x3f;
    let iss = esr & 0x1ffffff;
    let fsc = iss & 0x3f;

    crate::println!("=== Trap Info ===");
    crate::println!("ESR_EL1: {:#018x} (EC={:#x}, FSC={:#x})", esr, ec, fsc);
    crate::println!("FAR_EL1: {:#018x}", far);
    crate::println!("ELR_EL1: {:#018x}", trapframe.elr);
    match read_user_instruction_with_kva(trapframe.elr as usize) {
        Some((kva, instr)) => crate::println!("INSN_ELR: {:#010x} (kva={:#x})", instr, kva),
        None => crate::println!("INSN_ELR: <unmapped>"),
    }

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
    crate::println!(
        "x8={:#x} x9={:#x} x10={:#x} x11={:#x}",
        trapframe.regs.reg[8],
        trapframe.regs.reg[9],
        trapframe.regs.reg[10],
        trapframe.regs.reg[11]
    );
    crate::println!(
        "x12={:#x} x13={:#x} x14={:#x} x15={:#x}",
        trapframe.regs.reg[12],
        trapframe.regs.reg[13],
        trapframe.regs.reg[14],
        trapframe.regs.reg[15]
    );
    crate::println!(
        "x16={:#x} x17={:#x} x18={:#x} x19={:#x}",
        trapframe.regs.reg[16],
        trapframe.regs.reg[17],
        trapframe.regs.reg[18],
        trapframe.regs.reg[19]
    );
    crate::println!(
        "x20={:#x} x21={:#x} x22={:#x} x23={:#x}",
        trapframe.regs.reg[20],
        trapframe.regs.reg[21],
        trapframe.regs.reg[22],
        trapframe.regs.reg[23]
    );
    crate::println!(
        "x24={:#x} x25={:#x} x26={:#x} x27={:#x}",
        trapframe.regs.reg[24],
        trapframe.regs.reg[25],
        trapframe.regs.reg[26],
        trapframe.regs.reg[27]
    );
    crate::println!(
        "x28={:#x} x29={:#x} x30={:#x} sp={:#x} spsr={:#x}",
        trapframe.regs.reg[28],
        trapframe.regs.reg[29],
        trapframe.regs.reg[30],
        trapframe.sp,
        trapframe.spsr
    );
}
