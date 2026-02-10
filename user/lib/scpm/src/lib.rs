//! Scarlet Package Manager (SCPM) Library
//!
//! SCPM is a minimal package manager for the Scarlet operating system.
//! It handles package installation, removal, and management with a simple
//! archive-based package format.

#![no_std]

extern crate alloc;
extern crate scarlet_std as std;

use alloc::{format, string::String, string::ToString, vec::Vec};

pub mod archive;
pub mod config;
pub mod error;
pub mod manager;
pub mod package;
pub mod repository;

pub use archive::PackageArchive;
pub use config::Config;
pub use error::{Error, Result};
pub use manager::PackageManager;
pub use package::{Dependency, Package, PackageMetadata};
pub use repository::{RepoEntry, Repository, RepositoryIndex};
