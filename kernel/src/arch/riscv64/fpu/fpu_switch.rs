use super::super::vcpu::Vcpu;

/// Save user FPU context when switching away from a task in the kernel.
///
/// RISC-V uses FS dirty tracking, so we only save when needed.
#[inline]
pub fn kernel_switch_out_user_fpu(vcpu: &mut Vcpu) {
    if super::is_fpu_dirty() {
        vcpu.fpu_used = true;
        super::enable_fpu();
        unsafe { vcpu.fpu.save() };
        super::mark_fpu_clean();
    }
}

/// Restore user FPU context when resuming a task in the kernel.
///
/// This restores the task's saved context into the architectural registers so
/// subsequent kernel operations (and the eventual user return path) can rely on
/// a consistent state.
#[inline]
pub fn kernel_switch_in_user_fpu(vcpu: &mut Vcpu) {
    if vcpu.fpu_used {
        super::enable_fpu();
        unsafe { vcpu.fpu.restore() };
        super::mark_fpu_clean();
    }
}

/// Handle user vector state on kernel switch-out.
///
/// We avoid saving vregs on every timeslice. If VS is dirty for the current
/// owner, we mark the per-hart owner as dirty so the save can be deferred until
/// another task must overwrite the live vregs.
#[inline]
pub fn kernel_switch_out_user_vector(cpu_id: usize, task_id: usize, vcpu: &mut Vcpu) {
    if super::is_vector_dirty() {
        vcpu.vector_used = true;

        if super::super::get_vector_owner(cpu_id) == task_id {
            super::super::set_vector_owner_dirty(cpu_id, true);
        }

        // Keep VS off while in the kernel unless explicitly needed.
        super::disable_vector();
    }
}
