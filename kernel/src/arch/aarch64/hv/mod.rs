pub mod guest_vcpu;
pub mod mmu;
pub mod pl011_mmio;
pub mod reg_index;
pub mod switch;
pub mod sysreg;
pub mod trap;
pub mod vgic;
pub mod vgic_mmio;
pub mod vm;

use alloc::sync::Arc;
use core::arch::asm;

pub use guest_vcpu::GuestVcpu;
pub use switch::{arch_guest_trap_exit, arch_run_guest_loop, el2_guest_exit_vector};
pub use trap::{arch_guest_trap_handler, clear_guest_mode, is_from_guest};
pub use vm::Vm;

use crate::arch::get_kernel_trapvector_paddr;
use crate::hypervisor::vm::VmId;
use crate::vm::manager::VirtualMemoryManager;

const VTCR_EL2_RES1: u64 = (1 << 31) | (1 << 23);
const VTCR_EL2_PS_40BIT: u64 = 0b010 << 16;
const VTCR_EL2_TG0_4K: u64 = 0b00 << 14;
const VTCR_EL2_SH0_INNER_SHAREABLE: u64 = 0b11 << 12;
const VTCR_EL2_ORGN0_WB: u64 = 0b01 << 10;
const VTCR_EL2_IRGN0_WB: u64 = 0b01 << 8;
const VTCR_EL2_SL0_L1: u64 = 0b01 << 6;
const VTCR_EL2_T0SZ_40BIT_IPA: u64 = 24;

const CNTHCTL_EL2_EL1PCEN: u64 = 1 << 1;
const CNTHCTL_EL2_EL1PCTEN: u64 = 1 << 0;
const CPTR_EL2_FPEN_EL1_EL0: u64 = 0b11 << 20;

fn vtcr_el2_value() -> u64 {
    VTCR_EL2_RES1
        | VTCR_EL2_PS_40BIT
        | VTCR_EL2_TG0_4K
        | VTCR_EL2_SH0_INNER_SHAREABLE
        | VTCR_EL2_ORGN0_WB
        | VTCR_EL2_IRGN0_WB
        | VTCR_EL2_SL0_L1
        | VTCR_EL2_T0SZ_40BIT_IPA
}

pub fn create_vm(id: VmId, owner_mm: VirtualMemoryManager) -> Result<Arc<Vm>, &'static str> {
    Ok(Arc::new(vm::Vm::new(id, owner_mm)?))
}

pub fn arch_init_hv() {
    if !super::super::is_hv_available() {
        crate::println!("[shv] EL2 not available, hypervisor disabled");
        return;
    }

    let vtcr_el2 = vtcr_el2_value();

    // SAFETY: the kernel is running at EL2 in VHE mode when hypervisor support
    // is available, so programming EL2 control registers is valid here.
    unsafe {
        asm!("msr vtcr_el2, {vtcr}", "isb", vtcr = in(reg) vtcr_el2, options(nostack));
    }

    if super::super::is_vhe_enabled() {
        crate::println!(
            "[shv] AArch64 hypervisor init: VHE=on VTCR_EL2={:#x} IPA=40b TG0=4K SL0=L1",
            vtcr_el2
        );
    } else {
        crate::println!(
            "[shv] AArch64 hypervisor init: VHE=off VTCR_EL2={:#x} IPA=40b TG0=4K SL0=L1",
            vtcr_el2
        );
    }
}

pub fn init_hv_per_cpu(cpu_id: usize) {
    let vtcr_el2 = vtcr_el2_value();
    let cnthctl_el2 = CNTHCTL_EL2_EL1PCEN | CNTHCTL_EL2_EL1PCTEN;
    let host_vbar = get_kernel_trapvector_paddr();

    // VTCR_EL2 is banked per CPU. A vCPU task may run on any online host CPU,
    // so every CPU must have the same stage-2 translation regime configured.
    // SAFETY: per-CPU hypervisor initialization runs on the current CPU while
    // executing at EL2, so EL2 timer/trap registers are directly accessible.
    unsafe {
        asm!(
            "msr vtcr_el2, {vtcr}",
            "msr cnthctl_el2, {cnthctl}",
            "msr cptr_el2, {cptr}",
            "msr vbar_el2, {vbar}",
            "msr vttbr_el2, xzr",
            "isb",
            vtcr = in(reg) vtcr_el2,
            cnthctl = in(reg) cnthctl_el2,
            cptr = in(reg) CPTR_EL2_FPEN_EL1_EL0,
            vbar = in(reg) host_vbar,
            options(nostack),
        );
    }

    crate::println!(
        "[shv] AArch64 hypervisor per-cpu init: cpu={} VTCR_EL2={:#x} CNTHCTL_EL2={:#x} CPTR_EL2={:#x}",
        cpu_id,
        vtcr_el2,
        cnthctl_el2,
        CPTR_EL2_FPEN_EL1_EL0
    );

    let num_lrs = vgic::probe_vgic();
    crate::println!(
        "[shv] AArch64 VGICv3 probed: {} List Registers available",
        num_lrs
    );
}
