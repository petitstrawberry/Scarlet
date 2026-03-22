//! SPI adapter wrapper for shared bus access.

extern crate alloc;

use alloc::sync::Arc;
use spin::Mutex;

use crate::device::spi::{SpiBus, SpiError, SpiTransfer, SpiTransferFlags};

pub struct SpiAdapter {
    bus: Arc<dyn SpiBus>,
    bus_lock: Mutex<()>,
}

impl SpiAdapter {
    pub fn new(bus: Arc<dyn SpiBus>) -> Self {
        Self {
            bus,
            bus_lock: Mutex::new(()),
        }
    }

    pub fn bus_number(&self) -> u32 {
        self.bus.bus_number()
    }

    pub fn bus_speed(&self) -> u32 {
        self.bus.bus_speed()
    }

    pub fn set_bus_speed(&self, hz: u32) -> Result<(), SpiError> {
        let _guard = self.bus_lock.lock();
        self.bus.set_bus_speed(hz)
    }

    pub fn transfer(&self, segments: &mut [SpiTransfer]) -> Result<(), SpiError> {
        let _guard = self.bus_lock.lock();
        self.bus.transfer(segments)
    }

    pub fn write(&self, cs: u8, data: &[u8]) -> Result<(), SpiError> {
        let mut seg = [SpiTransfer::write(cs, data)];
        self.transfer(&mut seg)
    }

    pub fn read(&self, cs: u8, buf: &mut [u8]) -> Result<(), SpiError> {
        let mut seg = [SpiTransfer::read(cs, buf.len())];
        self.transfer(&mut seg)?;
        buf.copy_from_slice(&seg[0].data);
        Ok(())
    }

    pub fn write_then_read(&self, cs: u8, write: &[u8], read: &mut [u8]) -> Result<(), SpiError> {
        let mut segments = [
            SpiTransfer::write(cs, write),
            SpiTransfer::read(cs, read.len()),
        ];
        self.transfer(&mut segments)?;
        read.copy_from_slice(&segments[1].data);
        Ok(())
    }

    pub fn full_duplex(&self, cs: u8, tx: &[u8], rx: &mut [u8]) -> Result<(), SpiError> {
        let mut seg = [SpiTransfer::full_duplex(cs, tx, rx.len())];
        self.transfer(&mut seg)?;
        rx.copy_from_slice(&seg[0].data[tx.len()..]);
        Ok(())
    }
}
