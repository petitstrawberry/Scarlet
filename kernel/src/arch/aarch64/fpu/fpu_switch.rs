use super::super::vcpu::Vcpu;

/// Save user FPU/SIMD context when switching away from a task in the kernel.
///
/// On AArch64, user FP/SIMD shares the same register file (V0-V31).
#[inline]
pub fn kernel_switch_out_user_fpu(vcpu: &mut Vcpu) {
    if vcpu.fpu_used {
        unsafe { vcpu.fpu.save() };
    }
}

/// Restore user FPU/SIMD context when resuming a task in the kernel.
#[inline]
pub fn kernel_switch_in_user_fpu(vcpu: &mut Vcpu) {
    if vcpu.fpu_used {
        unsafe { vcpu.fpu.restore() };
    }
}

/// AArch64 doesn't have a separate user "vector" context apart from FP/SIMD.
/// This hook is a no-op.
#[inline]
pub fn kernel_switch_out_user_vector(_cpu_id: usize, _task_id: usize, _vcpu: &mut Vcpu) {}
