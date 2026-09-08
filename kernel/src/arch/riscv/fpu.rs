//! Floating-Point Unit and Vector context for RISC-V 64-bit
//!
//! This module provides the FPU and Vector context structures for saving and restoring
//! floating-point and vector register state during context switches.
//!
//! ## FPU (F/D Extensions)
//! RISC-V uses the F (single-precision) and D (double-precision) extensions with
//! 32 floating-point registers (f0-f31, each 64-bit for D extension) and fcsr
//! control/status register.
//!
//! ## Vector (V Extension)
//! RISC-V Vector extension provides 32 vector registers (v0-v31) with configurable
//! VLEN (vector length). The actual size depends on the implementation. This module
//! supports VLEN up to 256 bits (32 bytes per register, vlenb=32).

use core::arch::asm;

mod fpu_switch;

pub use fpu_switch::{
    kernel_switch_in_user_fpu, kernel_switch_out_user_fpu, kernel_switch_out_user_vector,
};

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
        unsafe {
            asm!(
                ".option push",
                ".option arch, +f, +d",
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
                ".option pop",
                in(reg) ptr,
                options(nostack),
            );
        }
        // Save fcsr
        let fcsr: u32;
        unsafe {
            asm!(
                ".option push",
                ".option arch, +f, +d",
                "frcsr {0}",
                ".option pop",
                out(reg) fcsr,
                options(nomem, nostack),
            );
        }
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
        unsafe {
            asm!(
                ".option push",
                ".option arch, +f, +d",
                "fscsr {0}",
                ".option pop",
                in(reg) self.fcsr,
                options(nomem, nostack),
            );
        }
        let ptr = self.f.as_ptr();
        unsafe {
            asm!(
                ".option push",
                ".option arch, +f, +d",
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
                ".option pop",
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

/// Maximum vector length in bytes (VLEN / 8) supported by this implementation.
/// This supports VLEN up to 256 bits (32 bytes per register).
/// QEMU virt machine typically uses VLEN=128 (vlenb=16).
pub const MAX_VLENB: usize = 32;

/// Vector context for RISC-V 64-bit (V extension)
///
/// Contains all vector registers and vector CSRs.
/// The actual number of bytes used per register depends on the implementation's VLEN.
/// This structure reserves space for VLEN up to 256 bits.
#[repr(C, align(16))]
#[derive(Debug, Clone)]
pub struct VectorContext {
    /// Vector registers v0-v31 (up to 256 bits = 32 bytes each)
    /// Stored as arrays of u64 for alignment
    pub v: [[u64; MAX_VLENB / 8]; 32],
    /// Vector type register (vtype)
    pub vtype: u64,
    /// Vector length register (vl)
    pub vl: u64,
    /// Vector start index register (vstart)
    pub vstart: u64,
    /// Vector fixed-point rounding mode register (vxrm)
    pub vxrm: u64,
    /// Vector fixed-point saturation flag (vxsat)
    pub vxsat: u64,
    /// Vector control and status register (vcsr) - combines vxrm and vxsat
    pub vcsr: u64,
    /// Cached vlenb value (VLEN/8 in bytes)
    pub vlenb: u64,
}

impl VectorContext {
    /// Create a new zeroed Vector context
    pub const fn new() -> Self {
        Self {
            v: [[0; MAX_VLENB / 8]; 32],
            vtype: 0,
            vl: 0,
            vstart: 0,
            vxrm: 0,
            vxsat: 0,
            vcsr: 0,
            vlenb: 0,
        }
    }

    /// Save the current Vector state to this context
    ///
    /// # Safety
    /// This function directly accesses Vector registers. The Vector extension must
    /// be enabled (sstatus.VS != Off) before calling this function.
    #[inline]
    pub unsafe fn save(&mut self) {
        // Read vlenb to know the actual vector register size
        let vlenb: u64;
        unsafe {
            asm!(
                ".option push",
                ".option arch, +v",
                "csrr {0}, vlenb",
                ".option pop",
                out(reg) vlenb,
                options(nomem, nostack),
            );
        }
        self.vlenb = vlenb;

        // Save vector CSRs
        unsafe {
            asm!(
                ".option push",
                ".option arch, +v",
                "csrr {0}, vtype",
                "csrr {1}, vl",
                "csrr {2}, vstart",
                "csrr {3}, vcsr",
                ".option pop",
                out(reg) self.vtype,
                out(reg) self.vl,
                out(reg) self.vstart,
                out(reg) self.vcsr,
                options(nomem, nostack),
            );
        }

        // Extract vxrm and vxsat from vcsr
        self.vxrm = (self.vcsr >> 1) & 0x3;
        self.vxsat = self.vcsr & 0x1;

        // Save vector registers using vs1r.v (whole register store).
        // Use the runtime vlenb as the stride so we only touch the bytes that
        // the implementation actually uses. This avoids unnecessary memory
        // traffic (e.g. QEMU virt often uses vlenb=16).
        let ptr = self.v.as_mut_ptr() as *mut u8;
        let stride = vlenb as usize;

        // Use inline assembly to save each vector register
        // vs1r.v stores one vector register (VLEN bits)
        unsafe {
            asm!(
                ".option push",
                ".option arch, +v",
                "add t0, {ptr}, {stride}",
                "vs1r.v v0, ({ptr})",
                "vs1r.v v1, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vs1r.v v2, ({ptr})",
                "vs1r.v v3, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vs1r.v v4, ({ptr})",
                "vs1r.v v5, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vs1r.v v6, ({ptr})",
                "vs1r.v v7, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vs1r.v v8, ({ptr})",
                "vs1r.v v9, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vs1r.v v10, ({ptr})",
                "vs1r.v v11, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vs1r.v v12, ({ptr})",
                "vs1r.v v13, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vs1r.v v14, ({ptr})",
                "vs1r.v v15, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vs1r.v v16, ({ptr})",
                "vs1r.v v17, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vs1r.v v18, ({ptr})",
                "vs1r.v v19, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vs1r.v v20, ({ptr})",
                "vs1r.v v21, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vs1r.v v22, ({ptr})",
                "vs1r.v v23, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vs1r.v v24, ({ptr})",
                "vs1r.v v25, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vs1r.v v26, ({ptr})",
                "vs1r.v v27, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vs1r.v v28, ({ptr})",
                "vs1r.v v29, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vs1r.v v30, ({ptr})",
                "vs1r.v v31, (t0)",
                ".option pop",
                ptr = inout(reg) ptr => _,
                stride = in(reg) stride,
                out("t0") _,
                options(nostack),
            );
        }
    }

    /// Restore the Vector state from this context
    ///
    /// # Safety
    /// This function directly accesses Vector registers. The Vector extension must
    /// be enabled (sstatus.VS != Off) before calling this function.
    #[inline]
    pub unsafe fn restore(&self) {
        // Restore vector CSRs first
        unsafe {
            asm!(
                ".option push",
                ".option arch, +v",
                "csrw vstart, {0}",
                "csrw vcsr, {1}",
                ".option pop",
                in(reg) self.vstart,
                in(reg) self.vcsr,
                options(nomem, nostack),
            );
        }

        // Restore vtype and vl using vsetvl
        // This sets both vtype and vl atomically
        unsafe {
            asm!(
                ".option push",
                ".option arch, +v",
                "vsetvl x0, {0}, {1}",
                ".option pop",
                in(reg) self.vl,
                in(reg) self.vtype,
                options(nomem, nostack),
            );
        }

        // Restore vector registers using vl1r.v (whole register load)
        let ptr = self.v.as_ptr() as *const u8;
        // Use the saved vlenb if available; fall back to MAX_VLENB for
        // never-saved (zero-initial) contexts.
        let stride = if self.vlenb == 0 {
            MAX_VLENB
        } else {
            self.vlenb as usize
        };

        unsafe {
            asm!(
                ".option push",
                ".option arch, +v",
                "add t0, {ptr}, {stride}",
                "vl1r.v v0, ({ptr})",
                "vl1r.v v1, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vl1r.v v2, ({ptr})",
                "vl1r.v v3, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vl1r.v v4, ({ptr})",
                "vl1r.v v5, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vl1r.v v6, ({ptr})",
                "vl1r.v v7, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vl1r.v v8, ({ptr})",
                "vl1r.v v9, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vl1r.v v10, ({ptr})",
                "vl1r.v v11, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vl1r.v v12, ({ptr})",
                "vl1r.v v13, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vl1r.v v14, ({ptr})",
                "vl1r.v v15, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vl1r.v v16, ({ptr})",
                "vl1r.v v17, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vl1r.v v18, ({ptr})",
                "vl1r.v v19, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vl1r.v v20, ({ptr})",
                "vl1r.v v21, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vl1r.v v22, ({ptr})",
                "vl1r.v v23, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vl1r.v v24, ({ptr})",
                "vl1r.v v25, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vl1r.v v26, ({ptr})",
                "vl1r.v v27, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vl1r.v v28, ({ptr})",
                "vl1r.v v29, (t0)",
                "add {ptr}, t0, {stride}",
                "add t0, {ptr}, {stride}",
                "vl1r.v v30, ({ptr})",
                "vl1r.v v31, (t0)",
                ".option pop",
                ptr = inout(reg) ptr => _,
                stride = in(reg) stride,
                out("t0") _,
                options(nostack),
            );
        }
    }
}

impl Default for VectorContext {
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

/// Disable FPU access by setting sstatus.FS to Off.
///
/// When FS is Off, any FPU instruction executed (in S/U) raises an illegal
/// instruction exception. The kernel should re-enable FS temporarily when it
/// needs to save/restore user state.
#[inline]
pub fn disable_fpu() {
    const SSTATUS_FS_MASK: usize = 0x6000;

    unsafe {
        asm!(
            "csrr {0}, sstatus",
            "and {0}, {0}, {1}",
            "csrw sstatus, {0}",
            out(reg) _,
            in(reg) !SSTATUS_FS_MASK,
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

/// Check if the FPU state is marked Dirty in sstatus (FS == 0b11).
#[inline]
pub fn is_fpu_dirty() -> bool {
    let sstatus: usize;
    unsafe {
        asm!(
            "csrr {0}, sstatus",
            out(reg) sstatus,
            options(nomem, nostack),
        );
    }
    (sstatus & 0x6000) == 0x6000
}

/// Mark the FPU state as Clean in sstatus (FS = 0b10).
///
/// This is useful after saving/restoring FPU state so that a task that doesn't
/// touch the FPU in its next timeslice won't incur an unnecessary save.
#[inline]
pub fn mark_fpu_clean() {
    const SSTATUS_FS_CLEAN: usize = 0x4000;
    const SSTATUS_FS_MASK: usize = 0x6000;

    unsafe {
        asm!(
            "csrr {0}, sstatus",
            "and {0}, {0}, {1}",
            "or {0}, {0}, {2}",
            "csrw sstatus, {0}",
            out(reg) _,
            in(reg) !SSTATUS_FS_MASK,
            in(reg) SSTATUS_FS_CLEAN,
            options(nomem, nostack),
        );
    }
}

/// Enable Vector extension access by setting sstatus.VS to Initial state
///
/// This must be called before user space can use vector instructions.
/// The VS field in sstatus controls access to the Vector extension:
/// - 00: Off - Vector access causes illegal instruction exception
/// - 01: Initial - Vector is enabled with initial state
/// - 10: Clean - Vector is enabled, state has not been modified
/// - 11: Dirty - Vector is enabled, state has been modified
#[inline]
pub fn enable_vector() {
    // sstatus.VS bits are at position 9:10 (bits 9 and 10)
    // Set to Initial (01) = 0x200
    const SSTATUS_VS_INITIAL: usize = 0x200;
    const SSTATUS_VS_MASK: usize = 0x600;

    unsafe {
        asm!(
            "csrr {0}, sstatus",
            "and {0}, {0}, {1}",  // Clear VS bits
            "or {0}, {0}, {2}",   // Set VS to Initial
            "csrw sstatus, {0}",
            out(reg) _,
            in(reg) !SSTATUS_VS_MASK,
            in(reg) SSTATUS_VS_INITIAL,
            options(nomem, nostack),
        );
    }
}

/// Disable Vector extension access by setting sstatus.VS to Off.
#[inline]
pub fn disable_vector() {
    const SSTATUS_VS_MASK: usize = 0x600;

    unsafe {
        asm!(
            "csrr {0}, sstatus",
            "and {0}, {0}, {1}",
            "csrw sstatus, {0}",
            out(reg) _,
            in(reg) !SSTATUS_VS_MASK,
            options(nomem, nostack),
        );
    }
}

/// Check if Vector extension is enabled (sstatus.VS != Off)
#[inline]
pub fn is_vector_enabled() -> bool {
    let sstatus: usize;
    unsafe {
        asm!(
            "csrr {0}, sstatus",
            out(reg) sstatus,
            options(nomem, nostack),
        );
    }
    // VS bits are at position 9:10
    (sstatus & 0x600) != 0
}

/// Check if the Vector state is marked Dirty in sstatus (VS == 0b11).
#[inline]
pub fn is_vector_dirty() -> bool {
    let sstatus: usize;
    unsafe {
        asm!(
            "csrr {0}, sstatus",
            out(reg) sstatus,
            options(nomem, nostack),
        );
    }
    (sstatus & 0x600) == 0x600
}

/// Mark the Vector state as Clean in sstatus (VS = 0b10).
#[inline]
pub fn mark_vector_clean() {
    const SSTATUS_VS_CLEAN: usize = 0x400;
    const SSTATUS_VS_MASK: usize = 0x600;

    unsafe {
        asm!(
            "csrr {0}, sstatus",
            "and {0}, {0}, {1}",
            "or {0}, {0}, {2}",
            "csrw sstatus, {0}",
            out(reg) _,
            in(reg) !SSTATUS_VS_MASK,
            in(reg) SSTATUS_VS_CLEAN,
            options(nomem, nostack),
        );
    }
}

/// Get the vector length in bytes (vlenb = VLEN / 8)
///
/// Returns 0 if the Vector extension is not available.
#[inline]
pub fn get_vlenb() -> usize {
    if !is_vector_enabled() {
        return 0;
    }
    let vlenb: usize;
    unsafe {
        asm!(
            "csrr {0}, vlenb",
            out(reg) vlenb,
            options(nomem, nostack),
        );
    }
    vlenb
}
