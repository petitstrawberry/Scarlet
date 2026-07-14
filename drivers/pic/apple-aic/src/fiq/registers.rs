//! System-register access for Apple CPU-local FIQ sources.

use core::arch::asm;

const FAST_IPI_PENDING: u64 = 1;
const VM_TIMER_ENABLE_MASK: u64 = 0b11;
const VGIC_ENABLE: u64 = 1;
const MPIDR_AFFINITY_MASK: u64 = 0xff;

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

pub(super) fn read_local() -> LocalState {
    let cntp_ctl: u64;
    let cntv_ctl: u64;
    let fast_ipi_status: u64;
    let pmcr0: u64;
    let upmcr0: u64;
    let upmsr: u64;
    let isr: u64;
    let daif: u64;
    let elr: u64;
    let spsr: u64;

    // SAFETY: AIC initialization restricts these CPU-local reads to supported Apple SoCs.
    unsafe {
        asm!(
            "mrs {cntp_ctl}, cntp_ctl_el0",
            "mrs {cntv_ctl}, cntv_ctl_el0",
            "mrs {fast_ipi_status}, S3_5_C15_C1_1",
            "mrs {pmcr0}, S3_1_C15_C0_0",
            "mrs {upmcr0}, S3_7_C15_C0_4",
            "mrs {upmsr}, S3_7_C15_C6_4",
            "mrs {isr}, isr_el1",
            "mrs {daif}, daif",
            "mrs {elr}, elr_el1",
            "mrs {spsr}, spsr_el1",
            cntp_ctl = out(reg) cntp_ctl,
            cntv_ctl = out(reg) cntv_ctl,
            fast_ipi_status = out(reg) fast_ipi_status,
            pmcr0 = out(reg) pmcr0,
            upmcr0 = out(reg) upmcr0,
            upmsr = out(reg) upmsr,
            isr = out(reg) isr,
            daif = out(reg) daif,
            elr = out(reg) elr,
            spsr = out(reg) spsr,
            options(nomem, nostack, preserves_flags)
        );
    }

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
    use super::fast_ipi_request;

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
