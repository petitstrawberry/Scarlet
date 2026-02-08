//! MMIO access helpers for RISC-V.
//!
//! RISC-V does not currently require the AArch64/HVF-specific single-instruction
//! workaround. Keep this minimal and use volatile pointer accesses.

/// Read an 8-bit value from an MMIO address.
///
/// # Safety
/// Caller must ensure `addr` is a valid MMIO address for an 8-bit access.
#[inline(always)]
pub unsafe fn read8(addr: usize) -> u8 {
    unsafe { core::ptr::read_volatile(addr as *const u8) }
}

/// Write an 8-bit value to an MMIO address.
///
/// # Safety
/// Caller must ensure `addr` is a valid MMIO address for an 8-bit access.
#[inline(always)]
pub unsafe fn write8(addr: usize, val: u8) {
    unsafe { core::ptr::write_volatile(addr as *mut u8, val) }
}

/// Read a 16-bit value from an MMIO address.
///
/// # Safety
/// Caller must ensure `addr` is a valid MMIO address for a 16-bit access.
#[inline(always)]
pub unsafe fn read16(addr: usize) -> u16 {
    unsafe { core::ptr::read_volatile(addr as *const u16) }
}

/// Write a 16-bit value to an MMIO address.
///
/// # Safety
/// Caller must ensure `addr` is a valid MMIO address for a 16-bit access.
#[inline(always)]
pub unsafe fn write16(addr: usize, val: u16) {
    unsafe { core::ptr::write_volatile(addr as *mut u16, val) }
}

/// Read a 32-bit value from an MMIO address.
///
/// # Safety
/// Caller must ensure `addr` is a valid MMIO address for a 32-bit access.
#[inline(always)]
pub unsafe fn read32(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

/// Write a 32-bit value to an MMIO address.
///
/// # Safety
/// Caller must ensure `addr` is a valid MMIO address for a 32-bit access.
#[inline(always)]
pub unsafe fn write32(addr: usize, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}

/// Read a 64-bit value from an MMIO address.
///
/// # Safety
/// Caller must ensure `addr` is a valid MMIO address for a 64-bit access.
#[inline(always)]
pub unsafe fn read64(addr: usize) -> u64 {
    unsafe { core::ptr::read_volatile(addr as *const u64) }
}

/// Write a 64-bit value to an MMIO address.
///
/// # Safety
/// Caller must ensure `addr` is a valid MMIO address for a 64-bit access.
#[inline(always)]
pub unsafe fn write64(addr: usize, val: u64) {
    unsafe { core::ptr::write_volatile(addr as *mut u64, val) }
}
