//! Floating-Point Unit context for RISC-V 64-bit
//!
//! This module provides the FPU context structure for saving and restoring
//! floating-point register state during context switches. RISC-V uses the
//! F (single-precision) and D (double-precision) extensions with 32 floating-point
//! registers (f0-f31, each 64-bit for D extension) and fcsr control/status register.

use core::arch::asm;

/// FPU context for RISC-V 64-bit (F/D extensions)
///
/// Contains all floating-point registers and the floating-point control/status register.
/// This is saved/restored during task context switches to preserve FPU state.
#[repr(C, align(16))]
#[derive(Debug, Clone)]
pub struct FpuContext {
    /// Floating-point registers f0-f31 (64-bit each for D extension)
    pub f: [u64; 32],
    /// Floating-point control and status register (fcsr)
    pub fcsr: u32,
}

impl FpuContext {
    /// Create a new zeroed FPU context
    pub const fn new() -> Self {
        Self {
            f: [0; 32],
            fcsr: 0,
        }
    }

    /// Save the current FPU state to this context
    ///
    /// # Safety
    /// This function directly accesses FPU registers. The FPU must be enabled
    /// (sstatus.FS != Off) before calling this function.
    #[inline]
    pub unsafe fn save(&mut self) {
        let ptr = self.f.as_mut_ptr();
        asm!(
            // Save all 32 floating-point registers
            "fsd f0, 0*8({0})",
            "fsd f1, 1*8({0})",
            "fsd f2, 2*8({0})",
            "fsd f3, 3*8({0})",
            "fsd f4, 4*8({0})",
            "fsd f5, 5*8({0})",
            "fsd f6, 6*8({0})",
            "fsd f7, 7*8({0})",
            "fsd f8, 8*8({0})",
            "fsd f9, 9*8({0})",
            "fsd f10, 10*8({0})",
            "fsd f11, 11*8({0})",
            "fsd f12, 12*8({0})",
            "fsd f13, 13*8({0})",
            "fsd f14, 14*8({0})",
            "fsd f15, 15*8({0})",
            "fsd f16, 16*8({0})",
            "fsd f17, 17*8({0})",
            "fsd f18, 18*8({0})",
            "fsd f19, 19*8({0})",
            "fsd f20, 20*8({0})",
            "fsd f21, 21*8({0})",
            "fsd f22, 22*8({0})",
            "fsd f23, 23*8({0})",
            "fsd f24, 24*8({0})",
            "fsd f25, 25*8({0})",
            "fsd f26, 26*8({0})",
            "fsd f27, 27*8({0})",
            "fsd f28, 28*8({0})",
            "fsd f29, 29*8({0})",
            "fsd f30, 30*8({0})",
            "fsd f31, 31*8({0})",
            in(reg) ptr,
            options(nostack),
        );
        // Save fcsr
        let fcsr: u32;
        asm!(
            "frcsr {0}",
            out(reg) fcsr,
            options(nomem, nostack),
        );
        self.fcsr = fcsr;
    }

    /// Restore the FPU state from this context
    ///
    /// # Safety
    /// This function directly accesses FPU registers. The FPU must be enabled
    /// (sstatus.FS != Off) before calling this function.
    #[inline]
    pub unsafe fn restore(&self) {
        // Restore fcsr first
        asm!(
            "fscsr {0}",
            in(reg) self.fcsr,
            options(nomem, nostack),
        );
        let ptr = self.f.as_ptr();
        asm!(
            // Restore all 32 floating-point registers
            "fld f0, 0*8({0})",
            "fld f1, 1*8({0})",
            "fld f2, 2*8({0})",
            "fld f3, 3*8({0})",
            "fld f4, 4*8({0})",
            "fld f5, 5*8({0})",
            "fld f6, 6*8({0})",
            "fld f7, 7*8({0})",
            "fld f8, 8*8({0})",
            "fld f9, 9*8({0})",
            "fld f10, 10*8({0})",
            "fld f11, 11*8({0})",
            "fld f12, 12*8({0})",
            "fld f13, 13*8({0})",
            "fld f14, 14*8({0})",
            "fld f15, 15*8({0})",
            "fld f16, 16*8({0})",
            "fld f17, 17*8({0})",
            "fld f18, 18*8({0})",
            "fld f19, 19*8({0})",
            "fld f20, 20*8({0})",
            "fld f21, 21*8({0})",
            "fld f22, 22*8({0})",
            "fld f23, 23*8({0})",
            "fld f24, 24*8({0})",
            "fld f25, 25*8({0})",
            "fld f26, 26*8({0})",
            "fld f27, 27*8({0})",
            "fld f28, 28*8({0})",
            "fld f29, 29*8({0})",
            "fld f30, 30*8({0})",
            "fld f31, 31*8({0})",
            in(reg) ptr,
            options(nostack),
        );
    }
}

impl Default for FpuContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Enable FPU access by setting sstatus.FS to Initial state
///
/// This must be called before user space can use floating-point instructions.
/// The FS field in sstatus controls access to the FPU:
/// - 00: Off - FPU access causes illegal instruction exception
/// - 01: Initial - FPU is enabled with initial state
/// - 10: Clean - FPU is enabled, state has not been modified
/// - 11: Dirty - FPU is enabled, state has been modified
#[inline]
pub fn enable_fpu() {
    // sstatus.FS bits are at position 13:14
    // Set to Initial (01) = 0x2000
    const SSTATUS_FS_INITIAL: usize = 0x2000;
    const SSTATUS_FS_MASK: usize = 0x6000;

    unsafe {
        asm!(
            "csrr {0}, sstatus",
            "and {0}, {0}, {1}",  // Clear FS bits
            "or {0}, {0}, {2}",   // Set FS to Initial
            "csrw sstatus, {0}",
            out(reg) _,
            in(reg) !SSTATUS_FS_MASK,
            in(reg) SSTATUS_FS_INITIAL,
            options(nomem, nostack),
        );
    }
}

/// Check if FPU is enabled (sstatus.FS != Off)
#[inline]
pub fn is_fpu_enabled() -> bool {
    let sstatus: usize;
    unsafe {
        asm!(
            "csrr {0}, sstatus",
            out(reg) sstatus,
            options(nomem, nostack),
        );
    }
    // FS bits are at position 13:14
    (sstatus & 0x6000) != 0
}
