//! System-register access for Apple CPU-local FIQ sources.

use core::arch::asm;

const FAST_IPI_PENDING: u64 = 1;
const VM_TIMER_ENABLE_MASK: u64 = 0b11;
const VGIC_ENABLE: u64 = 1;
const MPIDR_AFFINITY_MASK: u64 = 0xff;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostTimerBank {
    Physical,
    Virtual,
}

const fn host_timer_bank(vhe: bool) -> HostTimerBank {
    if vhe {
        HostTimerBank::Physical
    } else {
        HostTimerBank::Virtual
    }
}

const fn fast_ipi_request(current_mpidr: u64, target_mpidr: u64) -> (bool, u64) {
    let current_cluster = (current_mpidr >> 8) & MPIDR_AFFINITY_MASK;
    let target_cluster = (target_mpidr >> 8) & MPIDR_AFFINITY_MASK;
    let target_cpu = target_mpidr & MPIDR_AFFINITY_MASK;
    (
        current_cluster == target_cluster,
        target_cpu | (target_cluster << 16),
    )
}

#[derive(Clone, Copy)]
pub(super) struct LocalState {
    pub(super) cntp_ctl: u64,
    pub(super) cntv_ctl: u64,
    pub(super) fast_ipi_status: u64,
    pub(super) pmcr0: u64,
    pub(super) upmcr0: u64,
    pub(super) upmsr: u64,
    pub(super) isr: u64,
    pub(super) daif: u64,
    pub(super) elr: u64,
    pub(super) spsr: u64,
}

#[derive(Clone, Copy, Default)]
pub(super) struct El2State {
    pub(super) guest_cntp_ctl: u64,
    pub(super) guest_cntv_ctl: u64,
    pub(super) vm_timer_enable: u64,
    pub(super) hcr: u64,
    pub(super) cnthctl: u64,
    pub(super) vgic_hcr: u64,
    pub(super) vgic_misr: u64,
}

pub(super) fn read_local(vhe: bool) -> LocalState {
    let (cntp_ctl, cntv_ctl) = read_timer_state(vhe);
    let fast_ipi_status = read_fast_ipi_status();
    let (pmcr0, upmcr0, upmsr) = read_pmu_state();
    let (isr, daif, elr, spsr) = read_exception_state();

    LocalState {
        cntp_ctl,
        cntv_ctl,
        fast_ipi_status,
        pmcr0,
        upmcr0,
        upmsr,
        isr,
        daif,
        elr,
        spsr,
    }
}

pub(super) fn read_local_traced(cpu_id: u32, vhe: bool) -> LocalState {
    scarlet::early_println!(
        "[AIC] CPU {}: reading {} host timer state",
        cpu_id,
        if vhe { "physical" } else { "virtual" }
    );
    let (cntp_ctl, cntv_ctl) = read_timer_state(vhe);
    scarlet::early_println!(
        "[AIC] CPU {}: {} host timer state read",
        cpu_id,
        if vhe { "physical" } else { "virtual" }
    );

    scarlet::early_println!("[AIC] CPU {}: reading fast IPI state", cpu_id);
    let fast_ipi_status = read_fast_ipi_status();
    scarlet::early_println!("[AIC] CPU {}: fast IPI state read", cpu_id);

    scarlet::early_println!("[AIC] CPU {}: reading PMU FIQ state", cpu_id);
    let (pmcr0, upmcr0, upmsr) = read_pmu_state();
    scarlet::early_println!("[AIC] CPU {}: PMU FIQ state read", cpu_id);

    scarlet::early_println!("[AIC] CPU {}: reading exception state", cpu_id);
    let (isr, daif, elr, spsr) = read_exception_state();
    scarlet::early_println!("[AIC] CPU {}: exception state read", cpu_id);

    LocalState {
        cntp_ctl,
        cntv_ctl,
        fast_ipi_status,
        pmcr0,
        upmcr0,
        upmsr,
        isr,
        daif,
        elr,
        spsr,
    }
}

fn read_timer_state(vhe: bool) -> (u64, u64) {
    if host_timer_bank(vhe) == HostTimerBank::Physical {
        let cntp_ctl: u64;
        // SAFETY: Scarlet's VHE host timer uses the physical timer bank.
        unsafe {
            asm!(
                "mrs {cntp_ctl}, cntp_ctl_el0",
                cntp_ctl = out(reg) cntp_ctl,
                options(nomem, nostack, preserves_flags)
            );
        }
        (cntp_ctl, 0)
    } else {
        let cntv_ctl: u64;
        // SAFETY: Scarlet's EL1 timer uses the virtual timer bank, which is
        // directly accessible from privileged EL1.
        unsafe {
            asm!(
                "mrs {cntv_ctl}, cntv_ctl_el0",
                cntv_ctl = out(reg) cntv_ctl,
                options(nomem, nostack, preserves_flags)
            );
        }
        (0, cntv_ctl)
    }
}

fn read_fast_ipi_status() -> u64 {
    let value: u64;
    // SAFETY: The AIC driver is instantiated only for supported Apple SoCs.
    unsafe {
        asm!(
            "mrs {value}, S3_5_C15_C1_1",
            value = out(reg) value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}

fn read_pmu_state() -> (u64, u64, u64) {
    let pmcr0: u64;
    let upmcr0: u64;
    let upmsr: u64;
    // SAFETY: The AIC driver is instantiated only for supported Apple SoCs.
    unsafe {
        asm!(
            "mrs {pmcr0}, S3_1_C15_C0_0",
            "mrs {upmcr0}, S3_7_C15_C0_4",
            "mrs {upmsr}, S3_7_C15_C6_4",
            pmcr0 = out(reg) pmcr0,
            upmcr0 = out(reg) upmcr0,
            upmsr = out(reg) upmsr,
            options(nomem, nostack, preserves_flags)
        );
    }
    (pmcr0, upmcr0, upmsr)
}

fn read_exception_state() -> (u64, u64, u64, u64) {
    let isr: u64;
    let daif: u64;
    let elr: u64;
    let spsr: u64;
    // SAFETY: These architectural exception registers are readable from the
    // privileged AArch64 kernel context.
    unsafe {
        asm!(
            "mrs {isr}, isr_el1",
            "mrs {daif}, daif",
            "mrs {elr}, elr_el1",
            "mrs {spsr}, spsr_el1",
            isr = out(reg) isr,
            daif = out(reg) daif,
            elr = out(reg) elr,
            spsr = out(reg) spsr,
            options(nomem, nostack, preserves_flags)
        );
    }
    (isr, daif, elr, spsr)
}

pub(super) fn read_el2() -> El2State {
    let guest_cntp_ctl: u64;
    let guest_cntv_ctl: u64;
    let vm_timer_enable: u64;
    let hcr: u64;
    let cnthctl: u64;
    let vgic_hcr: u64;
    let vgic_misr: u64;

    // SAFETY: the caller checks VHE before accessing the EL2 and guest timer banks.
    unsafe {
        asm!(
            "mrs {guest_cntp_ctl}, S3_5_C14_C2_1",
            "mrs {guest_cntv_ctl}, S3_5_C14_C3_1",
            "mrs {vm_timer_enable}, S3_5_C15_C1_3",
            "mrs {hcr}, hcr_el2",
            "mrs {cnthctl}, cnthctl_el2",
            "mrs {vgic_hcr}, ich_hcr_el2",
            "mrs {vgic_misr}, ich_misr_el2",
            guest_cntp_ctl = out(reg) guest_cntp_ctl,
            guest_cntv_ctl = out(reg) guest_cntv_ctl,
            vm_timer_enable = out(reg) vm_timer_enable,
            hcr = out(reg) hcr,
            cnthctl = out(reg) cnthctl,
            vgic_hcr = out(reg) vgic_hcr,
            vgic_misr = out(reg) vgic_misr,
            options(nomem, nostack, preserves_flags)
        );
    }

    El2State {
        guest_cntp_ctl,
        guest_cntv_ctl,
        vm_timer_enable,
        hcr,
        cnthctl,
        vgic_hcr,
        vgic_misr,
    }
}

pub(super) fn disable_vgic_maintenance(control: u64) {
    // SAFETY: the caller checks VHE before accessing the EL2 virtual GIC
    // interface, and clearing En only disables inherited maintenance IRQs.
    unsafe {
        asm!(
            "msr ich_hcr_el2, {control}",
            "isb",
            control = in(reg) control & !VGIC_ENABLE,
            options(nomem, nostack, preserves_flags)
        );
    }
}

pub(super) fn mask_physical_timer(control: u64) {
    // SAFETY: the control value preserves all bits and sets the architectural IMASK bit.
    unsafe {
        asm!(
            "msr cntp_ctl_el0, {control}",
            control = in(reg) control,
            options(nomem, nostack, preserves_flags)
        );
    }
}

pub(super) fn mask_virtual_timer(control: u64) {
    // SAFETY: the control value preserves all bits and sets the architectural IMASK bit.
    unsafe {
        asm!(
            "msr cntv_ctl_el0, {control}",
            control = in(reg) control,
            options(nomem, nostack, preserves_flags)
        );
    }
}

pub(super) fn mask_guest_timers(physical: u64, virtual_: u64) {
    // SAFETY: the caller checks VHE and the Apple project does not enable guest ownership.
    unsafe {
        asm!(
            "msr S3_5_C14_C2_1, {physical}",
            "msr S3_5_C14_C3_1, {virtual}",
            "mrs {scratch}, S3_5_C15_C1_3",
            "bic {scratch}, {scratch}, {enable_mask}",
            "msr S3_5_C15_C1_3, {scratch}",
            physical = in(reg) physical,
            virtual = in(reg) virtual_,
            enable_mask = in(reg) VM_TIMER_ENABLE_MASK,
            scratch = out(reg) _,
            options(nomem, nostack, preserves_flags)
        );
    }
}

pub(super) fn clear_fast_ipi() {
    // SAFETY: bit zero of the Apple IPI status register is write-one-to-clear.
    unsafe {
        asm!(
            "msr S3_5_C15_C1_1, {pending}",
            pending = in(reg) FAST_IPI_PENDING,
            options(nomem, nostack, preserves_flags)
        );
    }
}

pub(super) fn read_mpidr() -> u64 {
    let mpidr: u64;
    // SAFETY: MPIDR_EL1 is readable from the privileged AArch64 kernel context.
    unsafe {
        asm!(
            "mrs {mpidr}, mpidr_el1",
            mpidr = out(reg) mpidr,
            options(nomem, nostack, preserves_flags)
        );
    }
    mpidr
}

pub(super) fn send_fast_ipi(current_mpidr: u64, target_mpidr: u64) {
    let (local, request) = fast_ipi_request(current_mpidr, target_mpidr);

    // SAFETY: These implementation-defined registers are used only by the
    // Apple AIC driver on SoCs advertising the t8103 fast-IPI interface.
    unsafe {
        if local {
            asm!(
                "msr S3_5_C15_C0_0, {request}",
                request = in(reg) request & MPIDR_AFFINITY_MASK,
                options(nomem, nostack, preserves_flags)
            );
        } else {
            asm!(
                "msr S3_5_C15_C0_1, {request}",
                request = in(reg) request,
                options(nomem, nostack, preserves_flags)
            );
        }
        asm!("isb", options(nomem, nostack, preserves_flags));
    }
}

pub(super) fn write_pmcr0(value: u64) {
    // SAFETY: the caller only clears the Apple PMU interrupt mode and active bits.
    unsafe {
        asm!(
            "msr S3_1_C15_C0_0, {value}",
            value = in(reg) value,
            options(nomem, nostack, preserves_flags)
        );
    }
}

pub(super) fn write_upmcr0(value: u64) {
    // SAFETY: the caller only clears the t8103-family uncore-PMU interrupt mode bits.
    unsafe {
        asm!(
            "msr S3_7_C15_C0_4, {value}",
            value = in(reg) value,
            options(nomem, nostack, preserves_flags)
        );
    }
}

pub(super) fn synchronize() {
    // SAFETY: ISB only synchronizes the preceding CPU-local system-register writes.
    unsafe {
        asm!("isb", options(nomem, nostack, preserves_flags));
    }
}

#[cfg(test)]
mod tests {
    use super::{HostTimerBank, fast_ipi_request, host_timer_bank};

    #[test_case]
    fn host_timer_bank_matches_kernel_execution_level() {
        assert_eq!(host_timer_bank(true), HostTimerBank::Physical);
        assert_eq!(host_timer_bank(false), HostTimerBank::Virtual);
    }

    #[test_case]
    fn fast_ipi_request_uses_local_register_within_cluster() {
        let (local, request) = fast_ipi_request(0x8000_0100, 0x8000_0103);

        assert!(local);
        assert_eq!(request & 0xff, 3);
    }

    #[test_case]
    fn fast_ipi_request_encodes_remote_cluster() {
        let (local, request) = fast_ipi_request(0x8000_0002, 0x8000_0101);

        assert!(!local);
        assert_eq!(request, 0x0001_0001);
    }
}
