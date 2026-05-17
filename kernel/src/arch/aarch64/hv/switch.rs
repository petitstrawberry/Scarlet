use core::arch::asm;
use core::arch::naked_asm;

use crate::arch::{Arch, Trapframe};

use super::guest_vcpu::GuestVcpu;
use super::sysreg::{GuestSystemRegs, HypervisorSystemRegs, capture_guest_sysregs};

const fn sys_reg(op0: u32, op1: u32, crn: u32, crm: u32, op2: u32) -> u32 {
    (op0 << 14) | (op1 << 11) | (crn << 7) | (crm << 3) | op2
}

const SYS_VBAR_EL12: u32 = sys_reg(3, 5, 12, 0, 0);
const SYS_SCTLR_EL12: u32 = sys_reg(3, 5, 1, 0, 0);
const SYS_TCR_EL12: u32 = sys_reg(3, 5, 2, 0, 2);
const SYS_TTBR0_EL12: u32 = sys_reg(3, 5, 2, 0, 0);
const SYS_TTBR1_EL12: u32 = sys_reg(3, 5, 2, 0, 1);
const SYS_MAIR_EL12: u32 = sys_reg(3, 5, 10, 2, 0);
const SYS_AMAIR_EL12: u32 = sys_reg(3, 5, 10, 3, 0);
const SYS_ELR_EL12: u32 = sys_reg(3, 5, 4, 0, 1);
const SYS_SPSR_EL12: u32 = sys_reg(3, 5, 4, 0, 0);
const SYS_ESR_EL12: u32 = sys_reg(3, 5, 5, 2, 0);
const SYS_FAR_EL12: u32 = sys_reg(3, 5, 6, 0, 0);
const SYS_CPACR_EL12: u32 = sys_reg(3, 5, 1, 0, 2);
const SYS_CONTEXTIDR_EL12: u32 = sys_reg(3, 5, 13, 0, 1);
const SYS_CNTKCTL_EL12: u32 = sys_reg(3, 5, 14, 1, 0);

/// When TGE=1 (host VHE mode), _EL12 registers like ESR_EL12 and FAR_EL12
/// are UNDEF. Only save/restore guest state when actually in guest context
/// (VM bit set in HCR_EL2, meaning a guest was entered).
fn is_guest_context() -> bool {
    let hcr_el2: u64;
    // SAFETY: reading HCR_EL2 at EL2 is side-effect free.
    unsafe {
        asm!("mrs {}, HCR_EL2", out(reg) hcr_el2, options(nostack));
    }
    (hcr_el2 & HCR_EL2_VM) != 0
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct HostHvContext {
    pub(crate) hcr_el2: u64,
    pub(crate) vbar_el2: u64,
    pub(crate) vbar_el1: u64,
    pub(crate) ich_hcr_el2: u64,
    pub(crate) ich_vmcr_el2: u64,
    pub(crate) tpidr_el1: u64,
    pub(crate) daif: u64,
    pub(crate) cnthctl_el2: u64,
    pub(crate) cntv_ctl_el0: u64,
    pub(crate) cntv_cval_el0: u64,
    pub(crate) cntvoff_el2: u64,
    pub(crate) sp_el0: u64,
    pub(crate) tpidr_el0: u64,
    pub(crate) tpidrro_el0: u64,
    pub(crate) cntkctl_el1: u64,
}

pub(crate) const HCR_EL2_VM: u64 = 1 << 0;
pub(crate) const HCR_EL2_SWIO: u64 = 1 << 1;
pub(crate) const HCR_EL2_PTW: u64 = 1 << 2;
pub(crate) const HCR_EL2_FMO: u64 = 1 << 3;
pub(crate) const HCR_EL2_IMO: u64 = 1 << 4;
pub(crate) const HCR_EL2_AMO: u64 = 1 << 5;
pub(crate) const HCR_EL2_FB: u64 = 1 << 9;
pub(crate) const HCR_EL2_BSU_IS: u64 = 0b11 << 10;
pub(crate) const HCR_EL2_TWI: u64 = 1 << 13;
pub(crate) const HCR_EL2_TWE: u64 = 1 << 14;
pub(crate) const HCR_EL2_TID1: u64 = 1 << 16;
pub(crate) const HCR_EL2_TID3: u64 = 1 << 18;
pub(crate) const HCR_EL2_TSC: u64 = 1 << 19;
pub(crate) const HCR_EL2_TIDCP: u64 = 1 << 20;
pub(crate) const HCR_EL2_TACR: u64 = 1 << 21;
pub(crate) const HCR_EL2_TSW: u64 = 1 << 22;
pub(crate) const HCR_EL2_TGE: u64 = 1 << 27;
pub(crate) const HCR_EL2_RW: u64 = 1 << 31;
pub(crate) const HCR_EL2_E2H: u64 = 1 << 34;

pub(crate) const HCR_EL2_HOST: u64 =
    HCR_EL2_E2H | HCR_EL2_TGE | HCR_EL2_RW | HCR_EL2_SWIO | HCR_EL2_FMO | HCR_EL2_IMO | HCR_EL2_AMO;
pub(crate) const HCR_EL2_GUEST: u64 = HCR_EL2_E2H
    | HCR_EL2_VM
    | HCR_EL2_SWIO
    | HCR_EL2_PTW
    | HCR_EL2_FMO
    | HCR_EL2_IMO
    | HCR_EL2_AMO
    | HCR_EL2_FB
    | HCR_EL2_BSU_IS
    | HCR_EL2_RW
    | HCR_EL2_TWI
    | HCR_EL2_TWE
    | HCR_EL2_TID1
    | HCR_EL2_TID3
    | HCR_EL2_TSC
    | HCR_EL2_TIDCP
    | HCR_EL2_TACR
    | HCR_EL2_TSW;
const HCR_EL2_GUEST_LO16: u16 = (HCR_EL2_GUEST & 0xffff) as u16;
const HCR_EL2_GUEST_HI16: u16 = ((HCR_EL2_GUEST >> 16) & 0xffff) as u16;
const HCR_EL2_GUEST_HI32: u16 = ((HCR_EL2_GUEST >> 32) & 0xffff) as u16;

// In VHE mode (E2H=1), CNTHCTL_EL2 uses a different bit layout:
//   bit 0  = EL0VTEN  (EL0 virtual timer access)
//   bit 1  = EL0VCTEN (EL0 virtual counter access)
//   bit 10 = EL1PCTEN (EL1 physical counter access, shifted by 10 in VHE)
//   bit 11 = EL1PCEN  (EL1 physical timer access, shifted by 10 in VHE)
//   bit 13 = EL1TVT   (trap EL1 virtual timer accesses in VHE)
//   bit 14 = EL1TVCT  (trap EL1 virtual counter accesses in VHE)
const CNTHCTL_EL2_EL0VTEN: u64 = 1 << 0;
const CNTHCTL_EL2_EL0VCTEN: u64 = 1 << 1;
const CNTHCTL_EL2_EL1TVT: u64 = 1 << 13; // VHE: trap EL1 virtual timer (CNTV_CTL/CVAL/TVAL)
const CNTHCTL_EL2_GUEST: u64 = CNTHCTL_EL2_EL0VTEN | CNTHCTL_EL2_EL0VCTEN | CNTHCTL_EL2_EL1TVT;
const CNTHCTL_EL2_GUEST_LO16: u16 = (CNTHCTL_EL2_GUEST & 0xffff) as u16;

pub(crate) static mut HOST_HV_CTX: HostHvContext = HostHvContext {
    hcr_el2: 0,
    vbar_el2: 0,
    vbar_el1: 0,
    ich_hcr_el2: 0,
    ich_vmcr_el2: 0,
    tpidr_el1: 0,
    daif: 0,
    cnthctl_el2: 0,
    cntv_ctl_el0: 0,
    cntv_cval_el0: 0,
    cntvoff_el2: 0,
    sp_el0: 0,
    tpidr_el0: 0,
    tpidrro_el0: 0,
    cntkctl_el1: 0,
};

pub(crate) static mut GUEST_TRAPFRAME_PTR: usize = 0;

unsafe extern "C" {
    fn el2_guest_exit_vector_base();
}

pub(crate) fn guest_exit_vector_base() -> usize {
    el2_guest_exit_vector_base as usize
}

pub struct VcpuSwitchData {
    guest_sysregs: GuestSystemRegs,
}

pub struct HypervisorSwitchData {
    hypervisor_sysregs: HypervisorSystemRegs,
}

impl HypervisorSwitchData {
    pub fn save() -> Self {
        if !is_guest_context() {
            return Self::default();
        }
        HypervisorSwitchData {
            hypervisor_sysregs: HypervisorSystemRegs::save(),
        }
    }

    pub fn restore(&self) {
        if !is_guest_context() {
            return;
        }
        self.hypervisor_sysregs.restore();
    }
}

impl Default for HypervisorSwitchData {
    fn default() -> Self {
        Self {
            hypervisor_sysregs: HypervisorSystemRegs::default(),
        }
    }
}

impl VcpuSwitchData {
    pub fn save() -> Self {
        if !is_guest_context() {
            return Self::default();
        }
        VcpuSwitchData {
            guest_sysregs: GuestSystemRegs::save(),
        }
    }

    pub fn restore(&self) {
        if !is_guest_context() {
            return;
        }
        self.guest_sysregs.restore();
    }
}

impl Default for VcpuSwitchData {
    fn default() -> Self {
        Self {
            guest_sysregs: GuestSystemRegs::default(),
        }
    }
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn el2_guest_exit_vector() {
    naked_asm!(
        ".balign 0x800",
        ".global el2_guest_exit_vector_base",
        "el2_guest_exit_vector_base:",
        ".balign 0x80",
        "b 1f",
        ".balign 0x80",
        "b 2f",
        ".balign 0x80",
        "b 3f",
        ".balign 0x80",
        "b 4f",
        ".balign 0x80",
        "b 1f",
        ".balign 0x80",
        "b 2f",
        ".balign 0x80",
        "b 3f",
        ".balign 0x80",
        "b 4f",
        ".balign 0x80",
        "b 1f",
        ".balign 0x80",
        "b 2f",
        ".balign 0x80",
        "b 3f",
        ".balign 0x80",
        "b 4f",
        ".balign 0x80",
        "b .",
        ".balign 0x80",
        "b .",
        ".balign 0x80",
        "b .",
        ".balign 0x80",
        "b .",
        "1:",
        "str x30, [sp, #-16]!",
        "stp x0, x1, [sp, #-16]!",
        "mov x30, #0",
        "b 5f",
        "2:",
        "str x30, [sp, #-16]!",
        "stp x0, x1, [sp, #-16]!",
        "mov x30, #1",
        "b 5f",
        "3:",
        "str x30, [sp, #-16]!",
        "stp x0, x1, [sp, #-16]!",
        "mov x30, #2",
        "b 5f",
        "4:",
        "str x30, [sp, #-16]!",
        "stp x0, x1, [sp, #-16]!",
        "mov x30, #3",
        "5:",
        "adrp x0, {trapframe_ptr}",
        "ldr x0, [x0, #:lo12:{trapframe_ptr}]",
        "stp x2, x3, [x0, #16]",
        "stp x4, x5, [x0, #32]",
        "stp x6, x7, [x0, #48]",
        "stp x8, x9, [x0, #64]",
        "stp x10, x11, [x0, #80]",
        "stp x12, x13, [x0, #96]",
        "stp x14, x15, [x0, #112]",
        "stp x16, x17, [x0, #128]",
        "stp x18, x19, [x0, #144]",
        "stp x20, x21, [x0, #160]",
        "stp x22, x23, [x0, #176]",
        "stp x24, x25, [x0, #192]",
        "stp x26, x27, [x0, #208]",
        "stp x28, x29, [x0, #224]",
        "ldr x1, [sp]",
        "ldr x2, [sp, #8]",
        "stp x1, x2, [x0, #0]",
        "ldr x1, [sp, #16]",
        "str x1, [x0, #240]",
        "mrs x1, sp_el1",
        "str x1, [x0, #248]",
        "mrs x1, elr_el2",
        "str x1, [x0, #256]",
        "mrs x1, spsr_el2",
        "str x1, [x0, #264]",
        "cbnz x30, 6f",
        "mrs x1, esr_el2",
        "b 7f",
        "6:",
        "mov x1, x30",
        "7:",
        "str x1, [x0, #288]",
        "adrp x1, {trap_exit}",
        "add x1, x1, #:lo12:{trap_exit}",
        "br x1",
        trapframe_ptr = sym GUEST_TRAPFRAME_PTR,
        trap_exit = sym arch_guest_trap_exit,
    );
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_run_guest_loop(
    _trapframe: *mut Trapframe,
    _vcpu: *const GuestVcpu,
    _arch: *const Arch,
) {
    naked_asm!(
        "sub sp, sp, #96",
        "stp x19, x20, [sp, #0]",
        "stp x21, x22, [sp, #16]",
        "stp x23, x24, [sp, #32]",
        "stp x25, x26, [sp, #48]",
        "stp x27, x28, [sp, #64]",
        "stp x29, x30, [sp, #80]",
        "adrp x3, {trapframe_ptr}",
        "str x0, [x3, #:lo12:{trapframe_ptr}]",
        "adrp x4, {guest_exit_vector_base}",
        "add x4, x4, #:lo12:{guest_exit_vector_base}",
        "msr vbar_el1, x4",
        "msr vbar_el2, x4",
        "isb",
        "mov x3, x1",
        "ldr x4, [x3, #264]",
        ".inst 0xd5000000 | ({sys_vbar_el12} << 5) | 4",
        "ldr x4, [x3, #272]",
        ".inst 0xd5000000 | ({sys_sctlr_el12} << 5) | 4",
        "ldr x4, [x3, #280]",
        ".inst 0xd5000000 | ({sys_tcr_el12} << 5) | 4",
        "ldr x4, [x3, #296]",
        ".inst 0xd5000000 | ({sys_ttbr0_el12} << 5) | 4",
        "ldr x4, [x3, #304]",
        ".inst 0xd5000000 | ({sys_ttbr1_el12} << 5) | 4",
        "ldr x4, [x3, #312]",
        ".inst 0xd5000000 | ({sys_mair_el12} << 5) | 4",
        "ldr x4, [x3, #320]",
        ".inst 0xd5000000 | ({sys_amair_el12} << 5) | 4",
        "ldr x4, [x3, #344]",
        "msr sp_el1, x4",
        "ldr x4, [x3, #352]",
        ".inst 0xd5000000 | ({sys_elr_el12} << 5) | 4",
        "ldr x4, [x3, #360]",
        ".inst 0xd5000000 | ({sys_spsr_el12} << 5) | 4",
        "ldr x4, [x3, #368]",
        ".inst 0xd5000000 | ({sys_esr_el12} << 5) | 4",
        "ldr x4, [x3, #376]",
        ".inst 0xd5000000 | ({sys_far_el12} << 5) | 4",
        "ldr x4, [x3, #384]",
        ".inst 0xd5000000 | ({sys_cpacr_el12} << 5) | 4",
        "ldr x4, [x3, #392]",
        ".inst 0xd5000000 | ({sys_contextidr_el12} << 5) | 4",
        "ldr x4, [x3, #400]",
        "msr tpidr_el1, x4",
        "ldr x4, [x3, #408]",
        "msr cntv_ctl_el0, x4",
        "ldr x4, [x3, #416]",
        "msr cntv_cval_el0, x4",
        "ldr x4, [x3, #424]",
        "msr cntvoff_el2, x4",
        "ldr x4, [x3, #432]",
        "msr sp_el0, x4",
        "ldr x4, [x3, #440]",
        "msr tpidr_el0, x4",
        "ldr x4, [x3, #448]",
        "msr tpidrro_el0, x4",
        "ldr x4, [x3, #456]",
        ".inst 0xd5000000 | ({sys_cntkctl_el12} << 5) | 4",
        "isb",
        "ldp x19, x20, [x3, #152]",
        "ldp x21, x22, [x3, #168]",
        "ldp x23, x24, [x3, #184]",
        "ldp x25, x26, [x3, #200]",
        "ldp x27, x28, [x3, #216]",
        "ldp x29, x30, [x3, #232]",
        "ldp x8, x9, [x3, #64]",
        "ldp x10, x11, [x3, #80]",
        "ldp x12, x13, [x3, #96]",
        "ldp x14, x15, [x3, #112]",
        "ldp x16, x17, [x3, #128]",
        "ldr x18, [x3, #144]",
        "ldp x4, x5, [x3, #32]",
        "ldp x6, x7, [x3, #48]",
        "ldr x2, [x3, #248]",
        "msr elr_el2, x2",
        "ldr x2, [x3, #256]",
        "msr spsr_el2, x2",
        "ldr x2, [x3, #344]",
        "msr sp_el1, x2",
        "ldp x0, x1, [x3, #0]",
        "ldp x2, x3, [x3, #16]",
        "msr tpidr_el2, x30",
        "movz x30, #{guest_hcr_lo}",
        "movk x30, #{guest_hcr_hi16}, lsl #16",
        "movk x30, #{guest_hcr_hi32}, lsl #32",
        "msr hcr_el2, x30",
        "isb",
        "movz x30, #{guest_cnthctl_lo}",
        "msr cnthctl_el2, x30",
        "isb",
        "mrs x30, tpidr_el2",
        "eret",
        trapframe_ptr = sym GUEST_TRAPFRAME_PTR,
        guest_exit_vector_base = sym el2_guest_exit_vector_base,
        guest_hcr_lo = const HCR_EL2_GUEST_LO16,
        guest_hcr_hi16 = const HCR_EL2_GUEST_HI16,
        guest_hcr_hi32 = const HCR_EL2_GUEST_HI32,
        guest_cnthctl_lo = const CNTHCTL_EL2_GUEST_LO16,
        sys_vbar_el12 = const SYS_VBAR_EL12,
        sys_sctlr_el12 = const SYS_SCTLR_EL12,
        sys_tcr_el12 = const SYS_TCR_EL12,
        sys_ttbr0_el12 = const SYS_TTBR0_EL12,
        sys_ttbr1_el12 = const SYS_TTBR1_EL12,
        sys_mair_el12 = const SYS_MAIR_EL12,
        sys_amair_el12 = const SYS_AMAIR_EL12,
        sys_elr_el12 = const SYS_ELR_EL12,
        sys_spsr_el12 = const SYS_SPSR_EL12,
        sys_esr_el12 = const SYS_ESR_EL12,
        sys_far_el12 = const SYS_FAR_EL12,
        sys_cpacr_el12 = const SYS_CPACR_EL12,
        sys_contextidr_el12 = const SYS_CONTEXTIDR_EL12,
        sys_cntkctl_el12 = const SYS_CNTKCTL_EL12,
    );
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn arch_guest_trap_exit() {
    naked_asm!(
        // Capture guest EL1 sysreg snapshot while HCR_EL2.VM is still set.
        // _EL12 registers become UNDEF once TGE=1 is restored, so this call
        // must happen before the host HCR_EL2 write below.
        // Save x30 (link register) so the bl does not clobber it; x0-x18 are
        // caller-saved and are not needed after this point in the exit path.
        "mrs x8, hcr_el2",
        "tbz x8, #0, 8f",
        "movz x0, #{guest_capture_hcr_lo}",
        "movk x0, #{guest_capture_hcr_hi16}, lsl #16",
        "movk x0, #{guest_capture_hcr_hi32}, lsl #32",
        "msr hcr_el2, x0",
        "isb",
        "str x30, [sp, #-16]!",
        "bl {capture_snapshot}",
        "ldr x30, [sp], #16",
        "8:",
        // Restore host hypervisor context registers.
        "adrp x0, {host_ctx}",
        "add x0, x0, #:lo12:{host_ctx}",
        "ldr x1, [x0, #0]",
        "ldr x2, [x0, #8]",
        "ldr x3, [x0, #16]",
        "ldr x6, [x0, #40]",
        "ldr x7, [x0, #48]",
        "ldr x8, [x0, #56]",
        "ldr x9, [x0, #64]",
        "ldr x10, [x0, #72]",
        "ldr x11, [x0, #80]",
        "ldr x12, [x0, #88]",
        "ldr x13, [x0, #96]",
        "ldr x14, [x0, #104]",
        "ldr x15, [x0, #112]",
        "msr hcr_el2, x1",
        "msr vbar_el2, x2",
        "msr vbar_el1, x3",
        "msr tpidr_el1, x6",
        "msr cnthctl_el2, x8",
        "msr cntvoff_el2, x11",
        "msr cntv_cval_el0, x10",
        "msr cntv_ctl_el0, x9",
        "msr sp_el0, x12",
        "msr tpidr_el0, x13",
        "msr tpidrro_el0, x14",
        "msr cntkctl_el1, x15",
        "msr vttbr_el2, xzr",
        "isb",
        "msr daif, x7",
        "add sp, sp, #32",
        "ldp x19, x20, [sp, #0]",
        "ldp x21, x22, [sp, #16]",
        "ldp x23, x24, [sp, #32]",
        "ldp x25, x26, [sp, #48]",
        "ldp x27, x28, [sp, #64]",
        "ldp x29, x30, [sp, #80]",
        "add sp, sp, #96",
        "ret",
        host_ctx = sym HOST_HV_CTX,
        capture_snapshot = sym capture_guest_sysregs,
        guest_capture_hcr_lo = const HCR_EL2_GUEST_LO16,
        guest_capture_hcr_hi16 = const HCR_EL2_GUEST_HI16,
        guest_capture_hcr_hi32 = const HCR_EL2_GUEST_HI32,
    );
}
