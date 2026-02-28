//! x86_64 MMIO (Memory-Mapped I/O) utilities
//!
//! Provides safe volatile access to memory-mapped I/O regions

use core::ptr::{read_volatile, write_volatile};

/// Read 8-bit value from MMIO address
#[inline(always)]
pub fn read_u8(addr: usize) -> u8 {
    unsafe { read_volatile(addr as *const u8) }
}

/// Read 16-bit value from MMIO address
#[inline(always)]
pub fn read_u16(addr: usize) -> u16 {
    unsafe { read_volatile(addr as *const u16) }
}

/// Read 32-bit value from MMIO address
#[inline(always)]
pub fn read_u32(addr: usize) -> u32 {
    unsafe { read_volatile(addr as *const u32) }
}

/// Read 64-bit value from MMIO address
#[inline(always)]
pub fn read_u64(addr: usize) -> u64 {
    unsafe { read_volatile(addr as *const u64) }
}

/// Write 8-bit value to MMIO address
#[inline(always)]
pub fn write_u8(addr: usize, value: u8) {
    unsafe { write_volatile(addr as *mut u8, value) }
}

/// Write 16-bit value to MMIO address
#[inline(always)]
pub fn write_u16(addr: usize, value: u16) {
    unsafe { write_volatile(addr as *mut u16, value) }
}

/// Write 32-bit value to MMIO address
#[inline(always)]
pub fn write_u32(addr: usize, value: u32) {
    unsafe { write_volatile(addr as *mut u32, value) }
}

/// Write 64-bit value to MMIO address
#[inline(always)]
pub fn write_u64(addr: usize, value: u64) {
    unsafe { write_volatile(addr as *mut u64, value) }
}
