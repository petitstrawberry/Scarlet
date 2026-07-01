//! Generic SPI bus abstractions.
//!
//! This module defines protocol-level types used by SPI controller drivers and
//! higher-level clients in the kernel, mirroring the [`crate::device::i2c`] pattern.

extern crate alloc;

use alloc::vec::Vec;

/// SPI chip-select mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpiCsMode {
    /// Assert CS for the entire transfer, de-assert after last segment.
    Single,
    /// Keep CS asserted across all segments, de-assert only after the last.
    Hold,
}

/// SPI transfer segment flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpiTransferFlags(u16);

impl SpiTransferFlags {
    /// No flags.
    pub const NONE: Self = Self(0);
    /// This segment is a read (data flows from slave to master).
    pub const READ: Self = Self(1 << 0);
    /// This segment is a write (data flows from master to slave).
    pub const WRITE: Self = Self(1 << 1);

    /// Returns true when all bits in `other` are present.
    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Raw flags value.
    pub fn bits(self) -> u16 {
        self.0
    }
}

impl core::ops::BitOr for SpiTransferFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

/// A single SPI transfer segment (half-duplex: either read or write).
#[derive(Debug, Clone)]
pub struct SpiTransfer {
    /// Segment flags.
    pub flags: SpiTransferFlags,
    /// Payload buffer.
    ///
    /// For write segments, this contains bytes to transmit.
    /// For read segments, this is pre-sized and filled by the bus driver.
    pub data: Vec<u8>,
    /// Chip-select number (0-based).
    pub cs: u8,
    /// SPI clock frequency in Hz for this segment (0 = use bus default).
    pub speed_hz: u32,
    /// Delay in microseconds after asserting chip-select before this segment.
    pub delay_before_us: u64,
    /// Delay in microseconds after this segment before the next segment or chip-select deassertion.
    pub delay_after_us: u64,
}

impl SpiTransfer {
    /// Creates a write transfer to the given chip-select.
    pub fn write(cs: u8, data: &[u8]) -> Self {
        Self {
            flags: SpiTransferFlags::WRITE,
            data: data.to_vec(),
            cs,
            speed_hz: 0,
            delay_before_us: 0,
            delay_after_us: 0,
        }
    }

    /// Creates a read transfer with a fixed receive length.
    pub fn read(cs: u8, len: usize) -> Self {
        Self {
            flags: SpiTransferFlags::READ,
            data: alloc::vec![0; len],
            cs,
            speed_hz: 0,
            delay_before_us: 0,
            delay_after_us: 0,
        }
    }

    /// Creates a full-duplex transfer (simultaneous write and read).
    ///
    /// `tx` bytes are sent while `rx` buffer is filled with received data.
    /// The returned transfer has both READ and WRITE flags set.
    pub fn full_duplex(cs: u8, tx: &[u8], _rx_len: usize) -> Self {
        Self {
            flags: SpiTransferFlags::READ | SpiTransferFlags::WRITE,
            data: tx.to_vec(),
            cs,
            speed_hz: 0,
            delay_before_us: 0,
            delay_after_us: 0,
        }
    }

    /// Creates a transfer with explicit flags and payload.
    pub fn with_flags(cs: u8, flags: SpiTransferFlags, data: Vec<u8>, speed_hz: u32) -> Self {
        Self {
            flags,
            data,
            cs,
            speed_hz,
            delay_before_us: 0,
            delay_after_us: 0,
        }
    }
}

/// SPI transfer errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpiError {
    /// Slave did not respond.
    NoResponse,
    /// Transfer timed out.
    Timeout,
    /// Generic bus or controller failure.
    BusError,
    /// Invalid argument (e.g. zero-length transfer, bad CS).
    InvalidArg,
    /// FIFO overflow or underflow.
    FifoError,
}

/// Generic SPI controller interface.
///
/// Controller drivers implement this trait to expose SPI bus capabilities.
/// The interface is designed for half-duplex operation; for full-duplex
/// use [`SpiTransferFlags::READ | SpiTransferFlags::WRITE`].
pub trait SpiBus: Send + Sync {
    /// Execute a sequence of SPI transfer segments.
    ///
    /// All segments share the same chip-select (asserted before the first,
    /// de-asserted after the last).
    fn transfer(&self, segments: &mut [SpiTransfer]) -> Result<(), SpiError>;

    /// Configure default bus speed in Hz.
    fn set_bus_speed(&self, hz: u32) -> Result<(), SpiError>;

    /// Get current bus speed in Hz.
    fn bus_speed(&self) -> u32;

    /// Get logical bus number.
    fn bus_number(&self) -> u32;
}
