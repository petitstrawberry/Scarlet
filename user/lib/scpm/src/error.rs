//! SCPM error types

use alloc::string::String;

/// SCPM error types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Package not found
    PackageNotFound(String),
    /// Package already installed
    PackageAlreadyInstalled(String),
    /// Package metadata is invalid
    InvalidMetadata(String),
    /// IO error
    IoError(String),
    /// Package installation failed
    InstallationFailed(String),
    /// Package removal failed
    RemovalFailed(String),
    /// Dependency resolution failed
    DependencyError(String),
    /// Invalid package format
    InvalidPackageFormat(String),
    /// Network error
    NetworkError(String),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::PackageNotFound(name) => write!(f, "Package '{}' not found", name),
            Error::PackageAlreadyInstalled(name) => {
                write!(f, "Package '{}' is already installed", name)
            }
            Error::InvalidMetadata(msg) => write!(f, "Invalid package metadata: {}", msg),
            Error::IoError(msg) => write!(f, "I/O error: {}", msg),
            Error::InstallationFailed(msg) => write!(f, "Installation failed: {}", msg),
            Error::RemovalFailed(msg) => write!(f, "Removal failed: {}", msg),
            Error::DependencyError(msg) => write!(f, "Dependency error: {}", msg),
            Error::InvalidPackageFormat(msg) => write!(f, "Invalid package format: {}", msg),
            Error::NetworkError(msg) => write!(f, "Network error: {}", msg),
        }
    }
}

/// Result type for SCPM operations
pub type Result<T> = core::result::Result<T, Error>;
