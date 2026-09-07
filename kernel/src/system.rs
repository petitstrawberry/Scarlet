//! Identity of the running kernel, independent of distribution metadata.

use crate::arch::Trapframe;
use crate::library::std::usercopy::copy_to_user;
use crate::task::mytask;

/// Name of this kernel implementation.
pub const KERNEL_NAME: &str = "Scarlet";
/// Version of the kernel package, embedded when the kernel is compiled.
pub const KERNEL_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Cargo target used to build this kernel.
pub const KERNEL_TARGET: &str = env!("TARGET");

// Keep this byte-only layout identical to scarlet_abi::RawKernelInfo. It has
// no padding, so every byte copied to userspace is initialized.
#[repr(C)]
struct KernelInfo {
    name: [u8; 32],
    version: [u8; 64],
    target: [u8; 64],
}

const _: [(); 160] = [(); core::mem::size_of::<KernelInfo>()];
const _: [(); 1] = [(); core::mem::align_of::<KernelInfo>()];

static KERNEL_INFO: KernelInfo = KernelInfo {
    name: field(KERNEL_NAME),
    version: field(KERNEL_VERSION),
    target: field(KERNEL_TARGET),
};

const fn field<const N: usize>(value: &str) -> [u8; N] {
    let bytes = value.as_bytes();
    assert!(
        bytes.len() < N,
        "kernel identity field exceeds ABI capacity"
    );
    let mut output = [0; N];
    let mut index = 0;
    while index < bytes.len() {
        output[index] = bytes[index];
        index += 1;
    }
    output
}

/// Copy the running-kernel identity into a userspace output record.
///
/// # Arguments
///
/// * `trapframe` - Syscall registers: argument 0 is the writable output address;
///   argument 1 is its capacity in bytes. The output uses the 160-byte
///   `scarlet_abi::RawKernelInfo` layout.
///
/// # Returns
///
/// The record size on success, or `usize::MAX` for an invalid address, an
/// undersized buffer, or a failed copy. Output must be ignored on failure.
/// Bytes beyond the record are untouched and the pointer is not retained.
pub fn sys_get_kernel_info(trapframe: &mut Trapframe) -> usize {
    let Some(task) = mytask() else {
        return usize::MAX;
    };
    trapframe.increment_pc_next(&task);

    let address = trapframe.get_arg(0);
    let capacity = trapframe.get_arg(1);
    let size = core::mem::size_of::<KernelInfo>();
    if address == 0 || capacity < size || address.checked_add(size).is_none() {
        return usize::MAX;
    }

    // SAFETY: KernelInfo consists solely of initialized u8 arrays, has no
    // padding, and the immutable static outlives this synchronous copy.
    let bytes = unsafe {
        core::slice::from_raw_parts(core::ptr::from_ref(&KERNEL_INFO).cast::<u8>(), size)
    };
    match copy_to_user(&task, address, bytes) {
        Ok(()) => size,
        Err(_) => usize::MAX,
    }
}
