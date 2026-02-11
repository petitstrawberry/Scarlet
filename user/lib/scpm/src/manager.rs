//! Package manager core operations

use crate::archive::PackageArchive;
use crate::package::{Package, PackageMetadata};
use crate::repository::Repository;
use crate::{Config, Error, RepoEntry, RepositoryIndex, Result};
use alloc::{format, string::String, string::ToString, vec::Vec};
use core::ops::Drop;
use scarlet_std::fs::{self, File};

struct ScpmLock;

impl ScpmLock {
    fn lock() -> Result<Self> {
        let lock_path = "/var/scpm/scpm.lock";

        match File::open(lock_path) {
            Ok(_) => {
                return Err(Error::IoError(
                    "SCPM is busy. If you believe this is an error, \
                    manually remove /var/scpm/scpm.lock"
                        .into(),
                ));
            }
            Err(_) => {}
        }

        let _ = fs::create_directory("/var");
        let _ = fs::create_directory("/var/scpm");
        let mut lock_file = File::create(lock_path)
            .map_err(|e| Error::IoError(format!("Failed to create lock file: {:?}", e)))?;

        lock_file
            .write_all(b"SCPM_LOCK")
            .map_err(|e| Error::IoError(format!("Failed to write lock file: {:?}", e)))?;

        Ok(ScpmLock)
    }
}

impl Drop for ScpmLock {
    fn drop(&mut self) {
        let _ = fs::remove_file("/var/scpm/scpm.lock");
    }
}

#[allow(dead_code)]
pub struct PackageManager {
    config: Config,
    installed_packages: Vec<PackageMetadata>,
    repository: RepositoryIndex,
}

impl PackageManager {
    pub fn new(config: Config) -> Self {
        let manager = Self {
            config,
            installed_packages: Vec::new(),
            repository: RepositoryIndex::new(),
        };
        manager
    }

    pub fn with_default_config() -> Self {
        let default_repo = Repository::new(String::from("file:///var/scpm/repository/"));
        let mut manager = Self::new(Config::default());
        manager.repository.add_repository(default_repo);
        if let Err(e) = manager.load_registry() {
            panic!("Failed to load registry: {:?}", e);
        }
        manager
    }

    pub fn load_registry(&mut self) -> Result<()> {
        let registry_path = "/var/scpm/registry";
        let mut file = match File::open(registry_path) {
            Ok(f) => {
                crate::debug_log!("[SCPM] Registry file opened successfully");
                f
            }
            Err(e) => {
                crate::debug_log!("[SCPM] Failed to open registry file: {:?}", e);
                return Ok(());
            }
        };

        let mut content = Vec::new();
        let mut buffer = [0u8; 4096];
        let mut total_bytes = 0;

        loop {
            match file.read(&mut buffer) {
                Ok(0) => {
                    crate::debug_log!("[SCPM] Read 0 bytes, end of file");
                    break;
                }
                Ok(bytes_read) => {
                    crate::debug_log!("[SCPM] Read {} bytes", bytes_read);
                    total_bytes += bytes_read;
                    content.extend_from_slice(&buffer[..bytes_read]);
                }
                Err(e) => {
                    crate::debug_log!("[SCPM] Error reading file: {:?}", e);
                    break;
                }
            }
        }

        crate::debug_log!("[SCPM] Total bytes read: {}", total_bytes);
        crate::debug_log!("[SCPM] Content length: {}", content.len());

        let content_str = match core::str::from_utf8(&content) {
            Ok(s) => {
                crate::debug_log!("[SCPM] Content decoded as UTF-8, length: {}", s.len());
                s
            }
            Err(e) => {
                crate::debug_log!("[SCPM] Invalid UTF-8 in registry file: {:?}", e);
                return Err(Error::IoError("Invalid UTF-8 in registry file".into()));
            }
        };

        let mut current_pkg_index: Option<usize> = None;
        let mut line_count = 0;

        for line in content_str.lines() {
            line_count += 1;
            let line = line.trim();
            crate::debug_log!("[SCPM] Processing line {}: '{}'", line_count, line);

            if line.is_empty() || line.starts_with('#') {
                crate::debug_log!("[SCPM]   Skipping empty/comment line");
                continue;
            }

            if line.starts_with(' ') {
                crate::debug_log!("[SCPM]   Found file entry");
                if let Some(idx) = current_pkg_index {
                    let file_path = line[1..].trim();
                    crate::debug_log!(
                        "[SCPM]   Adding file '{}' to package at index {}",
                        file_path,
                        idx
                    );
                    if let Some(pkg) = self.installed_packages.get_mut(idx) {
                        pkg.installed_files.push(file_path.to_string());
                    }
                }
            } else if line.contains(':') {
                crate::debug_log!("[SCPM]   Found package entry");
                let parts: Vec<&str> = line.splitn(2, ':').collect();
                if parts.len() < 2 {
                    crate::debug_log!("[SCPM]   Invalid format, skipping");
                    continue;
                }

                let name = parts[0].trim();
                let version = parts[1].trim();
                crate::debug_log!("[SCPM]   Package: {}-{}", name, version);

                let metadata = PackageMetadata {
                    name: name.to_string(),
                    version: version.to_string(),
                    description: String::new(),
                    author: None,
                    homepage: None,
                    bin_name: String::new(),
                    binaries: Vec::new(),
                    libraries: Vec::new(),
                    dependencies: Vec::new(),
                    architecture: String::new(),
                    license: None,
                    installed_files: Vec::new(),
                };

                if !self.is_installed(&metadata.name) {
                    current_pkg_index = Some(self.installed_packages.len());
                    crate::debug_log!(
                        "[SCPM]   Added package at index {}",
                        current_pkg_index.unwrap()
                    );
                    self.installed_packages.push(metadata);
                } else {
                    crate::debug_log!("[SCPM]   Package already installed, skipping");
                }
            }
        }

        crate::debug_log!(
            "[SCPM] Load registry complete. Total packages: {}",
            self.installed_packages.len()
        );
        Ok(())
    }

    pub fn save_registry(&self) -> Result<()> {
        let registry_path = "/var/scpm/registry";
        crate::debug_log!(
            "[SCPM] Saving registry, {} packages",
            self.installed_packages.len()
        );

        let mut content = String::new();

        for (idx, pkg) in self.installed_packages.iter().enumerate() {
            crate::debug_log!(
                "[SCPM]   Package {}: {}-{} ({} files)",
                idx,
                pkg.name,
                pkg.version,
                pkg.installed_files.len()
            );
            if !pkg.installed_files.is_empty() {
                content.push_str(&format!("{}:{}\n", pkg.name, pkg.version));
                for file in &pkg.installed_files {
                    content.push_str(&format!(" {}\n", file.as_str()));
                }
            }
        }

        crate::debug_log!("[SCPM] Creating /var/scpm directory");
        let _ = fs::create_directory("/var/scpm");

        crate::debug_log!("[SCPM] Creating registry file at {}", registry_path);
        let mut file = match File::create(registry_path) {
            Ok(f) => {
                crate::debug_log!("[SCPM] Registry file created successfully");
                f
            }
            Err(e) => {
                crate::debug_log!("[SCPM] Failed to create registry file: {:?}", e);
                return Err(Error::IoError(format!(
                    "Failed to create registry file: {:?}",
                    e
                )));
            }
        };

        crate::debug_log!("[SCPM] Writing {} bytes to registry", content.len());
        match file.write_all(content.as_bytes()) {
            Ok(_) => {
                crate::debug_log!("[SCPM] Registry written successfully");
            }
            Err(e) => {
                crate::debug_log!("[SCPM] Failed to write registry: {:?}", e);
                return Err(Error::IoError(format!(
                    "Failed to write registry file: {:?}",
                    e
                )));
            }
        }

        Ok(())
    }

    pub fn is_installed(&self, name: &str) -> bool {
        self.installed_packages.iter().any(|p| p.name == name)
    }

    pub fn get_installed(&self, name: &str) -> Option<&PackageMetadata> {
        self.installed_packages.iter().find(|p| p.name == name)
    }

    pub fn list_installed(&self) -> &[PackageMetadata] {
        &self.installed_packages
    }

    pub fn search(&self, query: &str) -> Vec<&RepoEntry> {
        self.repository.search_all(query)
    }

    pub fn install(&mut self, package: Package) -> Result<()> {
        let name = package.metadata.name.clone();
        let _version = package.metadata.version.clone();

        if self.is_installed(&name) {
            return Err(Error::PackageAlreadyInstalled(name));
        }

        for dep in &package.metadata.dependencies {
            if !self.is_installed(&dep.name) {
                return Err(Error::DependencyError(format!(
                    "Missing dependency: {}",
                    dep.name
                )));
            }
        }

        self.installed_packages.push(package.metadata.clone());
        self.save_registry()?;
        Ok(())
    }

    pub fn install_from_bytes(&mut self, _name: &str, data: &[u8]) -> Result<()> {
        let _lock = ScpmLock::lock()?;

        let archive = PackageArchive::from_bytes(data)?;
        let mut metadata = archive.metadata.clone();
        let pkg_name = metadata.name.clone();

        if self.is_installed(&pkg_name) {
            return Err(Error::PackageAlreadyInstalled(pkg_name.clone()));
        }

        for dep in &metadata.dependencies {
            if !self.is_installed(&dep.name) {
                return Err(Error::DependencyError(format!(
                    "Missing dependency: {}",
                    dep.name
                )));
            }
        }

        let installed_files = archive.extract_root("scarlet")?;
        metadata.installed_files = installed_files;

        self.installed_packages.push(metadata);
        if let Err(e) = self.save_registry() {
            panic!("Failed to save registry: {:?}", e);
        }

        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> Result<()> {
        let _lock = ScpmLock::lock()?;

        let package = self
            .get_installed(name)
            .ok_or(Error::PackageNotFound(name.to_string()))?;

        for file_path in &package.installed_files {
            let _ = fs::remove_file(file_path);
            let _ = fs::remove_directory(file_path);
        }

        self.installed_packages.retain(|p| p.name != name);
        self.save_registry()?;
        Ok(())
    }

    pub fn resolve_dependencies(&self, package: &Package) -> Vec<String> {
        let mut missing = Vec::new();
        for dep in &package.metadata.dependencies {
            if !self.is_installed(&dep.name) {
                missing.push(dep.name.clone());
            }
        }
        missing
    }
}
