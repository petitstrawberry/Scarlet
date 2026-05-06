use core::arch::asm;
use core::panic;

use crate::abi::syscall_dispatcher;
use crate::arch::trap::print_traplog;
use crate::arch::{Trapframe, get_cpu};
use crate::println;
use crate::sched::scheduler::get_scheduler;
use crate::task::mytask;

fn log_fatal_page_fault_context(
    trapframe: &Trapframe,
    cause: usize,
    vaddr: usize,
    task_id: usize,
    task_name: &str,
    asid: u16,
) {
    use crate::arch::vm::{get_root_pagetable_ptr, is_asid_used};

    let cpu_id = get_cpu().get_cpuid();
    let epc = trapframe.epc as usize;
    // RISC-V integer registers: x1=ra, x2=sp, x8=s0/fp
    let ra = trapframe.regs.reg[1] as usize;
    let sp = trapframe.regs.reg[2] as usize;
    let fp = trapframe.regs.reg[8] as usize;

    let asid_used = is_asid_used(asid);
    let root_pt = get_root_pagetable_ptr(asid).unwrap_or(core::ptr::null_mut());

    println!(
        "[Trap] fatal page fault map failed: cpu={} cause={} task_id={} name={} asid={} asid_used={} root_pt={:p}",
        cpu_id, cause, task_id, task_name, asid, asid_used, root_pt
    );
    println!(
        "[Trap] epc={:#x} vaddr={:#x} ra={:#x} sp={:#x} fp={:#x}",
        epc, vaddr, ra, sp, fp
    );
}

const INSTRUCTION_ADDRESS_MISALIGNED: usize = 0;
const INSTRUCTION_ACCESS_FAULT: usize = 1;
const ILLEGAL_INSTRUCTION: usize = 2;
const BREAKPOINT: usize = 3;
const LOAD_ADDRESS_MISALIGNED: usize = 4;
const LOAD_ACCESS_FAULT: usize = 5;
const STORE_ADDRESS_MISALIGNED: usize = 6;
const STORE_ACCESS_FAULT: usize = 7;
const ECALL_FROM_U_MODE: usize = 8;
const ECALL_FROM_HS_MODE: usize = 9;
const ECALL_FROM_VS_MODE: usize = 10;
const ECALL_FROM_M_MODE: usize = 11;
const INSTRUCTION_PAGE_FAULT: usize = 12;
const LOAD_PAGE_FAULT: usize = 13;
const STORE_PAGE_FAULT: usize = 15;
const INSTRUCTION_GUEST_PAGE_FAULT: usize = 20;
const LOAD_GUEST_PAGE_FAULT: usize = 21;
const VIRTUAL_INSTRUCTION: usize = 22;
const STORE_GUEST_PAGE_FAULT: usize = 23;

pub fn arch_exception_handler(trapframe: &mut Trapframe, cause: usize) {
    match cause {
        /* Illegal instruction (used for lazy FP/Vector enable) */
        ILLEGAL_INSTRUCTION => {
            #[cfg(all(feature = "hypervisor", target_arch = "riscv64"))]
            if crate::arch::hv::trap::is_from_guest() {
                use crate::arch::hv::switch::arch_guest_trap_exit;
                unsafe {
                    arch_guest_trap_exit();
                }
                unreachable!();
            }

            let task = get_scheduler()
                .get_current_task(get_cpu().get_cpuid())
                .unwrap();

            let user_fpu_allowed = crate::arch::user_fpu_enabled();
            let user_vec_allowed = crate::arch::user_vector_enabled();

            // Read stval (may contain the faulting instruction word; can also be 0).
            let mut inst: usize;
            let sstatus: usize;
            unsafe {
                asm!(
                    "csrr {0}, stval",
                    "csrr {1}, sstatus",
                    out(reg) inst,
                    out(reg) sstatus,
                );
            }

            // If stval doesn't contain the instruction, try to fetch it from the task's
            // mapped user code via the VM translation.
            if inst == 0 {
                if let Some(paddr) = task.vm_manager.translate_to_kva(trapframe.epc as usize) {
                    inst = crate::arch::instruction::Instruction::fetch(paddr).raw as usize;
                }
            }

            // Helpers for determining whether we should treat this as a lazy enable trap.
            let fs_off = (sstatus & 0x6000) == 0;
            let vs_off = (sstatus & 0x600) == 0;

            let raw32 = inst as u32;
            let is_32bit = (raw32 & 0b11) == 0b11;

            // Vector instructions are always 32-bit.
            let is_vector_insn = if is_32bit {
                let opcode = raw32 & 0x7f;
                if opcode == 0x57 {
                    true
                } else if opcode == 0x73 {
                    // SYSTEM/CSR access: treat v* CSRs as vector-related.
                    let csr = (raw32 >> 20) & 0xfff;
                    // vstart..vcsr, vl..vlenb
                    (csr >= 0x008 && csr <= 0x00a) || (csr >= 0xc20 && csr <= 0xc22)
                } else {
                    false
                }
            } else {
                false
            };

            let is_fpu_insn = if is_32bit {
                let opcode = raw32 & 0x7f;
                let funct3 = (raw32 >> 12) & 0x7;
                match opcode {
                    // FP arithmetic and conversion ops.
                    0x53 | 0x43 | 0x47 | 0x4b | 0x4f => true,
                    // FP loads/stores.
                    0x07 | 0x27 => matches!(funct3, 0b010 | 0b011 | 0b100),
                    _ => false,
                }
            } else {
                // Minimal support for common compressed FP loads/stores.
                let raw16 = (raw32 & 0xffff) as u16;
                let quadrant = raw16 & 0b11;
                let funct3 = (raw16 >> 13) & 0x7;

                // C.FLD/C.FSD (quadrant 0), C.FLDSP/C.FSDSP (quadrant 2)
                (quadrant == 0b00 || quadrant == 0b10) && matches!(funct3, 0b001 | 0b101)
            };

            // Handle lazy enable without relying purely on instruction decoding.
            // This avoids panicking if stval is 0 or if we cannot classify the instruction.
            #[cfg(feature = "user-vector")]
            if user_vec_allowed && vs_off && is_vector_insn {
                task.vcpu.lock().vector_used = true;
                crate::arch::riscv64::fpu::enable_vector();
                if task.vcpu.lock().vector.is_none() {
                    task.vcpu.lock().vector = Some(alloc::boxed::Box::new(
                        crate::arch::riscv64::fpu::VectorContext::new(),
                    ));
                }
                unsafe { task.vcpu.lock().vector.as_ref().unwrap().restore() };
                crate::arch::riscv64::fpu::mark_vector_clean();
                return;
            }

            #[cfg(feature = "user-fpu")]
            if user_fpu_allowed && fs_off && is_fpu_insn {
                task.vcpu.lock().fpu_used = true;
                crate::arch::riscv64::fpu::enable_fpu();
                unsafe { task.vcpu.lock().fpu.restore() };
                crate::arch::riscv64::fpu::mark_fpu_clean();
                return;
            }

            // Fallback: if the relevant extension is disabled, enable it and restore a
            // safe initial context (prevents leaking previous task state).
            #[cfg(feature = "user-fpu")]
            if user_fpu_allowed && fs_off {
                task.vcpu.lock().fpu_used = true;
                crate::arch::riscv64::fpu::enable_fpu();
                unsafe { task.vcpu.lock().fpu.restore() };
                crate::arch::riscv64::fpu::mark_fpu_clean();
                return;
            }

            #[cfg(feature = "user-vector")]
            if user_vec_allowed && vs_off {
                task.vcpu.lock().vector_used = true;
                crate::arch::riscv64::fpu::enable_vector();
                if task.vcpu.lock().vector.is_none() {
                    task.vcpu.lock().vector = Some(alloc::boxed::Box::new(
                        crate::arch::riscv64::fpu::VectorContext::new(),
                    ));
                }
                unsafe { task.vcpu.lock().vector.as_ref().unwrap().restore() };
                crate::arch::riscv64::fpu::mark_vector_clean();
                return;
            }

            print_traplog(trapframe);
            panic!(
                "Unhandled illegal instruction: inst={:#x} epc={:#x}",
                raw32, trapframe.epc
            );
        }
        BREAKPOINT => {
            #[cfg(all(feature = "hypervisor", target_arch = "riscv64"))]
            if crate::arch::hv::trap::is_from_guest() {
                use crate::arch::hv::switch::arch_guest_trap_exit;
                unsafe {
                    arch_guest_trap_exit();
                }
                unreachable!();
            }

            print_traplog(trapframe);
            panic!("Unhandled breakpoint: epc={:#x}", trapframe.epc);
        }
        /* Environment call from U-mode */
        ECALL_FROM_U_MODE => {
            /* Execute SystemCall */
            match syscall_dispatcher(trapframe) {
                Ok(ret) => {
                    trapframe.set_return_value(ret);
                }
                Err(msg) => {
                    // panic!("Syscall error: {}", msg);
                    // println!("Syscall error: {}", msg);
                    trapframe.set_return_value(usize::MAX); // Set error code: -1
                    trapframe.increment_pc_next(mytask().unwrap());
                }
            }
        }
        /* Instruction page fault */
        INSTRUCTION_PAGE_FAULT => {
            let mut vaddr = trapframe.epc as usize;
            let task = get_scheduler()
                .get_current_task(get_cpu().get_cpuid())
                .unwrap();
            use crate::object::capability::memory_mapping::{AccessKind, AccessOp};
            loop {
                let access = AccessKind {
                    op: AccessOp::Instruction,
                    vaddr,
                    size: None,
                };
                match task.vm_manager.lazy_map_page_with(access) {
                    Ok(_) => (),
                    Err(_) => {
                        print_traplog(trapframe);
                        log_fatal_page_fault_context(
                            trapframe,
                            cause,
                            vaddr,
                            task.get_id(),
                            &task.name.read(),
                            task.vm_manager.get_asid(),
                        );
                        panic!(
                            "Failed to map page for instruction page fault at vaddr: {:#x}",
                            vaddr
                        );
                    }
                }

                if vaddr & 0b11 == 0 {
                    // If the address is aligned, we can stop
                    break;
                }
                vaddr = (vaddr + 4) & !0b11; // Align to the next 4-byte boundary
            }
        }
        /* Load/Store page fault */
        LOAD_PAGE_FAULT | STORE_PAGE_FAULT => {
            let mut vaddr;
            unsafe {
                asm!("csrr {}, stval", out(reg) vaddr);
            }
            let task = get_scheduler()
                .get_current_task(get_cpu().get_cpuid())
                .unwrap();
            use crate::object::capability::memory_mapping::{AccessKind, AccessOp};
            loop {
                let op = if cause == LOAD_PAGE_FAULT {
                    AccessOp::Load
                } else {
                    AccessOp::Store
                };
                let access = AccessKind {
                    op,
                    vaddr,
                    size: None,
                };
                match task.vm_manager.lazy_map_page_with(access) {
                    Ok(_) => (),
                    Err(e) => {
                        print_traplog(trapframe);
                        log_fatal_page_fault_context(
                            trapframe,
                            cause,
                            vaddr,
                            task.get_id(),
                            &task.name.read(),
                            task.vm_manager.get_asid(),
                        );
                        panic!("lazy_map_page_with failed for vaddr={:#x}: {}", vaddr, e);
                    }
                }

                if vaddr & 0b11 == 0 {
                    // If the address is aligned, we can stop
                    break;
                }
                vaddr = (vaddr + 4) & !0b11;
            }
        }
        #[cfg(all(feature = "hypervisor", target_arch = "riscv64"))]
        INSTRUCTION_GUEST_PAGE_FAULT
        | LOAD_GUEST_PAGE_FAULT
        | STORE_GUEST_PAGE_FAULT
        | ECALL_FROM_VS_MODE => {
            use crate::arch::hv::switch::arch_guest_trap_exit;
            unsafe {
                arch_guest_trap_exit();
            }
        }
        _ => {
            print_traplog(trapframe);
            panic!("Unhandled exception: {}", cause);
        }
    }
}
