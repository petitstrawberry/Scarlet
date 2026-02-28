//! x86_64 FPU/SIMD state management
//!
//! Uses x87 FPU and SSE/AVX state saving/restoring via FXSAVE64/FXRSTOR64

use core::arch::asm;

static mut USER_FPU_ENABLED: bool = false;

/// Size of FXSAVE64 area (512 bytes for FXSAVE, can be larger for AVX)
const FXSAVE_SIZE: usize = 512;

/// FPU state buffer
#[repr(C, align(16))]
#[derive(Debug, Clone)]
pub struct FpuState {
    buffer: [u8; FXSAVE_SIZE],
}

impl FpuState {
    pub const fn new() -> Self {
        FpuState {
            buffer: [0; FXSAVE_SIZE],
        }
    }
}

/// Initialize FPU for kernel use
pub fn init_fpu() {
    unsafe {
        // Set CR4.OSFXSR and CR4.OSXMMEXCPT to enable FXSAVE/FXRSTOR
        let mut cr4: u64;
        asm!("mov {}, cr4", out(reg) cr4);
        cr4 |= (1 << 9) | (1 << 10); // OSFXSR | OSXMMEXCPT
        asm!("mov cr4, {}", in(reg) cr4);

        // Initialize FPU state
        asm!("fninit", options(nostack));
    }
}

/// Save current FPU state
#[inline(always)]
pub fn fpu_save(state: &mut FpuState) {
    unsafe {
        asm!(
            "fxsave64 [{}]",
            in(reg) state.buffer.as_ptr(),
            options(nostack, preserves_flags)
        );
    }
}

/// Restore FPU state
#[inline(always)]
pub fn fpu_restore(state: &FpuState) {
    unsafe {
        asm!(
            "fxrstor64 [{}]",
            in(reg) state.buffer.as_ptr(),
            options(nostack, preserves_flags)
        );
    }
}

/// Enable FPU/SIMD for user mode
pub fn set_user_fpu_enabled(enabled: bool) {
    unsafe {
        let mut cr0: u64;
        asm!("mov {}, cr0", out(reg) cr0, options(nostack));

        if enabled {
            // Clear EM and MP, set TS
            cr0 &= !(1 << 1); // Clear EM
            cr0 |= 1 << 3; // Set MP (monitor coprocessor)
            cr0 &= !(1 << 2); // Clear TS (task switched)
        } else {
            // Set TS to trap on FPU use
            cr0 |= 1 << 2; // Set TS
        }

        asm!("mov cr0, {}", in(reg) cr0, options(nostack));
        USER_FPU_ENABLED = enabled;
    }
}

/// Check if user FPU is enabled
pub fn get_user_fpu_enabled() -> bool {
    unsafe { USER_FPU_ENABLED }
}

/// Get current FPU ownership (lazy FPU switching)
pub fn fpu_enabled() -> bool {
    unsafe { USER_FPU_ENABLED }
}

/// Save user FPU state during context switch out
pub fn kernel_switch_out_user_fpu(vcpu: &mut crate::arch::vcpu::VCpuState) {
    if vcpu.fpu_used {
        fpu_save(&mut vcpu.fpu_state);
    }
}

/// Restore user FPU state during context switch in
pub fn kernel_switch_in_user_fpu(vcpu: &mut crate::arch::vcpu::VCpuState) {
    if vcpu.fpu_used {
        fpu_restore(&vcpu.fpu_state);
    }
}

/// Save user vector state (stub for x86_64 - handled by FPU save)
pub fn kernel_switch_out_user_vector(_vcpu: &mut crate::arch::vcpu::VCpuState) {
    // Vector state is included in FXSAVE on x86_64
}

/// Restore user vector state (stub for x86_64 - handled by FPU restore)
pub fn kernel_switch_in_user_vector(_vcpu: &mut crate::arch::vcpu::VCpuState) {
    // Vector state is included in FXRSTOR on x86_64
}
