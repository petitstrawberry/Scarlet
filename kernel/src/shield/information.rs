//! Monotonic disclosure constraints, independent of ABI, UID and Environment.
use crate::task::Task;
pub const HIDE_KERNEL_ADDRESSES: u32 = 1;
pub fn constrain(task: &Task, flags: u32) -> Result<(), &'static str> {
    if flags & !HIDE_KERNEL_ADDRESSES != 0 { return Err("unknown information constraint"); }
    if flags & HIDE_KERNEL_ADDRESSES != 0 { task.execution_restrictions.write().hide_kernel_addresses(); }
    Ok(())
}
pub fn constraints(task: &Task) -> u32 {
    if task.execution_restrictions.read().kernel_addresses_hidden() { HIDE_KERNEL_ADDRESSES } else { 0 }
}
pub fn redact_task(info: &mut scarlet_abi::RawTaskDebugInfoV1) {
    let limit = crate::environment::USER_LOWER_CANONICAL_END as u64;
    if info.task_type == 0 || info.flags & scarlet_abi::TASK_DEBUG_FLAG_PC_PRIVILEGED != 0 || info.observed_pc >= limit {
        info.observed_pc = 0;
        info.flags &= !scarlet_abi::TASK_DEBUG_FLAG_PC_VALID;
    }
    if info.task_type == 0 || info.syscall_pc >= limit { info.syscall_pc = 0; }
}
pub fn redact_cpu(info: &mut scarlet_abi::RawCpuDebugInfoV1) {
    // Untagged breadcrumb context may contain physical or virtual addresses.
    info.breadcrumb_aux = 0; info.breadcrumb_aux2 = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test_case]
    fn privileged_pcs_and_opaque_cpu_breadcrumbs_are_redacted_without_hiding_user_pcs() {
        let mut task = scarlet_abi::RawTaskDebugInfoV1::default();
        task.task_type = 1; task.observed_pc = 0x400000; task.syscall_pc = 0x400100;
        task.flags = scarlet_abi::TASK_DEBUG_FLAG_PC_VALID;
        redact_task(&mut task);
        assert_eq!(task.observed_pc, 0x400000);
        task.flags |= scarlet_abi::TASK_DEBUG_FLAG_PC_PRIVILEGED;
        task.observed_pc = u64::MAX - 4096;
        redact_task(&mut task);
        assert_eq!(task.observed_pc, 0);
        assert_eq!(task.flags & scarlet_abi::TASK_DEBUG_FLAG_PC_VALID, 0);
        assert_eq!(task.syscall_pc, 0x400100);
        task.task_type = 0; redact_task(&mut task); assert_eq!(task.syscall_pc, 0);
        let mut cpu = scarlet_abi::RawCpuDebugInfoV1::default();
        cpu.breadcrumb_phase = 7; cpu.breadcrumb_aux = 0xffff000000001000; cpu.breadcrumb_aux2 = 0x80001000;
        cpu.timer_deadline_ns = 123;
        redact_cpu(&mut cpu);
        assert_eq!((cpu.breadcrumb_aux, cpu.breadcrumb_aux2), (0, 0));
        assert_eq!((cpu.breadcrumb_phase, cpu.timer_deadline_ns), (7, 123));
    }
    #[test_case]
    fn information_constraints_are_monotonic_and_not_syscall_filters() {
        let task = crate::task::new_user_task("disclosure".into(), 0);
        assert!(constrain(&task, 2).is_err());
        assert_eq!(constraints(&task), 0);
        constrain(&task, 1).unwrap(); constrain(&task, 0).unwrap();
        assert_eq!(constraints(&task), 1);
        assert!(!task.execution_restrictions.read().is_constrained());
    }
}
