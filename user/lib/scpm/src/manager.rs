//! Package manager core operations

use crate::archive::PackageArchive;
use crate::package::{Package, PackageMetadata};
use crate::repository::Repository;
use crate::{Config, Error, RepoEntry, RepositoryIndex, Result};
use alloc::{format, string::String, string::ToString, vec::Vec};
use scarlet_std::fs::{self, File};

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
        manager
    }

    pub fn load_registry(&mut self) -> Result<()> {
        Ok(())
    }

    pub fn save_registry(&self) -> Result<()> {
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
        self.save_registry()?;

        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> Result<()> {
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
