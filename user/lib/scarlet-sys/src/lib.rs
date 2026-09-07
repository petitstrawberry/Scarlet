//! Unsafe Scarlet Native syscall bindings.
//!
//! This crate is the raw syscall layer for Scarlet userland. It is deliberately
//! small: syscall numbers and ABI types live in `scarlet-abi`, while safe
//! object wrappers live above this crate.
//!
//! # Safety boundary
//!
//! These functions accept untyped arguments and do not validate Rust ownership,
//! pointer validity, or aliasing. Kernel address checks do not establish Rust
//! memory safety: an otherwise valid request can unmap a live reference, close
//! an owning wrapper's handle, or overwrite borrowed memory. Prefer the typed
//! operations in `scarlet-os`.
//!
//! Every call must use the selected operation's exact ABI and argument count.
//! Input/output pointers must have the required layout, alignment, permissions
//! and lifetime, including any period for which the kernel retains them.
//! Writes require exclusive access; handle transfer, mapping replacement and
//! thread/TLS operations must preserve all live Rust ownership invariants.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

pub use scarlet_abi::{
    CPU_DEBUG_FLAG_CURRENT_TASK_VALID, CPU_DEBUG_FLAG_IDLE, CPU_DEBUG_FLAG_PENDING_RESCHEDULE,
    CPU_DEBUG_FLAG_TIMER_ARMED, CPU_DEBUG_INFO_VERSION_V1, ERRNO_EADDRINUSE, ERRNO_EADDRNOTAVAIL,
    ERRNO_EAGAIN, ERRNO_EBADF, ERRNO_ECONNABORTED, ERRNO_ECONNREFUSED, ERRNO_ECONNRESET,
    ERRNO_EINTR, ERRNO_EINVAL, ERRNO_EIO, ERRNO_EISCONN, ERRNO_EMSGSIZE, ERRNO_ENETUNREACH,
    ERRNO_ENOTCONN, ERRNO_EOPNOTSUPP, ERRNO_EPROTONOSUPPORT, ERRNO_ETIMEDOUT,
    FILE_PERMISSION_EXECUTE, FILE_PERMISSION_READ, FILE_PERMISSION_WRITE, FILE_TYPE_BLOCK_DEVICE,
    FILE_TYPE_CHAR_DEVICE, FILE_TYPE_DIRECTORY, FILE_TYPE_PIPE, FILE_TYPE_REGULAR,
    FILE_TYPE_SOCKET, FILE_TYPE_SYMLINK, FILE_TYPE_UNKNOWN, GET_RANDOM_FLAG_REQUIRE_ENTROPY, Pid,
    RAW_SCHEDULER_ATTR_V1_SIZE, RAW_SCHEDULER_STATE_V1_SIZE, RawCpuDebugInfoV1, RawFileMetadata,
    RawHandle, RawKernelInfo, RawSchedulerAttrV1, RawSchedulerResult, RawSchedulerStateV1,
    RawSchedulerStatus, RawTaskDeadlineParams, RawTaskDebugInfoV1, SCHED_AFFINITY_ANY,
    SCHED_AFFINITY_MASK, SCHED_AFFINITY_SINGLE, SCHED_ATTR_FLAGS_NONE, SCHED_CPU_ID_NONE,
    SCHED_NICE_MAX, SCHED_NICE_MIN, SCHED_POLICY_DEADLINE, SCHED_POLICY_FAIR, SCHED_UTIL_SCALE,
    SCHEDULER_CONTROL_VERSION_V1, SCTL_SOCKET_GET_NONBLOCK, SCTL_SOCKET_GET_READ_TIMEOUT_MS,
    SCTL_SOCKET_GET_WRITE_TIMEOUT_MS, SCTL_SOCKET_SET_NONBLOCK, SCTL_SOCKET_SET_READ_TIMEOUT_MS,
    SCTL_SOCKET_SET_WRITE_TIMEOUT_MS, SCTL_SOCKET_TAKE_ERROR, Syscall, TASK_DEBUG_FLAG_DEADLINE,
    TASK_DEBUG_FLAG_DEADLINE_THROTTLED, TASK_DEBUG_FLAG_DEADLINE_UNAVAILABLE,
    TASK_DEBUG_FLAG_PC_PRIVILEGED, TASK_DEBUG_FLAG_PC_VALID, TASK_DEBUG_FLAG_SOFTWARE_TIMER_ARMED,
    TASK_DEBUG_FLAG_SYSCALL_ACTIVE, TASK_DEBUG_FLAG_SYSCALL_VALID, TASK_DEBUG_INFO_VERSION_V1, Tid,
};

#[cfg(target_arch = "aarch64")]
#[path = "arch/aarch64.rs"]
mod arch;
#[cfg(target_arch = "riscv64")]
#[path = "arch/riscv64.rs"]
mod arch;

/// Invoke a Scarlet Native syscall with no arguments.
///
/// # Arguments
///
/// * `syscall` - Operation whose ABI takes 0 arguments.
///
/// # Returns
///
/// The unmodified kernel result; error encoding depends on the operation.
///
/// # Safety
///
/// The caller must satisfy the crate's [safety boundary](crate#safety-boundary),
/// including the operation-specific pointer, ownership and lifetime rules.
///
/// ```compile_fail,E0133
/// scarlet_sys::syscall0(scarlet_sys::Syscall::Getpid);
/// ```
pub unsafe fn syscall0(syscall: Syscall) -> usize {
    // SAFETY: The caller supplies the selected syscall's complete safety contract.
    unsafe { arch::syscall0(syscall) }
}

/// Invoke a Scarlet Native syscall with one argument.
///
/// # Arguments
///
/// * `syscall` - Operation whose ABI takes 1 argument.
/// * `arg1` - Untyped argument 1, interpreted by the selected operation.
///
/// # Returns
///
/// The unmodified kernel result; error encoding depends on the operation.
///
/// # Safety
///
/// The caller must satisfy the crate's [safety boundary](crate#safety-boundary),
/// including the operation-specific pointer, ownership and lifetime rules.
///
/// ```compile_fail,E0133
/// scarlet_sys::syscall1(scarlet_sys::Syscall::Getpid, 0);
/// ```
pub unsafe fn syscall1(syscall: Syscall, arg1: usize) -> usize {
    // SAFETY: The caller supplies the selected syscall's complete safety contract.
    unsafe { arch::syscall1(syscall, arg1) }
}

/// Invoke a Scarlet Native syscall with two arguments.
///
/// # Arguments
///
/// * `syscall` - Operation whose ABI takes 2 arguments.
/// * `arg1` - Untyped argument 1, interpreted by the selected operation.
/// * `arg2` - Untyped argument 2, interpreted by the selected operation.
///
/// # Returns
///
/// The unmodified kernel result; error encoding depends on the operation.
///
/// # Safety
///
/// The caller must satisfy the crate's [safety boundary](crate#safety-boundary),
/// including the operation-specific pointer, ownership and lifetime rules.
///
/// ```compile_fail,E0133
/// scarlet_sys::syscall2(scarlet_sys::Syscall::Getpid, 0, 0);
/// ```
pub unsafe fn syscall2(syscall: Syscall, arg1: usize, arg2: usize) -> usize {
    // SAFETY: The caller supplies the selected syscall's complete safety contract.
    unsafe { arch::syscall2(syscall, arg1, arg2) }
}

/// Invoke a Scarlet Native syscall with three arguments.
///
/// # Arguments
///
/// * `syscall` - Operation whose ABI takes 3 arguments.
/// * `arg1` - Untyped argument 1, interpreted by the selected operation.
/// * `arg2` - Untyped argument 2, interpreted by the selected operation.
/// * `arg3` - Untyped argument 3, interpreted by the selected operation.
///
/// # Returns
///
/// The unmodified kernel result; error encoding depends on the operation.
///
/// # Safety
///
/// The caller must satisfy the crate's [safety boundary](crate#safety-boundary),
/// including the operation-specific pointer, ownership and lifetime rules.
///
/// ```compile_fail,E0133
/// scarlet_sys::syscall3(scarlet_sys::Syscall::Getpid, 0, 0, 0);
/// ```
pub unsafe fn syscall3(syscall: Syscall, arg1: usize, arg2: usize, arg3: usize) -> usize {
    // SAFETY: The caller supplies the selected syscall's complete safety contract.
    unsafe { arch::syscall3(syscall, arg1, arg2, arg3) }
}

/// Invoke a Scarlet Native syscall with four arguments.
///
/// # Arguments
///
/// * `syscall` - Operation whose ABI takes 4 arguments.
/// * `arg1` - Untyped argument 1, interpreted by the selected operation.
/// * `arg2` - Untyped argument 2, interpreted by the selected operation.
/// * `arg3` - Untyped argument 3, interpreted by the selected operation.
/// * `arg4` - Untyped argument 4, interpreted by the selected operation.
///
/// # Returns
///
/// The unmodified kernel result; error encoding depends on the operation.
///
/// # Safety
///
/// The caller must satisfy the crate's [safety boundary](crate#safety-boundary),
/// including the operation-specific pointer, ownership and lifetime rules.
///
/// ```compile_fail,E0133
/// scarlet_sys::syscall4(scarlet_sys::Syscall::Getpid, 0, 0, 0, 0);
/// ```
pub unsafe fn syscall4(
    syscall: Syscall,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
) -> usize {
    // SAFETY: The caller supplies the selected syscall's complete safety contract.
    unsafe { arch::syscall4(syscall, arg1, arg2, arg3, arg4) }
}

/// Invoke a Scarlet Native syscall with five arguments.
///
/// # Arguments
///
/// * `syscall` - Operation whose ABI takes 5 arguments.
/// * `arg1` - Untyped argument 1, interpreted by the selected operation.
/// * `arg2` - Untyped argument 2, interpreted by the selected operation.
/// * `arg3` - Untyped argument 3, interpreted by the selected operation.
/// * `arg4` - Untyped argument 4, interpreted by the selected operation.
/// * `arg5` - Untyped argument 5, interpreted by the selected operation.
///
/// # Returns
///
/// The unmodified kernel result; error encoding depends on the operation.
///
/// # Safety
///
/// The caller must satisfy the crate's [safety boundary](crate#safety-boundary),
/// including the operation-specific pointer, ownership and lifetime rules.
///
/// ```compile_fail,E0133
/// scarlet_sys::syscall5(scarlet_sys::Syscall::Getpid, 0, 0, 0, 0, 0);
/// ```
pub unsafe fn syscall5(
    syscall: Syscall,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
) -> usize {
    // SAFETY: The caller supplies the selected syscall's complete safety contract.
    unsafe { arch::syscall5(syscall, arg1, arg2, arg3, arg4, arg5) }
}

/// Invoke a Scarlet Native syscall with six arguments.
///
/// # Arguments
///
/// * `syscall` - Operation whose ABI takes 6 arguments.
/// * `arg1` - Untyped argument 1, interpreted by the selected operation.
/// * `arg2` - Untyped argument 2, interpreted by the selected operation.
/// * `arg3` - Untyped argument 3, interpreted by the selected operation.
/// * `arg4` - Untyped argument 4, interpreted by the selected operation.
/// * `arg5` - Untyped argument 5, interpreted by the selected operation.
/// * `arg6` - Untyped argument 6, interpreted by the selected operation.
///
/// # Returns
///
/// The unmodified kernel result; error encoding depends on the operation.
///
/// # Safety
///
/// The caller must satisfy the crate's [safety boundary](crate#safety-boundary),
/// including the operation-specific pointer, ownership and lifetime rules.
///
/// ```compile_fail,E0133
/// scarlet_sys::syscall6(scarlet_sys::Syscall::Getpid, 0, 0, 0, 0, 0, 0);
/// ```
pub unsafe fn syscall6(
    syscall: Syscall,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
    arg6: usize,
) -> usize {
    // SAFETY: The caller supplies the selected syscall's complete safety contract.
    unsafe { arch::syscall6(syscall, arg1, arg2, arg3, arg4, arg5, arg6) }
}
