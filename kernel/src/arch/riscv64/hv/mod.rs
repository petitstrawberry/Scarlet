//! RISC-V H-extension hypervisor support

pub mod csr;
pub mod guest_vcpu;
pub mod mmu;
pub mod reg_index;
pub mod switch;
pub mod trap;
pub mod vm;

use core::sync::atomic::{compiler_fence, fence};

use alloc::sync::Arc;
pub use guest_vcpu::GuestVcpu;
pub use switch::{arch_guest_trap_exit, arch_run_guest_loop};
pub use trap::{arch_guest_trap_handler, clear_guest_mode, is_from_guest};
pub use vm::{Riscv64VmObject, RiscvVmState, Vm};

use crate::{
    arch::hv::csr::{
        read_hvip, write_hcounteren, write_hedeleg, write_hgatp, write_hgeie, write_hideleg,
        write_hie, write_hstatus, write_hvip, write_vsatp, write_vsepc, write_vsie, write_vsip,
        write_vsscratch, write_vstval, write_vstvec,
    },
    hypervisor::vm::VmId,
};

pub fn create_vm(id: VmId) -> Result<Arc<Vm>, &'static str> {
    Ok(Arc::new(Riscv64VmObject::new(id)?))
}

pub fn arch_init_hv() {
    crate::println!("[shv] Initializing RISC-V H-extension support");
}

pub fn init_hv_per_cpu(cpu_id: usize) {
    use crate::arch::riscv64::trap::cause::*;

    crate::println!(
        "[shv] Initializing RISC-V H-extension CSRs for CPU {}",
        cpu_id
    );

    // HS-mode CSRs
    write_hstatus(0x0);
    write_hcounteren(0x2); // Enable guest access to the time register (rdtime)
    write_hgatp(0); // Start with no guest page tables mapped

    // Delegate virtual supervisor software, timer, and external interrupts to guest mode by default
    write_hideleg(
        1 << INTERRUPT_VIRTUAL_SUPERVISOR_SOFTWARE
            | 1 << INTERRUPT_VIRTUAL_SUPERVISOR_TIMER
            | 1 << INTERRUPT_VIRTUAL_SUPERVISOR_EXTERNAL,
    );

    // Delegate common exceptions to guest mode by default (guest can always modify this later)
    write_hedeleg(
        1 << EXCEPTION_INSTRUCTION_ADDRESS_MISALIGNED
            | 1 << EXCEPTION_BREAKPOINT
            | 1 << EXCEPTION_ENVIRONMENT_CALL_FROM_UMODE_OR_VUMODE
            | 1 << EXCEPTION_INSTRUCTION_PAGE_FAULT
            | 1 << EXCEPTION_LOAD_PAGE_FAULT
            | 1 << EXCEPTION_STORE_AMO_PAGE_FAULT,
    );

    write_hgeie(0); // Disable all guest external interrupts by default (When Scarlet supports AIA, we can enable specific interrupts here)

    // Disable all hypervisor interrupts for now
    write_hie(0);
    write_hvip(0); // Clear any pending virtual interrupts

    // VS-mode CSRs
    write_vsatp(0);
    write_vsscratch(0);
    write_vsip(0);
    write_vsie(0);
    write_vstval(0);
    write_vstvec(0);
    write_vsepc(0);
}
