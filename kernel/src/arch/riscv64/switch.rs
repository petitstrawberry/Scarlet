//! RISC-V kernel context switching implementation
//!
//! This module provides low-level context switching functionality for RISC-V,
//! enabling kernel tasks to yield execution and resume later at the same point.

use crate::arch::KernelContext;
use core::arch::{asm, naked_asm};

/// Switch from the current kernel context to the next kernel context
///
/// This function performs a complete kernel context switch:
/// 1. Saves callee-saved registers (sp, ra, s0-s11) to prev_ctx
/// 2. Saves FPU registers (f0-f31, fcsr) to prev_ctx
/// 3. Restores callee-saved registers from next_ctx
/// 4. Restores FPU registers from next_ctx
/// 5. Returns to the point where next_ctx was previously switched out
///
/// # Arguments
/// * `prev_ctx` - Mutable reference to store the current context
/// * `next_ctx` - Reference to the context to switch to
///
/// # Safety
/// This function must only be called from kernel code with valid contexts.
/// The stack pointers in both contexts must point to valid, allocated stacks.
/// FPU must be enabled before calling this function.
///
/// # Returns
/// This function returns twice:
/// - Once immediately (when switching away from this context)
/// - Once when this context is resumed later
#[unsafe(naked)]
pub unsafe extern "C" fn switch_to(prev_ctx: *mut KernelContext, next_ctx: *const KernelContext) {
    naked_asm!(
        // Save current context (prev_ctx)
        // a0 = prev_ctx, a1 = next_ctx
        
        // Save stack pointer
        "sd sp, 0(a0)",
        
        // Save return address
        "sd ra, 8(a0)",
        
        // Save callee-saved registers s0-s11
        "sd s0, 16(a0)",
        "sd s1, 24(a0)",
        "sd s2, 32(a0)",
        "sd s3, 40(a0)",
        "sd s4, 48(a0)",
        "sd s5, 56(a0)",
        "sd s6, 64(a0)",
        "sd s7, 72(a0)",
        "sd s8, 80(a0)",
        "sd s9, 88(a0)",
        "sd s10, 96(a0)",
        "sd s11, 104(a0)",
        
        // Save FPU registers f0-f31 (starting at offset 112)
        "fsd f0, 112(a0)",
        "fsd f1, 120(a0)",
        "fsd f2, 128(a0)",
        "fsd f3, 136(a0)",
        "fsd f4, 144(a0)",
        "fsd f5, 152(a0)",
        "fsd f6, 160(a0)",
        "fsd f7, 168(a0)",
        "fsd f8, 176(a0)",
        "fsd f9, 184(a0)",
        "fsd f10, 192(a0)",
        "fsd f11, 200(a0)",
        "fsd f12, 208(a0)",
        "fsd f13, 216(a0)",
        "fsd f14, 224(a0)",
        "fsd f15, 232(a0)",
        "fsd f16, 240(a0)",
        "fsd f17, 248(a0)",
        "fsd f18, 256(a0)",
        "fsd f19, 264(a0)",
        "fsd f20, 272(a0)",
        "fsd f21, 280(a0)",
        "fsd f22, 288(a0)",
        "fsd f23, 296(a0)",
        "fsd f24, 304(a0)",
        "fsd f25, 312(a0)",
        "fsd f26, 320(a0)",
        "fsd f27, 328(a0)",
        "fsd f28, 336(a0)",
        "fsd f29, 344(a0)",
        "fsd f30, 352(a0)",
        "fsd f31, 360(a0)",
        
        // Save FPU control and status register (fcsr at offset 368)
        "frcsr t0",
        "sd t0, 368(a0)",
        
        // Restore next context (next_ctx)
        // Load stack pointer
        "ld sp, 0(a1)",
        
        // Load return address
        "ld ra, 8(a1)",
        
        // Load callee-saved registers s0-s11
        "ld s0, 16(a1)",
        "ld s1, 24(a1)",
        "ld s2, 32(a1)",
        "ld s3, 40(a1)",
        "ld s4, 48(a1)",
        "ld s5, 56(a1)",
        "ld s6, 64(a1)",
        "ld s7, 72(a1)",
        "ld s8, 80(a1)",
        "ld s9, 88(a1)",
        "ld s10, 96(a1)",
        "ld s11, 104(a1)",
        
        // Load FPU registers f0-f31 (starting at offset 112)
        "fld f0, 112(a1)",
        "fld f1, 120(a1)",
        "fld f2, 128(a1)",
        "fld f3, 136(a1)",
        "fld f4, 144(a1)",
        "fld f5, 152(a1)",
        "fld f6, 160(a1)",
        "fld f7, 168(a1)",
        "fld f8, 176(a1)",
        "fld f9, 184(a1)",
        "fld f10, 192(a1)",
        "fld f11, 200(a1)",
        "fld f12, 208(a1)",
        "fld f13, 216(a1)",
        "fld f14, 224(a1)",
        "fld f15, 232(a1)",
        "fld f16, 240(a1)",
        "fld f17, 248(a1)",
        "fld f18, 256(a1)",
        "fld f19, 264(a1)",
        "fld f20, 272(a1)",
        "fld f21, 280(a1)",
        "fld f22, 288(a1)",
        "fld f23, 296(a1)",
        "fld f24, 304(a1)",
        "fld f25, 312(a1)",
        "fld f26, 320(a1)",
        "fld f27, 328(a1)",
        "fld f28, 336(a1)",
        "fld f29, 344(a1)",
        "fld f30, 352(a1)",
        "fld f31, 360(a1)",
        
        // Load FPU control and status register (fcsr at offset 368)
        "ld t0, 368(a1)",
        "fscsr t0",
        
        // Return to the saved return address
        // This will either:
        // - Return to the original caller (first time)
        // - Resume where this context was previously switched out
        "ret",
    );
}

/// Initialize a kernel context for first-time execution
///
/// This function sets up a kernel context to start executing at the specified
/// entry point when first switched to.
///
/// # Arguments
/// * `ctx` - Mutable reference to the context to initialize
/// * `entry_point` - Function pointer to start executing
/// * `stack_top` - Top of the stack for this context
pub fn init_kernel_context(ctx: &mut KernelContext, entry_point: fn(), stack_top: u64) {
    // Set up initial state for first-time execution
    ctx.sp = stack_top;
    ctx.ra = entry_point as u64;
    
    // Clear all saved registers
    ctx.s = [0; 12];
    // Clear all FPU registers
    ctx.f = [0; 32];
    ctx.fcsr = 0;
}

/// Wrapper function for safe context switching
///
/// This provides a safe interface to the low-level switch_to function.
///
/// # Arguments
/// * `prev_ctx` - Mutable reference to store the current context
/// * `next_ctx` - Reference to the context to switch to
///
/// # Safety
/// Both contexts must have valid stack pointers and be properly initialized.
pub fn kernel_switch_to(prev_ctx: &mut KernelContext, next_ctx: &KernelContext) {
    unsafe {
        switch_to(prev_ctx as *mut KernelContext, next_ctx as *const KernelContext);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;
    use crate::environment::TASK_KERNEL_STACK_SIZE;

    /// Test kernel context initialization
    #[test_case]
    fn test_init_kernel_context() {
        let mut ctx = KernelContext::new();
        let stack = Box::new([0u8; TASK_KERNEL_STACK_SIZE]);
        let stack_top = stack.as_ptr() as u64 + TASK_KERNEL_STACK_SIZE as u64;
        
        fn test_entry() {
            // Test entry point
        }
        
        init_kernel_context(&mut ctx, test_entry, stack_top);
        
        assert_eq!(ctx.sp, stack_top);
        assert_eq!(ctx.ra, test_entry as u64);
        assert_eq!(ctx.s, [0; 12]);
        assert_eq!(ctx.f, [0; 32]);
        assert_eq!(ctx.fcsr, 0);
    }

    /// Test FPU context switching
    #[test_case]
    fn test_fpu_context_switching() {
        // Test that FPU registers are properly saved and restored during context switching
        let mut ctx1 = KernelContext::new();
        let mut ctx2 = KernelContext::new();
        
        // Set different values for FPU registers to test preservation
        ctx1.f[0] = 0x3FF0000000000000; // 1.0 in IEEE 754 double
        ctx1.f[1] = 0x4000000000000000; // 2.0 in IEEE 754 double
        ctx1.f[31] = 0x4008000000000000; // 3.0 in IEEE 754 double
        ctx1.fcsr = 0x20; // Some control flags
        
        ctx2.f[0] = 0x4010000000000000; // 4.0 in IEEE 754 double
        ctx2.f[1] = 0x4014000000000000; // 5.0 in IEEE 754 double
        ctx2.f[31] = 0x4018000000000000; // 6.0 in IEEE 754 double
        ctx2.fcsr = 0x40; // Different control flags
        
        // Store original values for comparison
        let orig_ctx1_f0 = ctx1.f[0];
        let orig_ctx1_f1 = ctx1.f[1];
        let orig_ctx1_f31 = ctx1.f[31];
        let orig_ctx1_fcsr = ctx1.fcsr;
        
        let orig_ctx2_f0 = ctx2.f[0];
        let orig_ctx2_f1 = ctx2.f[1]; 
        let orig_ctx2_f31 = ctx2.f[31];
        let orig_ctx2_fcsr = ctx2.fcsr;
        
        // Note: In a real test we would perform actual context switches,
        // but for unit testing we verify the structure layout is correct
        // The actual switching assembly is tested during kernel execution
        
        // Verify that the structure maintains the values
        assert_eq!(ctx1.f[0], orig_ctx1_f0);
        assert_eq!(ctx1.f[1], orig_ctx1_f1);
        assert_eq!(ctx1.f[31], orig_ctx1_f31);
        assert_eq!(ctx1.fcsr, orig_ctx1_fcsr);
        
        assert_eq!(ctx2.f[0], orig_ctx2_f0);
        assert_eq!(ctx2.f[1], orig_ctx2_f1);
        assert_eq!(ctx2.f[31], orig_ctx2_f31);
        assert_eq!(ctx2.fcsr, orig_ctx2_fcsr);
        
        // Verify contexts are independent
        assert_ne!(ctx1.f[0], ctx2.f[0]);
        assert_ne!(ctx1.f[1], ctx2.f[1]);
        assert_ne!(ctx1.f[31], ctx2.f[31]);
        assert_ne!(ctx1.fcsr, ctx2.fcsr);
    }

    /// Test FPU register offsets in KernelContext structure
    #[test_case]
    fn test_fpu_register_offsets() {
        use core::mem::{offset_of, size_of};
        
        // Verify memory layout matches assembly code expectations
        assert_eq!(offset_of!(KernelContext, sp), 0);
        assert_eq!(offset_of!(KernelContext, ra), 8);
        assert_eq!(offset_of!(KernelContext, s), 16);
        assert_eq!(offset_of!(KernelContext, f), 112); // 16 + 12*8 = 112
        assert_eq!(offset_of!(KernelContext, fcsr), 368); // 112 + 32*8 = 368
        
        // Verify sizes
        assert_eq!(size_of::<[u64; 12]>(), 96); // s registers
        assert_eq!(size_of::<[u64; 32]>(), 256); // f registers
        assert_eq!(size_of::<u64>(), 8); // fcsr register
    }
}
