use crate::package::PackageMetadata;
use crate::{Error, Result};
use alloc::{format, string::String, string::ToString, vec::Vec};
use scarlet_std::fs::{self, File};
use scarlet_std::io::Write;

#[derive(Debug)]
pub struct TarEntry {
    pub name: String,
    pub mode: u32,
    pub size: u64,
    pub is_file: bool,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub struct PackageArchive {
    pub metadata: PackageMetadata,
    pub entries: Vec<TarEntry>,
}

impl PackageArchive {
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let mut entries = Vec::new();
        let metadata = PackageMetadata::default();

        Ok(PackageArchive { metadata, entries })
    }

    pub fn get_binary(&self, name: &str) -> Result<&[u8]> {
        for entry in &self.entries {
            if entry.name == name && entry.is_file {
                return Ok(&entry.data);
            }
        }
        Err(Error::IoError(format!(
            "Binary '{}' not found in package",
            name
        )))
    }

    pub fn get_library(&self, name: &str) -> Result<&[u8]> {
        for entry in &self.entries {
            if entry.name == name && entry.is_file {
                return Ok(&entry.data);
            }
        }
        Err(Error::IoError(format!(
            "Library '{}' not found in package",
            name
        )))
    }

    pub fn extract_to(&self, dest_dir: &str) -> Result<()> {
        let _ = fs::create_directory(dest_dir);

        for entry in &self.entries {
            let full_path = if dest_dir.ends_with('/') {
                format!("{}{}", dest_dir, entry.name)
            } else {
                format!("{}/{}", dest_dir, entry.name)
            };

            if entry.is_file {
                if let Some(parent) = get_parent_path(&full_path) {
                    let parent_path = format!("{}/{}", dest_dir, parent);
                    let _ = fs::create_directory(&parent_path);
                }
                write_file(&full_path, &entry.data)?;
                if entry.mode & 0o111 != 0 {
                    mark_executable(&full_path)?;
                }
            } else if entry.is_dir {
                let _ = fs::create_directory(&full_path);
            } else if entry.is_symlink {
                let target = String::from_utf8_lossy(&entry.data).to_string();
                let _ = fs::create_symlink(&full_path, &target);
            }
        }

        Ok(())
    }

    pub fn extract_root(&self, root_prefix: &str) -> Result<Vec<String>> {
        let mut installed_files = Vec::new();
        let prefix = format!("{}/", root_prefix);

        for entry in &self.entries {
            if !entry.name.starts_with(&prefix) {
                continue;
            }

            let dest_path = &entry.name[prefix.len()..];
            if dest_path.is_empty() {
                continue;
            }

            let full_path = format!("/ {}", dest_path);
            let full_path = full_path.replace("/ ", "/");

            if entry.is_file {
                if let Some(parent) = get_parent_path(&full_path) {
                    let _ = fs::create_directory(parent);
                }

                if file_exists(&full_path) {
                    return Err(Error::IoError(format!(
                        "File '{}' already exists",
                        full_path
                    )));
                }

                write_file(&full_path, &entry.data)?;
                installed_files.push(full_path.clone());

                if entry.mode & 0o111 != 0 {
                    mark_executable(&full_path)?;
                }
            } else if entry.is_dir {
                let _ = fs::create_directory(&full_path);
                installed_files.push(full_path.clone());
            } else if entry.is_symlink {
                let target = String::from_utf8_lossy(&entry.data).to_string();
                let _ = fs::create_symlink(&full_path, &target);
                installed_files.push(full_path.clone());
            }
        }

        Ok(installed_files)
    }
}

fn get_parent_path(path: &str) -> Option<&str> {
    path.rfind('/').map(|i| &path[..i])
}

fn write_file(path: &str, data: &[u8]) -> Result<()> {
    let mut file = File::create(path)
        .map_err(|e| Error::IoError(format!("Failed to create file {}: {:?}", path, e)))?;
    file.write_all(data)
        .map_err(|e| Error::IoError(format!("Failed to write to file {}: {:?}", path, e)))?;
    Ok(())
}

fn mark_executable(_path: &str) -> Result<()> {
    Ok(())
}

fn file_exists(path: &str) -> bool {
    File::open(path).is_ok()
}
