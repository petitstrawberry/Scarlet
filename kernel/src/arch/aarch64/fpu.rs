//! Floating-Point and SIMD (NEON) context for AArch64
//!
//! This module provides the FPU/SIMD context structure for saving and restoring
//! floating-point and NEON vector register state during context switches.
//! AArch64 has 32 vector registers (V0-V31, each 128-bit) that are used for both
//! floating-point and SIMD operations, plus FPCR and FPSR control/status registers.

use core::arch::asm;

mod fpu_switch;

pub use fpu_switch::{
    kernel_switch_in_user_fpu, kernel_switch_out_user_fpu, kernel_switch_out_user_vector,
};

/// FPU/SIMD context for AArch64 (NEON)
///
/// Contains all vector registers and the floating-point control/status registers.
/// This is saved/restored during task context switches to preserve FPU/SIMD state.
///
/// Each vector register is 128-bit (Q registers / V registers).
/// We store them as pairs of 64-bit values for easier manipulation.
#[repr(C, align(16))]
#[derive(Debug, Clone)]
pub struct FpuContext {
    /// Vector registers V0-V31 (128-bit each, stored as [low, high] pairs)
    pub v: [[u64; 2]; 32],
    /// Floating-point Control Register
    pub fpcr: u64,
    /// Floating-point Status Register
    pub fpsr: u64,
}

impl FpuContext {
    /// Create a new zeroed FPU/SIMD context
    pub const fn new() -> Self {
        Self {
            v: [[0; 2]; 32],
            fpcr: 0,
            fpsr: 0,
        }
    }

    /// Save the current FPU/SIMD state to this context
    ///
    /// # Safety
    /// This function directly accesses FPU/SIMD registers. The FPU/SIMD must be
    /// enabled (CPACR_EL1.FPEN = 0b11) before calling this function.
    #[inline]
    pub unsafe fn save(&mut self) {
        let ptr = self.v.as_mut_ptr() as *mut u8;
        asm!(
            ".arch armv8-a+fp+simd",
            // Save all 32 vector registers using STP for Q registers
            "stp q0, q1, [{0}, #0*32]",
            "stp q2, q3, [{0}, #1*32]",
            "stp q4, q5, [{0}, #2*32]",
            "stp q6, q7, [{0}, #3*32]",
            "stp q8, q9, [{0}, #4*32]",
            "stp q10, q11, [{0}, #5*32]",
            "stp q12, q13, [{0}, #6*32]",
            "stp q14, q15, [{0}, #7*32]",
            "stp q16, q17, [{0}, #8*32]",
            "stp q18, q19, [{0}, #9*32]",
            "stp q20, q21, [{0}, #10*32]",
            "stp q22, q23, [{0}, #11*32]",
            "stp q24, q25, [{0}, #12*32]",
            "stp q26, q27, [{0}, #13*32]",
            "stp q28, q29, [{0}, #14*32]",
            "stp q30, q31, [{0}, #15*32]",
            in(reg) ptr,
            options(nostack),
        );
        // Save FPCR and FPSR
        let fpcr: u64;
        let fpsr: u64;
        asm!(
            ".arch armv8-a+fp+simd",
            "mrs {0}, fpcr",
            "mrs {1}, fpsr",
            out(reg) fpcr,
            out(reg) fpsr,
            options(nomem, nostack),
        );
        self.fpcr = fpcr;
        self.fpsr = fpsr;
    }

    /// Restore the FPU/SIMD state from this context
    ///
    /// # Safety
    /// This function directly accesses FPU/SIMD registers. The FPU/SIMD must be
    /// enabled (CPACR_EL1.FPEN = 0b11) before calling this function.
    #[inline]
    pub unsafe fn restore(&self) {
        // SAFETY: The caller guarantees that FP/SIMD access is enabled.
        unsafe {
            self.restore_control();
            self.restore_vectors();
        }
    }

    /// Restore the floating-point control and status registers.
    ///
    /// # Safety
    ///
    /// FP/SIMD access must be enabled for the current exception level.
    #[inline]
    pub(crate) unsafe fn restore_control(&self) {
        // Restore FPCR and FPSR first
        // SAFETY: Guaranteed by the caller.
        unsafe {
            asm!(
                ".arch armv8-a+fp+simd",
                "msr fpcr, {0}",
                "msr fpsr, {1}",
                in(reg) self.fpcr,
                in(reg) self.fpsr,
                options(nomem, nostack),
            );
        }
    }

    /// Restore the 32 architectural FP/SIMD vector registers.
    ///
    /// # Safety
    ///
    /// FP/SIMD access must be enabled for the current exception level.
    #[inline]
    pub(crate) unsafe fn restore_vectors(&self) {
        let ptr = self.v.as_ptr() as *const u8;
        // SAFETY: Guaranteed by the caller; `FpuContext` is 16-byte aligned
        // and owns the complete 512-byte vector-register image.
        unsafe {
            asm!(
                ".arch armv8-a+fp+simd",
                // Restore all 32 vector registers using LDP for Q registers
                "ldp q0, q1, [{0}, #0*32]",
                "ldp q2, q3, [{0}, #1*32]",
                "ldp q4, q5, [{0}, #2*32]",
                "ldp q6, q7, [{0}, #3*32]",
                "ldp q8, q9, [{0}, #4*32]",
                "ldp q10, q11, [{0}, #5*32]",
                "ldp q12, q13, [{0}, #6*32]",
                "ldp q14, q15, [{0}, #7*32]",
                "ldp q16, q17, [{0}, #8*32]",
                "ldp q18, q19, [{0}, #9*32]",
                "ldp q20, q21, [{0}, #10*32]",
                "ldp q22, q23, [{0}, #11*32]",
                "ldp q24, q25, [{0}, #12*32]",
                "ldp q26, q27, [{0}, #13*32]",
                "ldp q28, q29, [{0}, #14*32]",
                "ldp q30, q31, [{0}, #15*32]",
                in(reg) ptr,
                options(nostack),
            );
        }
    }
}

impl Default for FpuContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Enable FPU/SIMD access by setting CPACR_EL1.FPEN to 0b11
///
/// This must be called before user space can use floating-point or SIMD instructions.
/// The FPEN field (bits 20:21) in CPACR_EL1 controls access:
/// - 0b00: Traps EL0 and EL1 accesses
/// - 0b01: Traps EL0 accesses only
/// - 0b10: Traps EL0 and EL1 accesses
/// - 0b11: No trapping, full access enabled
#[inline]
pub fn enable_fpu() {
    // CPACR_EL1.FPEN is at bits 20:21
    // Set to 0b11 to enable full access
    const CPACR_EL1_FPEN_FULL: u64 = 0b11 << 20;

    unsafe {
        let mut cpacr: u64;
        asm!(
            "mrs {0}, cpacr_el1",
            out(reg) cpacr,
            options(nomem, nostack),
        );
        cpacr |= CPACR_EL1_FPEN_FULL;
        asm!(
            "msr cpacr_el1, {0}",
            "isb",
            in(reg) cpacr,
            options(nomem, nostack),
        );
    }
}

/// Configure whether EL0 (user mode) may use FP/SIMD.
///
/// We keep EL1 access enabled so the kernel can always save/restore contexts.
///
/// CPACR_EL1.FPEN meanings:
/// - 0b00/0b10: trap EL0 and EL1
/// - 0b01:     trap EL0 only (EL1 allowed)
/// - 0b11:     no trapping (EL0+EL1 allowed)
#[inline]
pub fn set_user_fpu_enabled(enabled: bool) {
    const FPEN_SHIFT: u64 = 20;
    const FPEN_MASK: u64 = 0b11 << FPEN_SHIFT;
    const FPEN_TRAP_EL0_ONLY: u64 = 0b01 << FPEN_SHIFT;
    const FPEN_FULL: u64 = 0b11 << FPEN_SHIFT;

    let desired = if enabled {
        FPEN_FULL
    } else {
        FPEN_TRAP_EL0_ONLY
    };

    unsafe {
        let mut cpacr: u64;
        asm!(
            "mrs {0}, cpacr_el1",
            out(reg) cpacr,
            options(nomem, nostack),
        );
        cpacr &= !FPEN_MASK;
        cpacr |= desired;
        asm!(
            "msr cpacr_el1, {0}",
            "isb",
            in(reg) cpacr,
            options(nomem, nostack),
        );
    }
}

/// Check if FPU/SIMD is enabled (CPACR_EL1.FPEN == 0b11)
#[inline]
pub fn is_fpu_enabled() -> bool {
    let cpacr: u64;
    unsafe {
        asm!(
            "mrs {0}, cpacr_el1",
            out(reg) cpacr,
            options(nomem, nostack),
        );
    }
    // FPEN bits are at position 20:21
    ((cpacr >> 20) & 0b11) == 0b11
}
