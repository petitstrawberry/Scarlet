//! Package archive operations - STUB ONLY

use crate::package::PackageMetadata;
use crate::{Error, Result};
use alloc::{format, string::String, vec::Vec};

#[derive(Debug)]
pub struct TarEntry {
    pub name: String,
    pub mode: u32,
    pub size: u64,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub struct PackageArchive {
    pub metadata: PackageMetadata,
    pub entries: Vec<TarEntry>,
}

impl PackageArchive {
    pub fn from_bytes(_data: &[u8]) -> Result<Self> {
        Err(Error::InstallationFailed(String::from(
            "Archive parsing not yet implemented",
        )))
    }

    pub fn get_binary(&self, _name: &str) -> Result<&[u8]> {
        for entry in &self.entries {
            if entry.name == _name {
                return Ok(&entry.data);
            }
        }
        Err(Error::IoError(format!(
            "Binary '{}' not found in package",
            _name
        )))
    }

    pub fn get_library(&self, _name: &str) -> Result<&[u8]> {
        for entry in &self.entries {
            if entry.name == _name {
                return Ok(&entry.data);
            }
        }
        Err(Error::IoError(format!(
            "Library '{}' not found in package",
            _name
        )))
    }
}
