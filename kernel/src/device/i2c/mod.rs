//! Generic I2C bus abstractions.
//!
//! This module defines protocol-level types used by I2C controller drivers and
//! higher-level clients in the kernel.

extern crate alloc;

use alloc::vec::Vec;

/// I2C slave address (7-bit or 10-bit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2cAddress {
    /// 7-bit I2C address.
    SevenBit(u8),
    /// 10-bit I2C address.
    TenBit(u16),
}

impl I2cAddress {
    /// Returns true when this is a 10-bit address.
    pub fn is_ten_bit(&self) -> bool {
        matches!(self, Self::TenBit(_))
    }

    /// Returns address value as raw integer.
    pub fn raw(&self) -> u16 {
        match self {
            Self::SevenBit(v) => *v as u16,
            Self::TenBit(v) => *v,
        }
    }
}

/// I2C message flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct I2cMessageFlags(u16);

impl I2cMessageFlags {
    /// Empty/no flags.
    pub const NONE: Self = Self(0);
    /// Read transaction (controller reads from slave).
    pub const READ: Self = Self(1 << 0);
    /// Send STOP condition after this message.
    pub const STOP: Self = Self(1 << 1);
    /// Do not issue START/address phase before this message.
    pub const NOSTART: Self = Self(1 << 2);
    /// Message uses 10-bit address mode.
    pub const TEN_BIT: Self = Self(1 << 3);

    /// Returns true when all bits in `other` are present.
    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Raw flags value.
    pub fn bits(self) -> u16 {
        self.0
    }
}

impl core::ops::BitOr for I2cMessageFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for I2cMessageFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// A single I2C message segment.
#[derive(Debug, Clone)]
pub struct I2cMessage {
    /// Target slave address.
    pub addr: I2cAddress,
    /// Segment flags.
    pub flags: I2cMessageFlags,
    /// Payload buffer.
    ///
    /// For write messages, this contains bytes to transmit.
    /// For read messages, this is pre-sized and filled by the bus driver.
    pub data: Vec<u8>,
}

impl I2cMessage {
    /// Creates a write message.
    pub fn write(addr: I2cAddress, data: &[u8], stop: bool) -> Self {
        let mut flags = I2cMessageFlags::NONE;
        if stop {
            flags |= I2cMessageFlags::STOP;
        }
        if addr.is_ten_bit() {
            flags |= I2cMessageFlags::TEN_BIT;
        }

        Self {
            addr,
            flags,
            data: data.to_vec(),
        }
    }

    /// Creates a read message with a fixed receive length.
    pub fn read(addr: I2cAddress, len: usize, stop: bool) -> Self {
        let mut flags = I2cMessageFlags::READ;
        if stop {
            flags |= I2cMessageFlags::STOP;
        }
        if addr.is_ten_bit() {
            flags |= I2cMessageFlags::TEN_BIT;
        }

        Self {
            addr,
            flags,
            data: alloc::vec![0; len],
        }
    }

    /// Creates a message with explicit flags and payload.
    pub fn with_flags(addr: I2cAddress, flags: I2cMessageFlags, data: Vec<u8>) -> Self {
        Self { addr, flags, data }
    }
}

/// A complete I2C transaction (one or more message segments).
#[derive(Debug, Clone, Default)]
pub struct I2cTransfer {
    messages: Vec<I2cMessage>,
}

impl I2cTransfer {
    /// Create an empty transfer.
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    /// Create a transfer from pre-built messages.
    pub fn from_messages(messages: Vec<I2cMessage>) -> Self {
        Self { messages }
    }

    /// Append a message to this transfer.
    pub fn push(&mut self, msg: I2cMessage) {
        self.messages.push(msg);
    }

    /// Borrow all messages.
    pub fn messages(&self) -> &[I2cMessage] {
        &self.messages
    }

    /// Borrow all messages mutably.
    pub fn messages_mut(&mut self) -> &mut [I2cMessage] {
        &mut self.messages
    }
}

/// I2C transfer errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2cError {
    /// Slave NACKed address or data.
    Nack,
    /// Multi-master arbitration loss.
    ArbitrationLost,
    /// Polling or bus operation timeout.
    Timeout,
    /// Generic bus/controller failure.
    BusError,
    /// Invalid argument.
    InvalidArg,
}

/// Generic I2C controller interface.
pub trait I2cBus: Send + Sync {
    /// Execute a full I2C transaction.
    fn transfer(&self, msgs: &mut [I2cMessage]) -> Result<(), I2cError>;

    /// Configure bus speed in Hz.
    fn set_bus_speed(&self, hz: u32) -> Result<(), I2cError>;

    /// Get current bus speed in Hz.
    fn bus_speed(&self) -> u32;

    /// Get logical bus number.
    fn bus_number(&self) -> u32;
}
