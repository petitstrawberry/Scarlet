//! Running-kernel identity and physical-memory information.
//!
//! Distribution names, versions, and release codenames belong in
//! `/etc/os-release`. These queries describe the kernel currently executing
//! the application, independently of the filesystem's distribution metadata.

extern crate alloc;

use alloc::string::String;
use scarlet_abi::{RawTaskCapacityLimitsV1, RawTaskCapacitySnapshotV1};
use scarlet_sys::{RawKernelInfo, Syscall, syscall2, syscall3};

const TASK_CAPACITY_ABI_VERSION_V1: u32 = 1;

/// Identity of the running Scarlet kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelInfo {
    /// Kernel name, such as `Scarlet`.
    pub name: String,
    /// Version compiled from the kernel package's Cargo manifest.
    pub version: String,
    /// Cargo target used to build the running kernel.
    pub target: String,
}

/// Query the identity of the running kernel.
///
/// This query does not read distribution metadata and does not use the Linux
/// compatibility ABI's `uname` values.
///
/// # Returns
///
/// The kernel name, package version, and build target. Returns an error if the
/// syscall is unavailable, fails to copy the record, or returns malformed data.
///
/// # Examples
///
/// ```no_run
/// let info = scarlet_os::system::kernel_info().expect("kernel information");
/// assert!(!info.version.is_empty());
/// ```
pub fn kernel_info() -> Result<KernelInfo, &'static str> {
    let mut raw = RawKernelInfo {
        name: [0; 32],
        version: [0; 64],
        target: [0; 64],
    };
    // SAFETY: raw is an initialized, exclusively borrowed output record with
    // the exact ABI layout and capacity. The kernel does not retain its pointer.
    let result = unsafe {
        syscall2(
            Syscall::GetKernelInfo,
            core::ptr::from_mut(&mut raw) as usize,
            core::mem::size_of::<RawKernelInfo>(),
        )
    };
    if result != core::mem::size_of::<RawKernelInfo>() {
        return Err("Kernel information unavailable");
    }
    decode_kernel_info(&raw)
}

/// Physical pages currently managed by the kernel allocator.
/// Reserved RAM and MMIO are excluded; free memory is an instantaneous snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryInfo {
    pub page_size: u64,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

/// Explicit limits for user and kernel task roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskCapacityLimits {
    pub max_user_tasks: usize,
    pub max_kernel_tasks: usize,
    pub max_stack_bytes: usize,
}

/// Current task-capacity policy and scheduler usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskCapacitySnapshot {
    pub limits: TaskCapacityLimits,
    pub active_user_tasks: usize,
    pub active_kernel_tasks: usize,
    pub pending_user_tasks: usize,
    pub pending_kernel_tasks: usize,
    pub stack_slots: usize,
    pub used_stack_slots: usize,
}

/// Read physical-memory capacity and current free bytes through the native ABI.
/// The returned snapshot does not reserve memory or guarantee a later allocation.
pub fn memory_info() -> Result<MemoryInfo, &'static str> {
    let mut raw = scarlet_abi::RawMemoryInfo::default();
    // SAFETY: raw is initialized, exclusively borrowed, and has the exact
    // output layout/capacity. The synchronous kernel query retains no pointer.
    let result = unsafe {
        syscall2(
            Syscall::GetMemoryInfo,
            core::ptr::from_mut(&mut raw) as usize,
            core::mem::size_of_val(&raw),
        )
    };
    if result != core::mem::size_of_val(&raw) {
        return Err("Memory information unavailable");
    }
    decode_memory_info(&raw)
}

/// Query the current task-capacity policy and usage.
pub fn task_capacity() -> Result<TaskCapacitySnapshot, &'static str> {
    let mut raw = RawTaskCapacitySnapshotV1 {
        size: core::mem::size_of::<RawTaskCapacitySnapshotV1>() as u32,
        version: TASK_CAPACITY_ABI_VERSION_V1,
        ..RawTaskCapacitySnapshotV1::default()
    };
    // SAFETY: raw is an initialized, exclusively borrowed output record with
    // the exact ABI layout. The synchronous kernel query retains no pointer.
    let result = unsafe {
        syscall3(
            Syscall::TaskCapacityControl,
            0,
            core::ptr::from_mut(&mut raw) as usize,
            core::mem::size_of_val(&raw),
        )
    };
    if result != 0 {
        return Err("Task capacity query failed");
    }
    decode_task_capacity_snapshot(&raw)
}

/// Atomically configure task-capacity limits within the kernel's physical and
/// virtual stack ceilings. Failure leaves the previous policy unchanged.
pub fn configure_task_capacity(limits: TaskCapacityLimits) -> Result<(), &'static str> {
    let raw = encode_task_capacity_limits(limits);
    // SAFETY: raw is an initialized, readable input record with the exact ABI
    // layout. The synchronous kernel control copies it before returning.
    let result = unsafe {
        syscall3(
            Syscall::TaskCapacityControl,
            1,
            core::ptr::from_ref(&raw) as usize,
            core::mem::size_of_val(&raw),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err("Task capacity configuration denied or invalid")
    }
}

fn encode_task_capacity_limits(limits: TaskCapacityLimits) -> RawTaskCapacityLimitsV1 {
    RawTaskCapacityLimitsV1 {
        size: core::mem::size_of::<RawTaskCapacityLimitsV1>() as u32,
        version: TASK_CAPACITY_ABI_VERSION_V1,
        max_user_tasks: limits.max_user_tasks as u64,
        max_kernel_tasks: limits.max_kernel_tasks as u64,
        max_stack_bytes: limits.max_stack_bytes as u64,
    }
}

fn decode_task_capacity_snapshot(
    raw: &RawTaskCapacitySnapshotV1,
) -> Result<TaskCapacitySnapshot, &'static str> {
    if raw.size as usize != core::mem::size_of::<RawTaskCapacitySnapshotV1>()
        || raw.version != TASK_CAPACITY_ABI_VERSION_V1
    {
        return Err("Invalid task capacity snapshot header");
    }
    Ok(TaskCapacitySnapshot {
        limits: TaskCapacityLimits {
            max_user_tasks: usize::try_from(raw.max_user_tasks)
                .map_err(|_| "Task capacity value exceeds usize")?,
            max_kernel_tasks: usize::try_from(raw.max_kernel_tasks)
                .map_err(|_| "Task capacity value exceeds usize")?,
            max_stack_bytes: usize::try_from(raw.max_stack_bytes)
                .map_err(|_| "Task capacity value exceeds usize")?,
        },
        active_user_tasks: usize::try_from(raw.active_user_tasks)
            .map_err(|_| "Task capacity value exceeds usize")?,
        active_kernel_tasks: usize::try_from(raw.active_kernel_tasks)
            .map_err(|_| "Task capacity value exceeds usize")?,
        pending_user_tasks: usize::try_from(raw.pending_user_tasks)
            .map_err(|_| "Task capacity value exceeds usize")?,
        pending_kernel_tasks: usize::try_from(raw.pending_kernel_tasks)
            .map_err(|_| "Task capacity value exceeds usize")?,
        stack_slots: usize::try_from(raw.stack_slots)
            .map_err(|_| "Task capacity value exceeds usize")?,
        used_stack_slots: usize::try_from(raw.used_stack_slots)
            .map_err(|_| "Task capacity value exceeds usize")?,
    })
}

fn decode_memory_info(raw: &scarlet_abi::RawMemoryInfo) -> Result<MemoryInfo, &'static str> {
    if !raw.page_size.is_power_of_two() || raw.total_pages == 0 || raw.free_pages > raw.total_pages
    {
        return Err("Invalid memory information");
    }
    Ok(MemoryInfo {
        page_size: raw.page_size,
        total_bytes: raw
            .total_pages
            .checked_mul(raw.page_size)
            .ok_or("Memory size overflow")?,
        free_bytes: raw
            .free_pages
            .checked_mul(raw.page_size)
            .ok_or("Memory size overflow")?,
    })
}

fn decode_kernel_info(raw: &RawKernelInfo) -> Result<KernelInfo, &'static str> {
    Ok(KernelInfo {
        name: decode_field(&raw.name)?,
        version: decode_field(&raw.version)?,
        target: decode_field(&raw.target)?,
    })
}

fn decode_field(bytes: &[u8]) -> Result<String, &'static str> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .filter(|end| *end > 0)
        .ok_or("Invalid kernel information")?;
    core::str::from_utf8(&bytes[..end])
        .map(String::from)
        .map_err(|_| "Invalid kernel information")
}

#[cfg(test)]
mod tests {
    use super::{
        RawKernelInfo, RawTaskCapacityLimitsV1, RawTaskCapacitySnapshotV1,
        decode_field, decode_kernel_info, decode_task_capacity_snapshot,
        encode_task_capacity_limits,
    };

    #[test]
    fn task_capacity_v1_layouts_and_decoder_are_fixed_width() {
        assert_eq!(core::mem::size_of::<RawTaskCapacityLimitsV1>(), 32);
        assert_eq!(core::mem::align_of::<RawTaskCapacityLimitsV1>(), 8);
        assert_eq!(core::mem::size_of::<RawTaskCapacitySnapshotV1>(), 80);
        assert_eq!(core::mem::align_of::<RawTaskCapacitySnapshotV1>(), 8);

        let raw = RawTaskCapacitySnapshotV1 {
            size: 80,
            version: 1,
            max_user_tasks: 11,
            max_kernel_tasks: 12,
            max_stack_bytes: 13,
            active_user_tasks: 21,
            active_kernel_tasks: 22,
            pending_user_tasks: 23,
            pending_kernel_tasks: 24,
            stack_slots: 25,
            used_stack_slots: 26,
        };
        let snapshot = decode_task_capacity_snapshot(&raw).unwrap();
        assert_eq!(snapshot.limits.max_user_tasks, 11);
        assert_eq!(snapshot.active_kernel_tasks, 22);
        assert_eq!(snapshot.used_stack_slots, 26);

        assert!(decode_task_capacity_snapshot(&RawTaskCapacitySnapshotV1 {
            version: 2,
            ..raw
        })
        .is_err());
        assert!(decode_task_capacity_snapshot(&RawTaskCapacitySnapshotV1 {
            size: 72,
            ..raw
        })
        .is_err());
    }

    #[test]
    fn task_capacity_limits_encode_uses_the_raw_v1_record() {
        let raw = encode_task_capacity_limits(super::TaskCapacityLimits {
            max_user_tasks: 895,
            max_kernel_tasks: 128,
            max_stack_bytes: 64 * 1024 * 1024,
        });
        assert_eq!(raw.size, 32);
        assert_eq!(raw.version, 1);
        assert_eq!(raw.max_user_tasks, 895);
        assert_eq!(raw.max_kernel_tasks, 128);
        assert_eq!(raw.max_stack_bytes, 64 * 1024 * 1024);
    }

    #[test]
    fn memory_snapshot_reports_bytes_and_rejects_invalid_capacity() {
        let mut raw = scarlet_abi::RawMemoryInfo {
            page_size: 4096,
            total_pages: 2_097_152,
            free_pages: 1_048_576,
        };
        let info = super::decode_memory_info(&raw).unwrap();
        assert_eq!(info.total_bytes, 8 * 1024 * 1024 * 1024);
        assert_eq!(info.free_bytes, 4 * 1024 * 1024 * 1024);
        raw.free_pages = raw.total_pages + 1;
        assert!(super::decode_memory_info(&raw).is_err());
        raw.free_pages = 0;
        raw.page_size = 0;
        assert!(super::decode_memory_info(&raw).is_err());
        raw.page_size = 4096;
        raw.total_pages = u64::MAX;
        assert!(super::decode_memory_info(&raw).is_err());
    }

    #[test]
    fn kernel_info_layout_matches_the_syscall_record() {
        assert_eq!(core::mem::size_of::<RawKernelInfo>(), 160);
        assert_eq!(core::mem::align_of::<RawKernelInfo>(), 1);
        assert_eq!(core::mem::offset_of!(RawKernelInfo, name), 0);
        assert_eq!(core::mem::offset_of!(RawKernelInfo, version), 32);
        assert_eq!(core::mem::offset_of!(RawKernelInfo, target), 96);
    }

    #[test]
    fn decodes_kernel_identity_independently_of_distribution_version() {
        let mut raw = RawKernelInfo {
            name: [0; 32],
            version: [0; 64],
            target: [0; 64],
        };
        raw.name[..7].copy_from_slice(b"Scarlet");
        raw.version[..5].copy_from_slice(b"1.2.3");
        let target = b"aarch64-unknown-none-elf";
        raw.target[..target.len()].copy_from_slice(target);

        let info = decode_kernel_info(&raw).expect("valid kernel identity");
        assert_eq!(info.name, "Scarlet");
        assert_eq!(info.version, "1.2.3");
        assert_eq!(info.target, "aarch64-unknown-none-elf");
    }

    #[test]
    fn rejects_empty_unterminated_and_invalid_utf8_fields() {
        assert!(decode_field(&[0; 32]).is_err());
        assert!(decode_field(&[b'x'; 64]).is_err());
        assert!(decode_field(&[0xff, 0]).is_err());
    }
}
