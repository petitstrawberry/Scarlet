use core::arch::asm;

const SYS_VBAR_EL12: u32 = (3 << 14) | (5 << 11) | (12 << 7) | (0 << 3) | 0;
const SYS_SCTLR_EL12: u32 = (3 << 14) | (5 << 11) | (1 << 7) | (0 << 3) | 0;
const SYS_TCR_EL12: u32 = (3 << 14) | (5 << 11) | (2 << 7) | (0 << 3) | 2;
const SYS_TTBR0_EL12: u32 = (3 << 14) | (5 << 11) | (2 << 7) | (0 << 3) | 0;
const SYS_TTBR1_EL12: u32 = (3 << 14) | (5 << 11) | (2 << 7) | (0 << 3) | 1;
const SYS_MAIR_EL12: u32 = (3 << 14) | (5 << 11) | (10 << 7) | (2 << 3) | 0;
const SYS_AMAIR_EL12: u32 = (3 << 14) | (5 << 11) | (10 << 7) | (3 << 3) | 0;
const SYS_SP_EL1: u32 = (3 << 14) | (4 << 11) | (1 << 7) | (0 << 3) | 0;
const SYS_ELR_EL12: u32 = (3 << 14) | (5 << 11) | (4 << 7) | (0 << 3) | 1;
const SYS_SPSR_EL12: u32 = (3 << 14) | (5 << 11) | (4 << 7) | (0 << 3) | 0;
const SYS_ESR_EL12: u32 = (3 << 14) | (5 << 11) | (5 << 7) | (0 << 3) | 0;
const SYS_FAR_EL12: u32 = (3 << 14) | (5 << 11) | (6 << 7) | (0 << 3) | 0;
const SYS_CPACR_EL12: u32 = (3 << 14) | (5 << 11) | (1 << 7) | (0 << 3) | 2;
const SYS_CONTEXTIDR_EL12: u32 = (3 << 14) | (5 << 11) | (13 << 7) | (0 << 3) | 1;

#[inline(always)]
fn read_sysreg<const SYSREG: u32>() -> u64 {
    let value: u64;

    // SAFETY: the caller selects a valid system register encoding accessible at
    // EL2 for VHE guest context save/restore.
    unsafe {
        asm!(
            ".inst 0xd5200000 | ({sysreg} << 5) | {rt}",
            sysreg = const SYSREG,
            rt = const 0,
            lateout("x0") value,
            options(nostack),
        );
    }

    value
}

#[inline(always)]
fn write_sysreg<const SYSREG: u32>(value: u64) {
    // SAFETY: the caller selects a valid system register encoding accessible at
    // EL2 for VHE guest context save/restore.
    unsafe {
        asm!(
            ".inst 0xd5000000 | ({sysreg} << 5) | {rt}",
            sysreg = const SYSREG,
            rt = const 0,
            in("x0") value,
            options(nostack),
        );
    }
}

#[derive(Debug, Clone, Default)]
pub struct GuestSystemRegs {
    pub vbar_el1: u64,
    pub sctlr_el1: u64,
    pub tcr_el1: u64,
    pub ttbr0_el1: u64,
    pub ttbr1_el1: u64,
    pub mair_el1: u64,
    pub amair_el1: u64,
    pub sp_el1: u64,
    pub elr_el1: u64,
    pub spsr_el1: u64,
    pub esr_el1: u64,
    pub far_el1: u64,
    pub cpacr_el1: u64,
    pub contextidr_el1: u64,
    pub cntv_ctl_el0: u64,
    pub cntv_cval_el0: u64,
    pub cntvoff_el2: u64,
}

impl GuestSystemRegs {
    pub fn save() -> Self {
        let cntv_ctl_el0: u64;
        let cntv_cval_el0: u64;
        let cntvoff_el2: u64;

        // SAFETY: guest timer state is read from architected timer registers
        // while executing at EL2.
        unsafe {
            asm!(
                "mrs {cntv_ctl_el0}, cntv_ctl_el0",
                "mrs {cntv_cval_el0}, cntv_cval_el0",
                "mrs {cntvoff_el2}, cntvoff_el2",
                cntv_ctl_el0 = out(reg) cntv_ctl_el0,
                cntv_cval_el0 = out(reg) cntv_cval_el0,
                cntvoff_el2 = out(reg) cntvoff_el2,
                options(nostack),
            );
        }

        Self {
            vbar_el1: read_sysreg::<{ SYS_VBAR_EL12 }>(),
            sctlr_el1: read_sysreg::<{ SYS_SCTLR_EL12 }>(),
            tcr_el1: read_sysreg::<{ SYS_TCR_EL12 }>(),
            ttbr0_el1: read_sysreg::<{ SYS_TTBR0_EL12 }>(),
            ttbr1_el1: read_sysreg::<{ SYS_TTBR1_EL12 }>(),
            mair_el1: read_sysreg::<{ SYS_MAIR_EL12 }>(),
            amair_el1: read_sysreg::<{ SYS_AMAIR_EL12 }>(),
            sp_el1: read_sysreg::<{ SYS_SP_EL1 }>(),
            elr_el1: read_sysreg::<{ SYS_ELR_EL12 }>(),
            spsr_el1: read_sysreg::<{ SYS_SPSR_EL12 }>(),
            esr_el1: read_sysreg::<{ SYS_ESR_EL12 }>(),
            far_el1: read_sysreg::<{ SYS_FAR_EL12 }>(),
            cpacr_el1: read_sysreg::<{ SYS_CPACR_EL12 }>(),
            contextidr_el1: read_sysreg::<{ SYS_CONTEXTIDR_EL12 }>(),
            cntv_ctl_el0,
            cntv_cval_el0,
            cntvoff_el2,
        }
    }

    pub fn restore(&self) {
        write_sysreg::<{ SYS_VBAR_EL12 }>(self.vbar_el1);
        write_sysreg::<{ SYS_SCTLR_EL12 }>(self.sctlr_el1);
        write_sysreg::<{ SYS_TCR_EL12 }>(self.tcr_el1);
        write_sysreg::<{ SYS_TTBR0_EL12 }>(self.ttbr0_el1);
        write_sysreg::<{ SYS_TTBR1_EL12 }>(self.ttbr1_el1);
        write_sysreg::<{ SYS_MAIR_EL12 }>(self.mair_el1);
        write_sysreg::<{ SYS_AMAIR_EL12 }>(self.amair_el1);
        write_sysreg::<{ SYS_SP_EL1 }>(self.sp_el1);
        write_sysreg::<{ SYS_ELR_EL12 }>(self.elr_el1);
        write_sysreg::<{ SYS_SPSR_EL12 }>(self.spsr_el1);
        write_sysreg::<{ SYS_ESR_EL12 }>(self.esr_el1);
        write_sysreg::<{ SYS_FAR_EL12 }>(self.far_el1);
        write_sysreg::<{ SYS_CPACR_EL12 }>(self.cpacr_el1);
        write_sysreg::<{ SYS_CONTEXTIDR_EL12 }>(self.contextidr_el1);

        // SAFETY: guest timer state is restored to architected timer registers
        // while executing at EL2.
        unsafe {
            asm!(
                "msr cntv_ctl_el0, {cntv_ctl_el0}",
                "msr cntv_cval_el0, {cntv_cval_el0}",
                "msr cntvoff_el2, {cntvoff_el2}",
                cntv_ctl_el0 = in(reg) self.cntv_ctl_el0,
                cntv_cval_el0 = in(reg) self.cntv_cval_el0,
                cntvoff_el2 = in(reg) self.cntvoff_el2,
                options(nostack),
            );
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HypervisorSystemRegs {
    pub hcr_el2: u64,
    pub vttbr_el2: u64,
    pub vtcr_el2: u64,
    pub sctlr_el2: u64,
    pub tcr_el2: u64,
    pub ttbr0_el2: u64,
    pub cptr_el2: u64,
    pub cnthctl_el2: u64,
}

impl HypervisorSystemRegs {
    pub fn save() -> Self {
        let hcr_el2: u64;
        let vttbr_el2: u64;
        let vtcr_el2: u64;
        let sctlr_el2: u64;
        let tcr_el2: u64;
        let ttbr0_el2: u64;
        let cptr_el2: u64;
        let cnthctl_el2: u64;

        // SAFETY: host EL2 register state is captured while the kernel is
        // executing at EL2 in VHE mode.
        unsafe {
            asm!(
                "mrs {hcr_el2}, hcr_el2",
                "mrs {vttbr_el2}, vttbr_el2",
                "mrs {vtcr_el2}, vtcr_el2",
                "mrs {sctlr_el2}, sctlr_el2",
                "mrs {tcr_el2}, tcr_el2",
                "mrs {ttbr0_el2}, ttbr0_el2",
                "mrs {cptr_el2}, cptr_el2",
                "mrs {cnthctl_el2}, cnthctl_el2",
                hcr_el2 = out(reg) hcr_el2,
                vttbr_el2 = out(reg) vttbr_el2,
                vtcr_el2 = out(reg) vtcr_el2,
                sctlr_el2 = out(reg) sctlr_el2,
                tcr_el2 = out(reg) tcr_el2,
                ttbr0_el2 = out(reg) ttbr0_el2,
                cptr_el2 = out(reg) cptr_el2,
                cnthctl_el2 = out(reg) cnthctl_el2,
                options(nostack),
            );
        }

        Self {
            hcr_el2,
            vttbr_el2,
            vtcr_el2,
            sctlr_el2,
            tcr_el2,
            ttbr0_el2,
            cptr_el2,
            cnthctl_el2,
        }
    }

    pub fn restore(&self) {
        // SAFETY: host EL2 register state is restored before resuming host EL2
        // execution in the hypervisor world-switch path.
        unsafe {
            asm!(
                "msr hcr_el2, {hcr_el2}",
                "msr vttbr_el2, {vttbr_el2}",
                "msr vtcr_el2, {vtcr_el2}",
                "msr sctlr_el2, {sctlr_el2}",
                "msr tcr_el2, {tcr_el2}",
                "msr ttbr0_el2, {ttbr0_el2}",
                "msr cptr_el2, {cptr_el2}",
                "msr cnthctl_el2, {cnthctl_el2}",
                "isb",
                hcr_el2 = in(reg) self.hcr_el2,
                vttbr_el2 = in(reg) self.vttbr_el2,
                vtcr_el2 = in(reg) self.vtcr_el2,
                sctlr_el2 = in(reg) self.sctlr_el2,
                tcr_el2 = in(reg) self.tcr_el2,
                ttbr0_el2 = in(reg) self.ttbr0_el2,
                cptr_el2 = in(reg) self.cptr_el2,
                cnthctl_el2 = in(reg) self.cnthctl_el2,
                options(nostack),
            );
        }
    }
}
