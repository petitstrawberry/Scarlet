use crate::package::PackageMetadata;
use crate::{Error, Result};
use alloc::{format, string::String, string::ToString, vec::Vec};
use miniz_oxide::inflate::decompress_to_vec;
use scarlet_std::fs::{self, File};
use tar_no_std::TarArchiveRef;

#[derive(Debug)]
pub struct TarEntry {
    pub name: String,
    pub mode: u32,
    pub size: u64,
    pub is_file: bool,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub link_target: Option<String>,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub struct PackageArchive {
    pub metadata: PackageMetadata,
    pub entries: Vec<TarEntry>,
}

impl PackageArchive {
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        // Decompress gzip data
        let decompressed = decompress_gzip(data)?;

        let mut entries = Vec::new();
        let mut metadata: Option<PackageMetadata> = None;

        // Parse tar archive
        let archive = TarArchiveRef::new(&decompressed).map_err(|e| {
            Error::InvalidPackageFormat(format!("Failed to parse tar archive: {:?}", e))
        })?;

        // Iterate through tar entries
        for entry in archive.entries() {
            let name = entry
                .filename()
                .as_str()
                .map_err(|_| Error::IoError("Invalid UTF-8 in filename".into()))?
                .to_string();

            let size = entry.size() as u64;
            let header = entry.posix_header();
            let mode = header
                .mode
                .to_flags()
                .map(|f| f.bits() as u32)
                .unwrap_or(0o644);

            // Determine file type
            let typeflag = header
                .typeflag
                .try_to_type_flag()
                .map_err(|_| Error::IoError("Invalid type flag".into()))?;
            let is_file = matches!(
                typeflag,
                tar_no_std::TypeFlag::REGTYPE | tar_no_std::TypeFlag::AREGTYPE
            );
            let is_dir = typeflag == tar_no_std::TypeFlag::DIRTYPE;
            let is_symlink = typeflag == tar_no_std::TypeFlag::SYMTYPE;

            // Read file data
            let data = if is_file {
                let entry_data = entry.data().to_vec();
                // Parse package.toml if found (handle both "./package.toml" and "package.toml")
                if name == "package.toml" || name.ends_with("/package.toml") {
                    metadata = Some(parse_package_toml(&entry_data)?);
                }
                entry_data
            } else {
                Vec::new()
            };

            // Read symlink target from header linkname field
            let link_target = if is_symlink {
                match header.linkname.as_str() {
                    Ok(linkname) if !linkname.is_empty() => Some(String::from(linkname)),
                    _ => None,
                }
            } else {
                None
            };

            entries.push(TarEntry {
                name,
                mode,
                size,
                is_file,
                is_dir,
                is_symlink,
                link_target,
                data,
            });
        }

        let metadata = metadata.unwrap_or_default();
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
            } else if entry.is_symlink
                && let Some(ref target) = entry.link_target
            {
                let _ = fs::create_symlink(&full_path, target);
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
            } else if entry.is_symlink
                && let Some(ref target) = entry.link_target
            {
                let _ = fs::create_symlink(&full_path, target);
                installed_files.push(full_path.clone());
            }
        }

        Ok(installed_files)
    }
}

/// Decompress gzip data
fn decompress_gzip(data: &[u8]) -> Result<Vec<u8>> {
    // Check gzip magic number
    if data.len() < 18 {
        return Err(Error::IoError("Gzip data too short".into()));
    }

    if data[0] != 0x1f || data[1] != 0x8b {
        return Err(Error::IoError("Invalid gzip magic number".into()));
    }

    // Gzip header parsing
    let compression_method = data[2];
    if compression_method != 8 {
        return Err(Error::IoError(
            "Unsupported compression method (only deflate)".into(),
        ));
    }

    let flags = data[3];
    let mut offset = 10;

    // Skip extra fields (FLG.FEXTRA)
    if flags & 0x04 != 0 {
        if offset + 2 > data.len() {
            return Err(Error::IoError("Invalid extra field length".into()));
        }
        let xlen = (data[offset] as usize) | ((data[offset + 1] as usize) << 8);
        offset += 2 + xlen;
    }

    // Skip original filename (FLG.FNAME)
    if flags & 0x08 != 0 {
        while offset < data.len() && data[offset] != 0 {
            offset += 1;
        }
        offset += 1;
    }

    // Skip comment (FLG.FCOMMENT)
    if flags & 0x10 != 0 {
        while offset < data.len() && data[offset] != 0 {
            offset += 1;
        }
        offset += 1;
    }

    // Skip header CRC (FLG.FHCRC)
    if flags & 0x02 != 0 {
        offset += 2;
    }

    // Decompress deflate data (last 8 bytes are CRC32 and ISIZE)
    let deflate_data = &data[offset..data.len() - 8];
    decompress_to_vec(deflate_data)
        .map_err(|e| Error::IoError(format!("Decompression failed: {:?}", e)))
}

/// Parse package.toml from bytes
fn parse_package_toml(data: &[u8]) -> Result<PackageMetadata> {
    let toml_str = core::str::from_utf8(data)
        .map_err(|_| Error::IoError("package.toml is not valid UTF-8".into()))?;

    let mut metadata = PackageMetadata::default();
    let mut in_package_section = false;

    for line in toml_str.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Section header
        if line.starts_with('[') && line.ends_with(']') {
            let section = &line[1..line.len() - 1];
            in_package_section = section == "package";
            continue;
        }

        // Only parse keys from [package] section
        if !in_package_section {
            continue;
        }

        // Key-value pair
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"');

            match key {
                "name" => metadata.name = value.to_string(),
                "version" => metadata.version = value.to_string(),
                "description" => metadata.description = value.to_string(),
                "author" => metadata.author = Some(value.to_string()),
                "architecture" => metadata.architecture = value.to_string(),
                "binaries" => {
                    // Parse array format: ["item1", "item2"]
                    for item in value.split(',') {
                        let item = item
                            .trim()
                            .trim_matches('[')
                            .trim_matches(']')
                            .trim_matches('"');
                        if !item.is_empty() {
                            metadata.binaries.push(item.to_string());
                        }
                    }
                }
                "libraries" => {
                    for item in value.split(',') {
                        let item = item
                            .trim()
                            .trim_matches('[')
                            .trim_matches(']')
                            .trim_matches('"');
                        if !item.is_empty() {
                            metadata.libraries.push(item.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }

    Ok(metadata)
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
