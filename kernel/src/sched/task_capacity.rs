//! Bounded native task-capacity policy and control ABI.
//!
//! Capacity is derived from usable physical memory and the kernel's finite
//! virtual stack-slot geometry. Configuration may tighten or raise that policy
//! within the same physical and virtual ceilings.

use crate::arch::Trapframe;
use crate::environment::{KERNEL_KSTACK_SLOTS, PAGE_SIZE, TASK_KERNEL_STACK_SIZE};
use crate::library::std::usercopy::{copy_from_user, copy_to_user};
use crate::task::{Task, mytask};

const ABI_VERSION_V1: u32 = 1;
const LIMITS_V1_SIZE: usize = 32;
const SNAPSHOT_V1_SIZE: usize = 80;
const OP_QUERY: usize = 0;
const OP_CONFIGURE: usize = 1;
const STACK_PHYSICAL_BUDGET_DIVISOR: usize = 32;
const KERNEL_ROLE_MAX: usize = 128;
const BOOTSTRAP_STACK_SLOTS: usize = 1;

/// Runtime task-count and kernel-stack limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskLimits {
    pub max_user: usize,
    pub max_kernel: usize,
    pub stack_bytes: usize,
}

/// Coherent task-capacity policy and usage snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapacitySnapshot {
    pub limits: TaskLimits,
    pub active_user: usize,
    pub active_kernel: usize,
    pub pending_user: usize,
    pub pending_kernel: usize,
    pub stack_slots: usize,
    pub used_stack_slots: usize,
}

/// Derive conservative limits from total usable physical pages.
///
/// Kernel stacks receive one thirty-second of physical RAM, rounded down to a
/// whole stack, while the virtual stack window remains an independent ceiling.
/// One slot is reserved for the bootstrap context. Kernel tasks receive at
/// least one eighth of the remaining policy up to 128 slots; user tasks receive
/// the rest.
pub fn default_limits(total_pages: usize) -> TaskLimits {
    let physical_stack_slots = total_pages.saturating_mul(PAGE_SIZE)
        / STACK_PHYSICAL_BUDGET_DIVISOR
        / TASK_KERNEL_STACK_SIZE;
    default_limits_for_slots(physical_stack_slots)
}

fn default_limits_for_slots(physical_stack_slots: usize) -> TaskLimits {
    let stack_slots = physical_stack_slots.min(KERNEL_KSTACK_SLOTS);
    if stack_slots < BOOTSTRAP_STACK_SLOTS + 2 {
        return TaskLimits {
            max_user: 0,
            max_kernel: 0,
            stack_bytes: 0,
        };
    }

    let kernel_share = core::cmp::max(1, stack_slots / 8);
    let max_kernel =
        core::cmp::min(KERNEL_ROLE_MAX, kernel_share).min(stack_slots - BOOTSTRAP_STACK_SLOTS - 1);
    let max_user = stack_slots - BOOTSTRAP_STACK_SLOTS - max_kernel;
    TaskLimits {
        max_user,
        max_kernel,
        stack_bytes: stack_slots * TASK_KERNEL_STACK_SIZE,
    }
}

/// Validate explicit limits against physical memory and stack-slot geometry.
///
/// Returns the stack-slot budget represented by `limits.stack_bytes`.
pub fn validate(limits: TaskLimits, total_pages: usize) -> Result<usize, &'static str> {
    if limits.max_user == 0 || limits.max_kernel == 0 || limits.stack_bytes == 0 {
        return Err("task role and stack limits must be nonzero");
    }

    let usable_physical_bytes = total_pages
        .checked_mul(PAGE_SIZE)
        .ok_or("usable physical memory size overflow")?;
    if limits.stack_bytes > usable_physical_bytes {
        return Err("task stack budget exceeds usable physical memory");
    }

    let stack_slots = limits.stack_bytes / TASK_KERNEL_STACK_SIZE;
    if stack_slots == 0 {
        return Err("task stack budget is below one kernel stack");
    }
    if stack_slots > KERNEL_KSTACK_SLOTS {
        return Err("task stack budget exceeds virtual stack-slot geometry");
    }

    let roles_with_bootstrap = limits
        .max_user
        .checked_add(limits.max_kernel)
        .and_then(|roles| roles.checked_add(BOOTSTRAP_STACK_SLOTS))
        .ok_or("task role count overflow")?;
    if roles_with_bootstrap > stack_slots {
        return Err("task role limits exceed available stack slots");
    }

    Ok(stack_slots)
}

fn may_configure(task: &Task) -> bool {
    task.credentials
        .read()
        .permits(crate::shield::identity::MANAGE_RESOURCES)
}

fn decode_native_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn decode_native_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn encode_native_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

fn encode_native_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_ne_bytes());
}

fn decode_limits(bytes: &[u8; LIMITS_V1_SIZE]) -> Result<TaskLimits, &'static str> {
    if decode_native_u32(bytes, 0) as usize != LIMITS_V1_SIZE {
        return Err("invalid task-capacity limits size");
    }
    if decode_native_u32(bytes, 4) != ABI_VERSION_V1 {
        return Err("unsupported task-capacity ABI version");
    }
    Ok(TaskLimits {
        max_user: usize::try_from(decode_native_u64(bytes, 8))
            .map_err(|_| "user task limit exceeds usize")?,
        max_kernel: usize::try_from(decode_native_u64(bytes, 16))
            .map_err(|_| "kernel task limit exceeds usize")?,
        stack_bytes: usize::try_from(decode_native_u64(bytes, 24))
            .map_err(|_| "stack budget exceeds usize")?,
    })
}

fn encode_snapshot(snapshot: &CapacitySnapshot) -> Option<[u8; SNAPSHOT_V1_SIZE]> {
    let mut bytes = [0u8; SNAPSHOT_V1_SIZE];
    encode_native_u32(&mut bytes, 0, SNAPSHOT_V1_SIZE as u32);
    encode_native_u32(&mut bytes, 4, ABI_VERSION_V1);
    encode_native_u64(&mut bytes, 8, u64::try_from(snapshot.limits.max_user).ok()?);
    encode_native_u64(
        &mut bytes,
        16,
        u64::try_from(snapshot.limits.max_kernel).ok()?,
    );
    encode_native_u64(
        &mut bytes,
        24,
        u64::try_from(snapshot.limits.stack_bytes).ok()?,
    );
    encode_native_u64(&mut bytes, 32, u64::try_from(snapshot.active_user).ok()?);
    encode_native_u64(&mut bytes, 40, u64::try_from(snapshot.active_kernel).ok()?);
    encode_native_u64(&mut bytes, 48, u64::try_from(snapshot.pending_user).ok()?);
    encode_native_u64(&mut bytes, 56, u64::try_from(snapshot.pending_kernel).ok()?);
    encode_native_u64(&mut bytes, 64, u64::try_from(snapshot.stack_slots).ok()?);
    encode_native_u64(
        &mut bytes,
        72,
        u64::try_from(snapshot.used_stack_slots).ok()?,
    );
    Some(bytes)
}

/// Handle native syscall 55: task-capacity query or atomic configuration.
///
/// Returns zero on success and `usize::MAX` on any failure. Configuration
/// requires `MANAGE_RESOURCES`; the read-only query has no identity shortcut.
pub fn sys_control(frame: &mut Trapframe) -> usize {
    let Some(task) = mytask() else {
        return usize::MAX;
    };
    frame.increment_pc_next(&task);

    let operation = frame.get_arg(0);
    let address = frame.get_arg(1);
    let size = frame.get_arg(2);
    match operation {
        OP_QUERY => {
            if address == 0
                || size != SNAPSHOT_V1_SIZE
                || address.checked_add(SNAPSHOT_V1_SIZE).is_none()
            {
                return usize::MAX;
            }
            let snapshot = crate::sched::scheduler::task_capacity_snapshot();
            let Some(bytes) = encode_snapshot(&snapshot) else {
                return usize::MAX;
            };
            copy_to_user(&task, address, &bytes).map_or(usize::MAX, |_| 0)
        }
        OP_CONFIGURE => {
            if !may_configure(&task)
                || address == 0
                || size != LIMITS_V1_SIZE
                || address.checked_add(LIMITS_V1_SIZE).is_none()
            {
                return usize::MAX;
            }
            let mut bytes = [0u8; LIMITS_V1_SIZE];
            if copy_from_user(&task, address, &mut bytes).is_err() {
                return usize::MAX;
            }
            let limits = match decode_limits(&bytes) {
                Ok(limits) => limits,
                Err(_) => return usize::MAX,
            };
            if validate(limits, crate::mem::pmm::page_counts().0).is_err() {
                return usize::MAX;
            }
            crate::sched::scheduler::configure_task_capacity(limits).map_or(usize::MAX, |_| 0)
        }
        _ => usize::MAX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskType;
    use alloc::string::ToString;

    #[test_case]
    fn large_slot_budgets_use_geometry_instead_of_a_fixed_user_cap() {
        let limits = default_limits_for_slots(KERNEL_KSTACK_SLOTS + 1);
        let total_pages = KERNEL_KSTACK_SLOTS * (TASK_KERNEL_STACK_SIZE / PAGE_SIZE);

        assert!(KERNEL_KSTACK_SLOTS > 1023);
        assert!(limits.max_user > 895);
        assert_eq!(limits.max_kernel, KERNEL_ROLE_MAX);
        assert_eq!(validate(limits, total_pages).unwrap(), KERNEL_KSTACK_SLOTS);
    }

    #[test_case]
    fn largest_addressable_ram_keeps_a_valid_physical_stack_budget() {
        let total_pages = usize::MAX / PAGE_SIZE;
        let limits = default_limits(total_pages);
        assert!(validate(limits, total_pages).is_ok());
        assert!(limits.stack_bytes <= total_pages * PAGE_SIZE / STACK_PHYSICAL_BUDGET_DIVISOR);
    }

    #[test_case]
    fn small_ram_leaves_capacity_for_both_roles() {
        let total_pages = 32 * 1024 * 1024 / PAGE_SIZE;
        let limits = default_limits(total_pages);

        assert!(limits.max_user > 0);
        assert!(limits.max_kernel > 0);
        assert!(limits.max_user < 895);
        assert_eq!(limits.stack_bytes, 16 * TASK_KERNEL_STACK_SIZE);
        assert_eq!(validate(limits, total_pages), Ok(16));
    }

    #[test_case]
    fn explicit_limits_reject_physical_and_geometry_overcommit() {
        let three_stack_pages = 3 * TASK_KERNEL_STACK_SIZE / PAGE_SIZE;
        let limits = TaskLimits {
            max_user: 1,
            max_kernel: 1,
            stack_bytes: 3 * TASK_KERNEL_STACK_SIZE,
        };
        assert_eq!(validate(limits, three_stack_pages), Ok(3));
        assert!(validate(limits, three_stack_pages / 2).is_err());

        let too_many_slots = TaskLimits {
            max_user: 1,
            max_kernel: 1,
            stack_bytes: (KERNEL_KSTACK_SLOTS + 1) * TASK_KERNEL_STACK_SIZE,
        };
        let enough_pages = (KERNEL_KSTACK_SLOTS + 1) * (TASK_KERNEL_STACK_SIZE / PAGE_SIZE);
        assert!(validate(too_many_slots, enough_pages).is_err());
    }

    #[test_case]
    fn reserved_bootstrap_slot_is_always_accounted_for() {
        let limits = TaskLimits {
            max_user: 1,
            max_kernel: 1,
            stack_bytes: 3 * TASK_KERNEL_STACK_SIZE,
        };
        let total_pages = 3 * TASK_KERNEL_STACK_SIZE * STACK_PHYSICAL_BUDGET_DIVISOR / PAGE_SIZE;
        assert_eq!(validate(limits, total_pages), Ok(3));

        assert!(
            validate(
                TaskLimits {
                    max_kernel: 2,
                    ..limits
                },
                total_pages,
            )
            .is_err()
        );
    }

    #[test_case]
    fn invalid_limits_are_rejected() {
        let valid = TaskLimits {
            max_user: 1,
            max_kernel: 1,
            stack_bytes: 3 * TASK_KERNEL_STACK_SIZE,
        };
        let total_pages = 3 * TASK_KERNEL_STACK_SIZE * STACK_PHYSICAL_BUDGET_DIVISOR / PAGE_SIZE;
        assert!(
            validate(
                TaskLimits {
                    max_user: 0,
                    ..valid
                },
                total_pages,
            )
            .is_err()
        );
        assert!(
            validate(
                TaskLimits {
                    max_kernel: 0,
                    ..valid
                },
                total_pages,
            )
            .is_err()
        );
        assert!(
            validate(
                TaskLimits {
                    stack_bytes: 0,
                    ..valid
                },
                total_pages,
            )
            .is_err()
        );
        assert!(
            validate(
                TaskLimits {
                    max_user: usize::MAX,
                    ..valid
                },
                total_pages,
            )
            .is_err()
        );
    }

    #[test_case]
    fn snapshot_wire_record_has_the_published_v1_offsets() {
        let snapshot = CapacitySnapshot {
            limits: TaskLimits {
                max_user: 11,
                max_kernel: 12,
                stack_bytes: 13,
            },
            active_user: 21,
            active_kernel: 22,
            pending_user: 23,
            pending_kernel: 24,
            stack_slots: 25,
            used_stack_slots: 26,
        };
        let bytes = encode_snapshot(&snapshot).unwrap();
        assert_eq!(decode_native_u32(&bytes, 0) as usize, SNAPSHOT_V1_SIZE);
        assert_eq!(decode_native_u32(&bytes, 4), ABI_VERSION_V1);
        assert_eq!(decode_native_u64(&bytes, 8), 11);
        assert_eq!(decode_native_u64(&bytes, 32), 21);
        assert_eq!(decode_native_u64(&bytes, 56), 24);
        assert_eq!(decode_native_u64(&bytes, 72), 26);
    }

    #[test_case]
    fn configuration_authority_is_explicit_and_identity_independent() {
        let task = Task::new("task-capacity-auth".to_string(), 0, TaskType::User);
        assert!(may_configure(&task));
        task.credentials.write().set_authority(0, 0).unwrap();
        assert!(!may_configure(&task));
    }
}
