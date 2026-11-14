//! ABI-neutral, inode-owned extended attributes. Filesystems opt in explicitly.
//! Values are opaque metadata; storing a security label does not enforce it.

use alloc::vec::Vec;

pub const MAX_NAME: usize = 255;
pub const MAX_VALUE: usize = 65536;
pub const MAX_NODE_BYTES: usize = 128 * 1024;
pub const MAX_NODE_ATTRIBUTES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetMode {
    Upsert,
    Create,
    Replace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported,
    Missing,
    Exists,
    PermissionDenied,
    InvalidName,
    TooLarge,
    NoSpace,
    Io,
}

pub fn validate_name(name: &[u8]) -> Result<(), Error> {
    validate_backend_name(name)?;
    if scarlet_abi::shield::is_file_label_metadata_name(name) {
        Err(Error::PermissionDenied)
    } else {
        Ok(())
    }
}

/// Filesystem metadata backends use this validator directly. The reserved
/// Shield namespace remains inaccessible to public xattr operations.
pub(crate) fn validate_backend_name(name: &[u8]) -> Result<(), Error> {
    if name.is_empty() || name.len() > MAX_NAME || name.contains(&0) {
        Err(Error::InvalidName)
    } else {
        Ok(())
    }
}

pub fn label_metadata_key(domain: &[u8; 16]) -> Vec<u8> {
    scarlet_abi::shield::file_label_metadata_name(domain).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn shield_label_metadata_uses_a_stable_reserved_xattr_name() {
        let domain = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        let key = label_metadata_key(&domain);
        assert_eq!(
            key,
            b"trusted.scarlet.shield.0123456789abcdeffedcba9876543210".to_vec()
        );
        assert_eq!(validate_name(&key), Err(Error::PermissionDenied));
        assert!(validate_backend_name(&key).is_ok());
        assert!(validate_name(b"trusted.scarlet.shieldish.public").is_ok());
    }
}
