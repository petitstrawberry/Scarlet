use core::arch::asm;
use core::cell::UnsafeCell;

use super::switch::HCR_EL2_VM;

// Pre-shifted system register encoding constants for use with
// read_sysreg / write_sysreg (`.inst`-based accessors).
//
// Verified against Linux arch/arm64/include/asm/sysreg.h:
//   SYS_ESR_EL12    = sys_reg(3, 5, 5, 2, 0)   ← was CRm=0
//   SYS_SP_EL1      = sys_reg(3, 4, 4, 1, 0)   ← was op1=0
//   SYS_SCTLR_EL12  = sys_reg(3, 5, 1, 0, 0)   ← was op2=2
const SYS_VBAR_EL12: u32 = sys_reg(3, 5, 12, 0, 0);
const SYS_SCTLR_EL12: u32 = sys_reg(3, 5, 1, 0, 0);
const SYS_TCR_EL12: u32 = sys_reg(3, 5, 2, 0, 2);
const SYS_TTBR0_EL12: u32 = sys_reg(3, 5, 2, 0, 0);
const SYS_TTBR1_EL12: u32 = sys_reg(3, 5, 2, 0, 1);
const SYS_MAIR_EL12: u32 = sys_reg(3, 5, 10, 2, 0);
const SYS_AMAIR_EL12: u32 = sys_reg(3, 5, 10, 3, 0);
const SYS_SP_EL1: u32 = sys_reg(3, 4, 4, 1, 0);
const SYS_ELR_EL12: u32 = sys_reg(3, 5, 4, 0, 1);
const SYS_SPSR_EL12: u32 = sys_reg(3, 5, 4, 0, 0);
const SYS_ESR_EL12: u32 = sys_reg(3, 5, 5, 2, 0);
const SYS_FAR_EL12: u32 = sys_reg(3, 5, 6, 0, 0);
const SYS_CPACR_EL12: u32 = sys_reg(3, 5, 1, 0, 2);
const SYS_CONTEXTIDR_EL12: u32 = sys_reg(3, 5, 13, 0, 1);

/// Build a system register encoding in the pre-shifted format used by
/// read_sysreg / write_sysreg.  `value << 5` yields the instruction bits
/// [20:5] per ARM DDI 0487 §C5.2.
const fn sys_reg(op0: u32, op1: u32, crn: u32, crm: u32, op2: u32) -> u32 {
    (op0 << 14) | (op1 << 11) | (crn << 7) | (crm << 3) | op2
}

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

/// Per-CPU slot that holds a `GuestSystemRegs` snapshot captured while the
/// CPU is still executing in guest context (before `HCR_EL2.TGE` is
/// re-asserted).  The slot is written by [`capture_guest_sysregs`] and
/// consumed (taken) by [`GuestSystemRegs::save`].
///
/// Using a plain `static` with `UnsafeCell` is intentional: the hypervisor
/// world-switch path is single-threaded per CPU and the capture/consume
/// sequence is always paired within a single guest-exit handling window.
struct PendingSnapshot(UnsafeCell<Option<GuestSystemRegs>>);

// SAFETY: The snapshot slot is only accessed from the CPU that owns the
// current guest-exit context.  No concurrent access is possible because
// the hypervisor world-switch path runs with interrupts disabled.
unsafe impl Sync for PendingSnapshot {}

static PENDING_GUEST_SYSREGS: PendingSnapshot = PendingSnapshot(UnsafeCell::new(None));

#[inline(always)]
fn guest_context_active() -> bool {
    let hcr_el2: u64;

    // SAFETY: reading HCR_EL2 at EL2 is side-effect free.
    unsafe {
        asm!("mrs {hcr_el2}, hcr_el2", hcr_el2 = out(reg) hcr_el2, options(nostack));
    }

    (hcr_el2 & HCR_EL2_VM) != 0
}

#[inline(always)]
fn read_guest_sysregs() -> GuestSystemRegs {
    let cntv_ctl_el0: u64;
    let cntv_cval_el0: u64;
    let cntvoff_el2: u64;

    // SAFETY: guest timer state is read from architected timer registers
    // while executing at EL2 in guest context (HCR_EL2.VM=1, TGE=0).
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

    GuestSystemRegs {
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

/// Capture all EL12 guest system registers into a per-CPU snapshot.
///
/// **Must be called while the CPU is still in guest context** (i.e. before
/// `HCR_EL2.TGE` is set and before `HCR_EL2.VM` is cleared).  The captured
/// values are later consumed by [`GuestSystemRegs::save`].
/// # Safety
/// Must be called only while guest context is still active.
pub unsafe extern "C" fn capture_guest_sysregs() {
    let snapshot = read_guest_sysregs();

    // SAFETY: we are the sole writer; the slot is consumed before the next
    // guest entry so there is no aliasing with a concurrent reader.
    unsafe {
        *PENDING_GUEST_SYSREGS.0.get() = Some(snapshot);
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
    /// Consume the snapshot previously deposited by [`capture_guest_sysregs`].
    /// If no snapshot is pending, fall back to a live read only while guest
    /// context is still active. Returns `Default` when neither source exists.
    pub fn save() -> Self {
        // SAFETY: we are the sole reader; `capture_guest_sysregs` is always
        // called before this function in the guest-exit path, and the slot is
        // cleared here so it cannot be double-consumed.
        unsafe {
            (*PENDING_GUEST_SYSREGS.0.get()).take().unwrap_or_else(|| {
                if guest_context_active() {
                    read_guest_sysregs()
                } else {
                    Self::default()
                }
            })
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
