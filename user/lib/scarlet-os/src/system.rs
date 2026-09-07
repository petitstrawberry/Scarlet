//! Running-kernel identification.
//!
//! Distribution names, versions, and release codenames belong in
//! `/etc/os-release`. These queries describe the kernel currently executing
//! the application, independently of the filesystem's distribution metadata.

extern crate alloc;

use alloc::string::String;
use scarlet_sys::{RawKernelInfo, Syscall, syscall2};

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
    use super::{RawKernelInfo, decode_field, decode_kernel_info};

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
