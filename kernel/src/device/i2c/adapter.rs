//! I2C adapter wrapper for shared bus access.

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::device::i2c::{I2cAddress, I2cBus, I2cError, I2cMessage, I2cMessageFlags};

/// Shared adapter for a single I2C controller.
pub struct I2cAdapter {
    bus: Arc<dyn I2cBus>,
    bus_lock: Mutex<()>,
}

impl I2cAdapter {
    /// Create a new adapter from an I2C bus implementation.
    pub fn new(bus: Arc<dyn I2cBus>) -> Self {
        Self {
            bus,
            bus_lock: Mutex::new(()),
        }
    }

    /// Returns adapter bus number.
    pub fn bus_number(&self) -> u32 {
        self.bus.bus_number()
    }

    /// Returns current bus speed in Hz.
    pub fn bus_speed(&self) -> u32 {
        self.bus.bus_speed()
    }

    /// Sets bus speed in Hz.
    pub fn set_bus_speed(&self, hz: u32) -> Result<(), I2cError> {
        let _guard = self.bus_lock.lock();
        self.bus.set_bus_speed(hz)
    }

    /// Submit a low-level transfer.
    pub fn transfer(&self, msgs: &mut [I2cMessage]) -> Result<(), I2cError> {
        let _guard = self.bus_lock.lock();
        self.bus.transfer(msgs)
    }

    /// Write bytes to a slave.
    pub fn write(&self, addr: I2cAddress, data: &[u8]) -> Result<(), I2cError> {
        let mut msgs = [I2cMessage::write(addr, data, true)];
        self.transfer(&mut msgs)
    }

    /// Read bytes from a slave.
    pub fn read(&self, addr: I2cAddress, buf: &mut [u8]) -> Result<(), I2cError> {
        let mut msgs = [I2cMessage::read(addr, buf.len(), true)];
        self.transfer(&mut msgs)?;
        buf.copy_from_slice(&msgs[0].data);
        Ok(())
    }

    /// Write then read in one transaction.
    pub fn write_then_read(
        &self,
        addr: I2cAddress,
        write: &[u8],
        read: &mut [u8],
    ) -> Result<(), I2cError> {
        let mut msgs = [
            I2cMessage::with_flags(addr, I2cMessageFlags::NONE, write.to_vec()),
            I2cMessage::with_flags(
                addr,
                I2cMessageFlags::READ | I2cMessageFlags::NOSTART | I2cMessageFlags::STOP,
                alloc::vec![0; read.len()],
            ),
        ];
        self.transfer(&mut msgs)?;
        read.copy_from_slice(&msgs[1].data);
        Ok(())
    }

    /// Read consecutive bytes starting from an 8-bit register offset.
    pub fn read_register(&self, addr: I2cAddress, reg: u8, buf: &mut [u8]) -> Result<(), I2cError> {
        self.write_then_read(addr, &[reg], buf)
    }

    /// Write one register plus payload bytes.
    pub fn write_register(&self, addr: I2cAddress, reg: u8, data: &[u8]) -> Result<(), I2cError> {
        let mut payload = Vec::with_capacity(data.len() + 1);
        payload.push(reg);
        payload.extend_from_slice(data);
        self.write(addr, &payload)
    }

    /// Probes if a slave responds on this address.
    pub fn probe_device(&self, addr: I2cAddress) -> bool {
        self.write(addr, &[]).is_ok()
    }
}
