//! MMIO access helpers for AArch64.
//!
//! # Why this exists
//! On QEMU+HVF, some trapped MMIO accesses can arrive as EC_DATAABORT without ISV
//! set, which causes QEMU's HVF backend to abort (assert(isv)).
//!
//! To keep trapped accesses predictable and decodable, we force single-instruction
//! accesses (ldr/str/ldrb/strb) via inline assembly.
//!
//! # Memory Barriers
//! All write operations include a DSB (Data Synchronization Barrier) to ensure
//! writes complete before continuing. This is critical for device memory.

use core::arch::asm;

/// Read an 8-bit value from an MMIO address.
///
/// # Safety
/// Caller must ensure `addr` is a valid MMIO address for an 8-bit access.
#[inline(always)]
pub unsafe fn read8(addr: usize) -> u8 {
    let val: u32;
    unsafe {
        asm!(
            "ldrb {val:w}, [{addr}]",
            val = out(reg) val,
            addr = in(reg) addr,
            options(nostack, preserves_flags)
        );
    }
    val as u8
}

/// Write an 8-bit value to an MMIO address.
///
/// # Safety
/// Caller must ensure `addr` is a valid MMIO address for an 8-bit access.
#[inline(always)]
pub unsafe fn write8(addr: usize, val: u8) {
    let val32 = val as u32;
    unsafe {
        asm!(
            "strb {val:w}, [{addr}]",
            "dsb sy",  // Ensure write completes before continuing
            val = in(reg) val32,
            addr = in(reg) addr,
            options(nostack, preserves_flags)
        );
    }
}

/// Read a 16-bit value from an MMIO address.
///
/// # Safety
/// Caller must ensure `addr` is a valid MMIO address for a 16-bit access.
#[inline(always)]
pub unsafe fn read16(addr: usize) -> u16 {
    let val: u32;
    unsafe {
        asm!(
            "ldrh {val:w}, [{addr}]",
            val = out(reg) val,
            addr = in(reg) addr,
            options(nostack, preserves_flags)
        );
    }
    val as u16
}

/// Write a 16-bit value to an MMIO address.
///
/// # Safety
/// Caller must ensure `addr` is a valid MMIO address for a 16-bit access.
#[inline(always)]
pub unsafe fn write16(addr: usize, val: u16) {
    let val32 = val as u32;
    unsafe {
        asm!(
            "strh {val:w}, [{addr}]",
            "dsb sy",  // Ensure write completes before continuing
            val = in(reg) val32,
            addr = in(reg) addr,
            options(nostack, preserves_flags)
        );
    }
}

/// Read a 32-bit value from an MMIO address.
///
/// # Safety
/// Caller must ensure `addr` is a valid MMIO address for a 32-bit access.
#[inline(always)]
pub unsafe fn read32(addr: usize) -> u32 {
    let val: u32;
    unsafe {
        asm!(
            "ldr {val:w}, [{addr}]",
            val = out(reg) val,
            addr = in(reg) addr,
            options(nostack, preserves_flags)
        );
    }
    val
}

/// Write a 32-bit value to an MMIO address.
///
/// # Safety
/// Caller must ensure `addr` is a valid MMIO address for a 32-bit access.
#[inline(always)]
pub unsafe fn write32(addr: usize, val: u32) {
    unsafe {
        asm!(
            "str {val:w}, [{addr}]",
            "dsb sy",  // Ensure write completes before continuing
            val = in(reg) val,
            addr = in(reg) addr,
            options(nostack, preserves_flags)
        );
    }
}

/// Read a 64-bit value from an MMIO address.
///
/// # Safety
/// Caller must ensure `addr` is a valid MMIO address for a 64-bit access.
#[inline(always)]
pub unsafe fn read64(addr: usize) -> u64 {
    let val: u64;
    unsafe {
        asm!(
            "ldr {val}, [{addr}]",
            val = out(reg) val,
            addr = in(reg) addr,
            options(nostack, preserves_flags)
        );
    }
    val
}

/// Write a 64-bit value to an MMIO address.
///
/// # Safety
/// Caller must ensure `addr` is a valid MMIO address for a 64-bit access.
#[inline(always)]
pub unsafe fn write64(addr: usize, val: u64) {
    unsafe {
        asm!(
            "str {val}, [{addr}]",
            "dsb sy",  // Ensure write completes before continuing
            val = in(reg) val,
            addr = in(reg) addr,
            options(nostack, preserves_flags)
        );
    }
}
