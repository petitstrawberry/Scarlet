//! Package metadata and structure

#[allow(unused_imports)]
use alloc::{format, string::String, vec::Vec};
use serde::{Deserialize, Serialize};

/// Package metadata from package.toml
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PackageMetadata {
    /// Package name
    pub name: String,
    /// Package version (semantic versioning)
    pub version: String,
    /// Short description
    pub description: String,
    /// Author/maintainer
    pub author: Option<String>,
    /// Homepage URL
    pub homepage: Option<String>,
    /// List of binaries to install
    pub binaries: Vec<String>,
    /// Optional shared libraries
    pub libraries: Vec<String>,
    /// Package dependencies
    pub dependencies: Vec<Dependency>,
    /// Architecture (riscv64, aarch64, or "any")
    pub architecture: String,
    /// License
    pub license: Option<String>,
}

/// Package dependency specification
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Dependency {
    /// Package name
    pub name: String,
    /// Version constraint (optional)
    pub version: Option<String>,
}

/// Package information including metadata
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// Package metadata
    pub metadata: PackageMetadata,
    /// Installation path
    pub install_path: Option<String>,
}

impl Package {
    /// Create a new package from metadata
    pub fn new(metadata: PackageMetadata) -> Self {
        Self {
            metadata,
            install_path: None,
        }
    }

    /// Get the package identifier (name-version)
    pub fn id(&self) -> String {
        format!("{}-{}", self.metadata.name, self.metadata.version)
    }

    /// Get the archive filename
    pub fn archive_filename(&self) -> String {
        format!("{}.scarlet", self.id())
    }
}
