//! IRQ injection for RISC-V H-extension hypervisor
//!
//! Provides APIs to inject virtual interrupts into guest VMs via the HVIP CSR.

use super::csr;

pub const IRQ_VS_SOFT: u64 = 2;
pub const IRQ_VS_TIMER: u64 = 6;
pub const IRQ_VS_EXT: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestIrq {
    Software,
    Timer,
    External,
}

impl GuestIrq {
    pub fn bit(&self) -> u64 {
        match self {
            GuestIrq::Software => 1 << IRQ_VS_SOFT,
            GuestIrq::Timer => 1 << IRQ_VS_TIMER,
            GuestIrq::External => 1 << IRQ_VS_EXT,
        }
    }
}

#[inline]
pub fn inject_irq(irq: GuestIrq) {
    let hvip = csr::read_hvip();
    csr::write_hvip(hvip | irq.bit());
}

#[inline]
pub fn clear_irq(irq: GuestIrq) {
    let hvip = csr::read_hvip();
    csr::write_hvip(hvip & !irq.bit());
}

#[inline]
pub fn inject_ext() {
    inject_irq(GuestIrq::External);
}

#[inline]
pub fn inject_timer() {
    inject_irq(GuestIrq::Timer);
}

#[inline]
pub fn inject_soft() {
    inject_irq(GuestIrq::Software);
}

#[inline]
pub fn clear_ext() {
    clear_irq(GuestIrq::External);
}

#[inline]
pub fn clear_timer() {
    clear_irq(GuestIrq::Timer);
}

#[inline]
pub fn clear_soft() {
    clear_irq(GuestIrq::Software);
}

#[inline]
pub fn get_pending_irqs() -> u64 {
    csr::read_hvip() & ((1 << IRQ_VS_SOFT) | (1 << IRQ_VS_TIMER) | (1 << IRQ_VS_EXT))
}

#[inline]
pub fn has_pending(irq: GuestIrq) -> bool {
    (csr::read_hvip() & irq.bit()) != 0
}
