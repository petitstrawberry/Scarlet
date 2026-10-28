//! ABI-neutral, inode-owned extended attributes. Filesystems opt in explicitly.
//! Values are opaque metadata; storing a security label does not enforce it.

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
    InvalidName,
    TooLarge,
    NoSpace,
}

pub fn validate_name(name: &[u8]) -> Result<(), Error> {
    if name.is_empty() || name.len() > MAX_NAME || name.contains(&0) {
        Err(Error::InvalidName)
    } else {
        Ok(())
    }
}
