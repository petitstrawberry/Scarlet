//! PCI INTx interrupt source support.

extern crate alloc;

use alloc::sync::Arc;

use super::PciAddress;
use super::config::{self, PciConfig};
use super::device::PciDeviceInfo;
use crate::device::events::InterruptCapableDevice;
use crate::interrupt::{
    DeferredInterruptCompletion, InterruptClaim, InterruptId, InterruptResult, InterruptSource,
    MaskableInterruptSource,
};

/// Maskable interrupt source for a PCI legacy INTx function.
pub struct PciIntxInterruptSource {
    config: PciConfig,
    address: PciAddress,
    interrupt_id: InterruptId,
    handler: Arc<dyn InterruptCapableDevice>,
}

impl PciIntxInterruptSource {
    /// Create a PCI INTx interrupt source for a device.
    ///
    /// # Arguments
    ///
    /// * `device` - PCI function that provides the legacy INTx line.
    /// * `handler` - Device interrupt handler for this function.
    ///
    /// # Returns
    ///
    /// A maskable INTx source, or `None` when the function has no usable legacy
    /// interrupt routing.
    pub fn new(device: &PciDeviceInfo, handler: Arc<dyn InterruptCapableDevice>) -> Option<Self> {
        let interrupt_id = Self::interrupt_id_for_device(device)?;
        Some(Self {
            config: PciConfig::new(device.ecam_vaddr()),
            address: device.address(),
            interrupt_id,
            handler,
        })
    }

    /// Resolve the legacy INTx virtual IRQ for a PCI function.
    ///
    /// # Arguments
    ///
    /// * `device` - PCI function whose routing should be resolved.
    ///
    /// # Returns
    ///
    /// The routed virtual IRQ, or `None` when INTx is not usable.
    pub fn interrupt_id_for_device(device: &PciDeviceInfo) -> Option<InterruptId> {
        if device.interrupt_pin() == 0 {
            return None;
        }

        device.routed_irq().or_else(|| {
            let line = device.interrupt_line();
            (line != 0 && line != 0xff).then_some(line as InterruptId)
        })
    }

    fn set_masked(&self, masked: bool) {
        let command = self.config.read_u16(&self.address, config::offset::COMMAND);
        let next = if masked {
            command | config::command::INTERRUPT_DISABLE
        } else {
            command & !config::command::INTERRUPT_DISABLE
        };

        if next != command {
            self.config
                .write_u16(&self.address, config::offset::COMMAND, next);
        }
    }
}

impl InterruptSource for PciIntxInterruptSource {
    fn interrupt_id(&self) -> Option<InterruptId> {
        Some(self.interrupt_id)
    }

    fn claim_interrupt(&self) -> InterruptResult<InterruptClaim> {
        self.handler.claim_interrupt()
    }

    fn deferred_interrupt_ready(&self, completion: DeferredInterruptCompletion) {
        self.handler.deferred_interrupt_ready(completion);
    }
}

impl MaskableInterruptSource for PciIntxInterruptSource {
    fn mask_source(&self) -> InterruptResult<()> {
        self.set_masked(true);
        Ok(())
    }

    fn unmask_source(&self) -> InterruptResult<()> {
        self.set_masked(false);
        Ok(())
    }

    fn clear_pending_source(&self) -> InterruptResult<()> {
        let _ = self.handler.claim_interrupt()?;
        Ok(())
    }
}
