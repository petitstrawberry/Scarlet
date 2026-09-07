//! Package metadata and structure

use alloc::{format, string::String, vec::Vec};

/// Package metadata from package.toml
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Primary binary name (for single-binary packages)
    pub bin_name: String,
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
    /// List of installed files (tracked after installation)
    pub installed_files: Vec<String>,
}

impl Default for PackageMetadata {
    fn default() -> Self {
        Self {
            name: String::new(),
            version: String::new(),
            description: String::new(),
            author: None,
            homepage: None,
            bin_name: String::new(),
            binaries: Vec::new(),
            libraries: Vec::new(),
            dependencies: Vec::new(),
            architecture: String::from("any"),
            license: None,
            installed_files: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSpec {
    pub path: String,
    pub file_type: FileType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileType {
    Binary,
    Library,
    Config,
    Data,
    Other,
}

/// Package dependency specification
#[derive(Debug, Clone, PartialEq, Eq)]
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
