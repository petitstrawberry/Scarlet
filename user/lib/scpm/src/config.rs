//! SCPM configuration

use alloc::string::String;

/// SCPM configuration
#[derive(Debug, Clone)]
pub struct Config {
    /// Directory for installed packages
    pub installed_dir: String,
    /// Directory for package cache
    pub cache_dir: String,
    /// Directory for binaries
    pub bin_dir: String,
    /// Directory for libraries
    pub lib_dir: String,
    /// Package registry file
    pub registry_file: String,
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}

impl Config {
    /// Create default configuration
    pub fn new() -> Self {
        Self {
            installed_dir: String::from("/var/scpm/installed"),
            cache_dir: String::from("/var/scpm/cache"),
            bin_dir: String::from("/usr/local/bin"),
            lib_dir: String::from("/usr/local/lib"),
            registry_file: String::from("/var/scpm/registry.toml"),
        }
    }

    /// Create configuration with custom paths
    pub fn with_paths(
        installed_dir: String,
        cache_dir: String,
        bin_dir: String,
        lib_dir: String,
        registry_file: String,
    ) -> Self {
        Self {
            installed_dir,
            cache_dir,
            bin_dir,
            lib_dir,
            registry_file,
        }
    }
}
