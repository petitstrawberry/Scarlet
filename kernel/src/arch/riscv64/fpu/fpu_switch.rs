use super::super::vcpu::Vcpu;

/// Save user FPU context when switching away from a task in the kernel.
///
/// RISC-V uses FS dirty tracking, so we only save when needed.
#[inline]
pub fn kernel_switch_out_user_fpu(vcpu: &mut Vcpu) {
    if !crate::arch::user_fpu_enabled() {
        return;
    }
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
    if !crate::arch::user_fpu_enabled() {
        return;
    }
    if vcpu.fpu_used {
        super::enable_fpu();
        unsafe { vcpu.fpu.restore() };
        super::mark_fpu_clean();
    }
}

/// Save user vector state while the outgoing task still owns the live registers.
///
/// Unlike the FPU, vector ownership used to defer this save until a later user
/// entry. A task can now become stealable or migratable immediately after this
/// switch-out, so its vector state must be in the task-owned VCPU context before
/// the scheduler releases `running_cpu`.
#[inline]
pub fn kernel_switch_out_user_vector(cpu_id: usize, task_id: usize, vcpu: &mut Vcpu) {
    if !crate::arch::user_vector_enabled() {
        return;
    }
    let owner = super::super::get_vector_owner(cpu_id);
    let dirty = super::is_vector_dirty();
    let save_required = super::super::vector_switch_out_requires_save(owner, task_id, dirty);

    if save_required {
        let vector = vcpu
            .vector
            .as_mut()
            .expect("vector first use must allocate a context before switch-out");

        // `save()` accesses vregs directly and therefore requires VS access.
        super::enable_vector();
        unsafe { vector.save() };
        super::mark_vector_clean();
    }

    // Invalidate even clean ownership. The task may migrate before it runs
    // again, so this hart's registers must never satisfy a later restore skip.
    if owner == task_id {
        super::super::clear_vector_owner(cpu_id);
    }

    // Keep VS off while in the kernel unless explicitly needed.
    super::disable_vector();
}
