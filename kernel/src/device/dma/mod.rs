//! Generic DMA engine abstractions.
//!
//! This module provides provider-neutral DMA controller and channel traits used
//! by platform drivers. DMA controller drivers register providers by firmware
//! phandle, and client drivers resolve channels from Device Tree `dmas`
//! properties through `DeviceManager`.

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;

pub use crate::mem::address::DmaAddr;

/// Callback invoked after a DMA channel observes completed periods.
pub type DmaCompletionCallback = Arc<dyn Fn() + Send + Sync>;

/// Direction of a DMA transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaDirection {
    /// Copy from memory to a peripheral FIFO/register.
    MemToDev,
    /// Copy from a peripheral FIFO/register to memory.
    DevToMem,
    /// Copy from memory to memory.
    MemToMem,
}

/// Bus width used for peripheral DMA accesses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaBusWidth {
    /// 8-bit bus access.
    Width1,
    /// 16-bit bus access.
    Width2,
    /// 32-bit bus access.
    Width4,
    /// 64-bit bus access.
    Width8,
}

impl DmaBusWidth {
    /// Return the bus width in bytes.
    ///
    /// # Returns
    ///
    /// Number of bytes transferred by one peripheral bus access.
    pub fn bytes(self) -> usize {
        match self {
            Self::Width1 => 1,
            Self::Width2 => 2,
            Self::Width4 => 4,
            Self::Width8 => 8,
        }
    }
}

/// Peripheral-side DMA endpoint configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DmaPeripheralConfig {
    /// Peripheral FIFO or data register DMA address.
    pub addr: DmaAddr,
    /// Peripheral access width.
    pub width: DmaBusWidth,
    /// Number of bus-width beats per burst.
    pub burst_len: usize,
}

/// Cyclic DMA transfer configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DmaCyclicConfig {
    /// Device-visible address of the DMA ring buffer.
    pub buffer_addr: DmaAddr,
    /// Total ring buffer length in bytes.
    pub buffer_len: usize,
    /// Period length in bytes.
    pub period_len: usize,
    /// Transfer direction.
    pub direction: DmaDirection,
    /// Peripheral endpoint for memory/device transfers.
    pub peripheral: Option<DmaPeripheralConfig>,
}

impl DmaCyclicConfig {
    /// Validate the cyclic transfer geometry and endpoint fields.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the configuration is internally consistent.
    pub fn validate(&self) -> Result<(), DmaError> {
        if self.buffer_len == 0 || self.period_len == 0 {
            return Err(DmaError::InvalidConfig);
        }
        if self
            .buffer_addr
            .checked_add(self.buffer_len as u64 - 1)
            .is_none()
        {
            return Err(DmaError::InvalidConfig);
        }
        if !self.buffer_len.is_multiple_of(self.period_len) {
            return Err(DmaError::InvalidConfig);
        }
        if self.direction != DmaDirection::MemToMem && self.peripheral.is_none() {
            return Err(DmaError::InvalidConfig);
        }
        if let Some(peripheral) = self.peripheral
            && (peripheral.addr.is_zero()
                || peripheral.burst_len == 0
                || peripheral
                    .addr
                    .checked_add(peripheral.width.bytes() as u64 - 1)
                    .is_none())
        {
            return Err(DmaError::InvalidConfig);
        }
        Ok(())
    }
}

/// DMA operation errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaError {
    /// Firmware DMA specifier is malformed.
    InvalidSpec,
    /// Requested channel does not exist.
    ChannelNotFound,
    /// Requested channel is already owned.
    ChannelBusy,
    /// Transfer configuration is invalid.
    InvalidConfig,
    /// Operation is not supported by this controller.
    Unsupported,
    /// Hardware operation failed.
    HardwareError,
    /// Channel has not been prepared before an operation requiring preparation.
    NotPrepared,
}

/// Firmware DMA specifier resolved from a platform device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmaSpec {
    /// Firmware phandle identifying the DMA controller.
    pub controller_phandle: u32,
    /// Provider-specific specifier cells following the phandle.
    pub cells: Vec<u32>,
}

/// A single DMA channel.
pub trait DmaChannel: Send + Sync {
    /// Return the channel name.
    ///
    /// # Returns
    ///
    /// Static channel name used for diagnostics.
    fn name(&self) -> &'static str;

    /// Prepare a cyclic DMA transfer.
    ///
    /// # Arguments
    ///
    /// * `config` - Cyclic transfer geometry and endpoint configuration.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the channel accepted the configuration.
    fn prepare_cyclic(&self, config: DmaCyclicConfig) -> Result<(), DmaError>;

    /// Start the prepared transfer.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the transfer is running.
    fn start(&self) -> Result<(), DmaError>;

    /// Stop the active transfer.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the channel is stopped.
    fn stop(&self) -> Result<(), DmaError>;

    /// Pause the active transfer.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the channel is paused.
    fn pause(&self) -> Result<(), DmaError> {
        Err(DmaError::Unsupported)
    }

    /// Resume a paused transfer.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the channel resumed.
    fn resume(&self) -> Result<(), DmaError> {
        Err(DmaError::Unsupported)
    }

    /// Return bytes remaining until the next period boundary when supported.
    ///
    /// # Returns
    ///
    /// Remaining byte count, or an error if the controller cannot report it.
    fn residue(&self) -> Result<usize, DmaError> {
        Err(DmaError::Unsupported)
    }

    /// Return and clear the number of completed cyclic periods.
    ///
    /// # Returns
    ///
    /// Number of periods completed since the last call.
    fn take_completed_periods(&self) -> usize {
        0
    }

    /// Queue one period from a prepared cyclic buffer.
    ///
    /// This is used by clients that own the producer side of the cyclic buffer
    /// and must only expose committed periods to hardware.
    ///
    /// # Arguments
    ///
    /// * `byte_offset` - Byte offset of the period within the prepared cyclic buffer.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the descriptor was queued.
    fn queue_cyclic_period(&self, byte_offset: usize) -> Result<(), DmaError> {
        let _ = byte_offset;
        Err(DmaError::Unsupported)
    }

    /// Install a completion callback for cyclic transfers.
    ///
    /// # Arguments
    ///
    /// * `callback` - Callback to invoke when the channel observes completed periods,
    ///   or `None` to clear the callback.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the callback state was updated.
    fn set_completion_callback(
        &self,
        callback: Option<DmaCompletionCallback>,
    ) -> Result<(), DmaError> {
        let _ = callback;
        Ok(())
    }

    /// Report whether the channel is currently running.
    ///
    /// # Returns
    ///
    /// `true` when a transfer is active.
    fn is_running(&self) -> bool;
}

/// DMA controller exposed by firmware phandle.
pub trait DmaController: Send + Sync {
    /// Return the provider name.
    ///
    /// # Returns
    ///
    /// Static provider name used for diagnostics.
    fn name(&self) -> &'static str;

    /// Return the number of cells after the provider phandle.
    ///
    /// # Returns
    ///
    /// Number of `u32` cells required to identify a DMA channel.
    fn dma_cells(&self) -> usize;

    /// Request a DMA channel from this controller.
    ///
    /// # Arguments
    ///
    /// * `spec` - Firmware DMA specifier for this controller.
    ///
    /// # Returns
    ///
    /// A DMA channel handle on success.
    fn request_channel(&self, spec: &DmaSpec) -> Result<Arc<dyn DmaChannel>, DmaError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn cyclic_dma_validates_wide_buffer_and_peripheral_ranges() {
        let mut config = DmaCyclicConfig {
            buffer_addr: DmaAddr::new(0x1_8000_0000),
            buffer_len: 0x1000,
            period_len: 0x800,
            direction: DmaDirection::MemToDev,
            peripheral: Some(DmaPeripheralConfig {
                addr: DmaAddr::new(0x2_0000_0000),
                width: DmaBusWidth::Width4,
                burst_len: 1,
            }),
        };
        assert_eq!(config.validate(), Ok(()));
        config.buffer_addr = DmaAddr::new(u64::MAX - 0xffe);
        assert_eq!(config.validate(), Err(DmaError::InvalidConfig));
        config.buffer_addr = DmaAddr::new(0x1_8000_0000);
        config.peripheral.as_mut().unwrap().addr = DmaAddr::new(u64::MAX);
        assert_eq!(config.validate(), Err(DmaError::InvalidConfig));
    }
}
