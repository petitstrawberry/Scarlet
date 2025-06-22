//! OverlayFS Implementation
//!
//! This module provides an overlay filesystem implementation that combines
//! multiple filesystem layers into a unified view.

use alloc::{collections::BTreeSet, string::String, sync::Arc, vec::Vec};
use super::super::*;
use crate::fs::mount_tree::MountNode;

/// OverlayFS implementation
#[derive(Clone)]
pub struct OverlayFS {
    /// Upper layer for write operations (may be None for read-only overlay)
    upper_mount_node: Option<Arc<MountNode>>,
    /// Relative path within the upper mount
    upper_relative_path: String,
    /// Lower layer mount nodes (in priority order, highest priority first)
    lower_mount_nodes: Vec<Arc<MountNode>>,
    /// Relative paths corresponding to lower_mount_nodes
    lower_relative_paths: Vec<String>,
}

impl OverlayFS {
    /// Create a new OverlayFS instance
    pub fn new(
        upper_mount_node: Option<Arc<MountNode>>,
        upper_relative_path: String,
        lower_mount_nodes: Vec<Arc<MountNode>>,
        lower_relative_paths: Vec<String>,
    ) -> Result<Self, FileSystemError> {
        // Validate that lower arrays have matching lengths
        if lower_mount_nodes.len() != lower_relative_paths.len() {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotSupported,
                message: "Lower mount nodes and paths must have the same length".to_string(),
            });
        }

        Ok(Self {
            upper_mount_node,
            upper_relative_path,
            lower_mount_nodes,
            lower_relative_paths,
        })
    }

    /// Perform copy-up operation: copy a file from lower layer to upper layer
    /// This is needed for write operations on files that exist only in lower layers
    fn copy_up(&self, path: &str) -> Result<(), FileSystemError> {
        let (upper_node, upper_base_path) = self.get_upper_layer()?;
        let upper_path = Self::combine_paths(&upper_base_path, path);

        // Check if file already exists in upper layer
        let upper_mount_point = upper_node.get_mount_point()?;
        if let Ok((upper_fs, resolved_path)) = upper_mount_point.resolve_fs(&upper_path) {
            let fs_guard = upper_fs.read();
            if fs_guard.metadata(&resolved_path).is_ok() {
                // File already exists in upper layer, no need to copy
                return Ok(());
            }
        }

        // Find the file in lower layers
        for (i, lower_node) in self.lower_mount_nodes.iter().enumerate() {
            let lower_path = Self::combine_paths(&self.lower_relative_paths[i], path);
            
            if let Ok(mount_point) = lower_node.get_mount_point() {
                if let Ok((fs, resolved_path)) = mount_point.resolve_fs(&lower_path) {
                    let fs_guard = fs.read();
                    if let Ok(metadata) = fs_guard.metadata(&resolved_path) {
                        // File found in this lower layer, perform copy-up
                        
                        // Create the file in upper layer
                        let upper_mount_point = upper_node.get_mount_point()?;
                        let (upper_fs, upper_resolved_path) = upper_mount_point.resolve_fs(&upper_path)?;
                        let upper_fs_guard = upper_fs.read();
                        
                        match metadata.file_type {
                            FileType::Directory => {
                                // Create directory in upper layer
                                upper_fs_guard.create_dir(&upper_resolved_path)?;
                            }
                            FileType::RegularFile => {
                                // Create file in upper layer and copy content
                                upper_fs_guard.create_file(&upper_resolved_path, FileType::RegularFile)?;
                                
                                // Copy file content
                                if let Ok(source_file) = fs_guard.open(&resolved_path, 0) { // Read-only
                                    if let Ok(dest_file) = upper_fs_guard.open(&upper_resolved_path, 1) { // Write-only
                                        // Ensure we start writing from the beginning
                                        let _ = dest_file.seek(SeekFrom::Start(0));
                                        
                                        // Copy file content in chunks
                                        let mut buffer = [0u8; 4096]; // 4KB buffer
                                        
                                        loop {
                                            match source_file.read(&mut buffer) {
                                                Ok(bytes_read) if bytes_read > 0 => {
                                                    // Write the read data to destination
                                                    if let Err(_) = dest_file.write(&buffer[..bytes_read]) {
                                                        break; // Write error, stop copying
                                                    }
                                                }
                                                _ => break, // EOF or error, stop copying
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {
                                // For other file types (symlinks, devices, etc.), create a placeholder
                                // In a full implementation, these would need special handling
                                upper_fs_guard.create_file(&upper_resolved_path, metadata.file_type)?;
                            }
                        }
                        
                        return Ok(());
                    }
                }
            }
        }

        // File not found in any lower layer
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: format!("File not found for copy-up: {}", path),
        })
    }

    /// Check if a file exists in upper layer only
    fn file_exists_in_upper(&self, path: &str) -> bool {
        if let Some(ref upper_node) = self.upper_mount_node {
            let upper_path = Self::combine_paths(&self.upper_relative_path, path);
            
            if let Ok(mount_point) = upper_node.get_mount_point() {
                if let Ok((fs, resolved_path)) = mount_point.resolve_fs(&upper_path) {
                    let fs_guard = fs.read();
                    return fs_guard.metadata(&resolved_path).is_ok();
                }
            }
        }
        false
    }

    /// Check if a file exists in any lower layer
    fn file_exists_in_lower(&self, path: &str) -> bool {
        for (i, lower_node) in self.lower_mount_nodes.iter().enumerate() {
            let lower_path = Self::combine_paths(&self.lower_relative_paths[i], path);
            
            if let Ok(mount_point) = lower_node.get_mount_point() {
                if let Ok((fs, resolved_path)) = mount_point.resolve_fs(&lower_path) {
                    let fs_guard = fs.read();
                    if fs_guard.metadata(&resolved_path).is_ok() {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Check if a file exists only in lower layers (not in upper)
    fn file_exists_in_lower_only(&self, path: &str) -> bool {
        !self.file_exists_in_upper(path) && self.file_exists_in_lower(path)
    }

    /// Create a whiteout file to hide a file from lower layers
    fn create_whiteout(&self, path: &str) -> Result<(), FileSystemError> {
        let (upper_node, upper_base_path) = self.get_upper_layer()?;
        
        // Whiteout files are named with a special prefix
        let whiteout_name = format!(".wh.{}", 
            path.split('/').last().unwrap_or(path));
        let parent_path = if let Some(pos) = path.rfind('/') {
            &path[..pos]
        } else {
            ""
        };
        let whiteout_path = if parent_path.is_empty() {
            whiteout_name
        } else {
            format!("{}/{}", parent_path, whiteout_name)
        };
        
        let upper_full_path = Self::combine_paths(&upper_base_path, &whiteout_path);
        let upper_mount_point = upper_node.get_mount_point()?;
        let (upper_fs, resolved_path) = upper_mount_point.resolve_fs(&upper_full_path)?;
        
        let fs_guard = upper_fs.read();
        // Create an empty whiteout file
        fs_guard.create_file(&resolved_path, FileType::RegularFile)
    }

    fn remove_whiteout(&self, path: &str) -> Result<(), FileSystemError> {
        let (upper_node, upper_base_path) = self.get_upper_layer()?;
        
        // Whiteout files are named with a special prefix
        let whiteout_name = format!(".wh.{}", 
            path.split('/').last().unwrap_or(path));
        let parent_path = if let Some(pos) = path.rfind('/') {
            &path[..pos]
        } else {
            ""
        };
        let whiteout_path = if parent_path.is_empty() {
            whiteout_name
        } else {
            format!("{}/{}", parent_path, whiteout_name)
        };
        
        let upper_full_path = Self::combine_paths(&upper_base_path, &whiteout_path);
        let upper_mount_point = upper_node.get_mount_point()?;
        let (upper_fs, resolved_path) = upper_mount_point.resolve_fs(&upper_full_path)?;
        
        let fs_guard = upper_fs.read();
        // Remove the whiteout file
        fs_guard.remove(&resolved_path)
    }

    /// Check if a file is hidden by a whiteout file
    fn is_whiteout(&self, path: &str) -> bool {
        if let Some(ref upper_node) = self.upper_mount_node {
            let whiteout_name = format!(".wh.{}", 
                path.split('/').last().unwrap_or(path));
            let parent_path = if let Some(pos) = path.rfind('/') {
                &path[..pos]
            } else {
                ""
            };
            let whiteout_path = if parent_path.is_empty() {
                whiteout_name
            } else {
                format!("{}/{}", parent_path, whiteout_name)
            };
            
            let upper_full_path = Self::combine_paths(&self.upper_relative_path, &whiteout_path);
            
            if let Ok(mount_point) = upper_node.get_mount_point() {
                if let Ok((fs, resolved_path)) = mount_point.resolve_fs(&upper_full_path) {
                    let fs_guard = fs.read();
                    return fs_guard.metadata(&resolved_path).is_ok();
                }
            }
        }
        false
    }

    /// Helper to combine base path with relative path
    fn combine_paths(base: &str, rel: &str) -> String {
        match (base, rel) {
            (base, "/") => base.to_string(),
            ("/", rel) => rel.to_string(),
            (base, rel) => {
                let base_trimmed = base.trim_end_matches('/');
                let rel_trimmed = rel.trim_start_matches('/');
                if rel_trimmed.is_empty() {
                    base_trimmed.to_string()
                } else {
                    format!("{}/{}", base_trimmed, rel_trimmed)
                }
            }
        }
    }

    /// Ensure upper layer is available for write operations
    fn get_upper_layer(&self) -> Result<(Arc<MountNode>, String), FileSystemError> {
        match &self.upper_mount_node {
            Some(node) => Ok((node.clone(), self.upper_relative_path.clone())),
            None => Err(FileSystemError {
                kind: FileSystemErrorKind::PermissionDenied,
                message: "Overlay is read-only (no upper layer)".to_string(),
            }),
        }
    }
}

impl FileSystem for OverlayFS {
    fn mount(&mut self, _mount_point: &str) -> Result<(), FileSystemError> {
        Ok(())
    }

    fn unmount(&mut self) -> Result<(), FileSystemError> {
        Ok(())
    }

    fn name(&self) -> &str {
        "overlayfs"
    }
}

impl FileOperations for OverlayFS {
    fn open(&self, path: &str, flags: u32) -> Result<Arc<dyn FileObject>, FileSystemError> {
        // Check if the file is hidden by a whiteout
        if self.is_whiteout(path) {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: format!("File is hidden by whiteout: {}", path),
            });
        }

        // Check if this is a write operation (common write flags: 1=write, 2=read/write, etc.)
        let is_write_operation = (flags & 0x3) != 0; // O_WRONLY=1, O_RDWR=2
        
        // If writing to a file that exists only in lower layer, copy it up first
        if is_write_operation && self.file_exists_in_lower_only(path) {
            self.copy_up(path)?;
        }

        // Check upper layer first (if present)
        if let Some(ref upper_node) = self.upper_mount_node {
            let upper_path = Self::combine_paths(&self.upper_relative_path, path);
            
            if let Ok(mount_point) = upper_node.get_mount_point() {
                if let Ok((fs, resolved_path)) = mount_point.resolve_fs(&upper_path) {
                    let fs_guard = fs.read();
                    if let Ok(file) = fs_guard.open(&resolved_path, flags) {
                        return Ok(file);
                    }
                }
            }
        }

        // For write operations, we need an upper layer
        if is_write_operation {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::PermissionDenied,
                message: "Cannot write to read-only overlay".to_string(),
            });
        }

        // Check lower layers in order (read-only access)
        for (i, lower_node) in self.lower_mount_nodes.iter().enumerate() {
            let lower_path = Self::combine_paths(&self.lower_relative_paths[i], path);
            
            if let Ok(mount_point) = lower_node.get_mount_point() {
                if let Ok((fs, resolved_path)) = mount_point.resolve_fs(&lower_path) {
                    let fs_guard = fs.read();
                    if let Ok(file) = fs_guard.open(&resolved_path, flags) {
                        return Ok(file);
                    }
                }
            }
        }

        // File not found in any layer
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: format!("File not found: {}", path),
        })
    }

    fn readdir(&self, path: &str) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        let mut entries = Vec::new();
        let mut seen_names = BTreeSet::new();
        let mut found_any_layer = false;

        // Read from upper layer first (if present)
        if let Some(ref upper_node) = self.upper_mount_node {
            let upper_path = Self::combine_paths(&self.upper_relative_path, path);
            
            if let Ok(mount_point) = upper_node.get_mount_point() {
                if let Ok((fs, resolved_path)) = mount_point.resolve_fs(&upper_path) {
                    // Since VirtualFileSystem auto-implements FileOperations, we can call readdir directly
                    let fs_guard = fs.read();
                    if let Ok(upper_entries) = fs_guard.readdir(&resolved_path) {
                        found_any_layer = true;
                        for entry in upper_entries {
                            // Skip whiteout files themselves
                            if entry.name.starts_with(".wh.") {
                                continue;
                            }
                            seen_names.insert(entry.name.clone());
                            entries.push(entry);
                        }
                    }
                }
            }
        }

        // Read from lower layers (skip entries already seen in upper layers)
        for (i, lower_node) in self.lower_mount_nodes.iter().enumerate() {
            let lower_path = Self::combine_paths(&self.lower_relative_paths[i], path);
            
            if let Ok(mount_point) = lower_node.get_mount_point() {
                if let Ok((fs, resolved_path)) = mount_point.resolve_fs(&lower_path) {
                    let fs_guard = fs.read();
                    if let Ok(lower_entries) = fs_guard.readdir(&resolved_path) {
                        found_any_layer = true;
                        for entry in lower_entries {
                            // Only add if not already seen in a higher layer and not hidden by whiteout
                            if !seen_names.contains(&entry.name) && !self.is_whiteout(&entry.name) {
                                seen_names.insert(entry.name.clone());
                                entries.push(entry);
                            }
                        }
                    }
                }
            }
        }

        // If no layers were accessible, return an error for consistency
        if !found_any_layer && self.upper_mount_node.is_none() && self.lower_mount_nodes.is_empty() {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "No filesystem layers available".to_string(),
            });
        }

        Ok(entries)
    }

    fn create_file(&self, path: &str, file_type: FileType) -> Result<(), FileSystemError> {
        let entries = self.readdir(path)?;
        if entries.iter().any(|e| e.name == path) {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::AlreadyExists,
                message: format!("File already exists: {}", path),
            });
        }

        if self.is_whiteout(path) {
            // Remove the whiteout file if it exists
            self.remove_whiteout(path)?;
        }
     
        let (upper_node, upper_base_path) = self.get_upper_layer()?;
        let upper_path = Self::combine_paths(&upper_base_path, path);
        
        let upper_mount_point = upper_node.get_mount_point()?;
        let (upper_fs, resolved_path) = upper_mount_point.resolve_fs(&upper_path)?;
        
        let fs_guard = upper_fs.read();

        fs_guard.create_file(&resolved_path, file_type)
    }

    fn create_dir(&self, path: &str) -> Result<(), FileSystemError> {
        // If directory exists in lower layer only, copy it up first
        if self.file_exists_in_lower_only(path) {
            self.copy_up(path)?;
        }
        
        let (upper_node, upper_base_path) = self.get_upper_layer()?;
        let upper_path = Self::combine_paths(&upper_base_path, path);
        
        let upper_mount_point = upper_node.get_mount_point()?;
        let (upper_fs, resolved_path) = upper_mount_point.resolve_fs(&upper_path)?;
        
        let fs_guard = upper_fs.read();
        fs_guard.create_dir(&resolved_path)
    }

    fn remove(&self, path: &str) -> Result<(), FileSystemError> {
        let (upper_node, upper_base_path) = self.get_upper_layer()?;
        
        // Check if the file exists in upper layer
        if self.file_exists_in_upper(path) {
            // File exists in upper layer, just remove it
            let upper_path = Self::combine_paths(&upper_base_path, path);
            let upper_mount_point = upper_node.get_mount_point()?;
            let (upper_fs, resolved_path) = upper_mount_point.resolve_fs(&upper_path)?;
            let fs_guard = upper_fs.read();
            fs_guard.remove(&resolved_path)
        } else if self.file_exists_in_lower(path) {
            // File exists only in lower layer, create whiteout to hide it
            self.create_whiteout(path)
        } else {
            // File doesn't exist anywhere
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: format!("File not found: {}", path),
            })
        }
    }

    fn metadata(&self, path: &str) -> Result<FileMetadata, FileSystemError> {
        // Check if the file is hidden by a whiteout
        if self.is_whiteout(path) {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: format!("File is hidden by whiteout: {}", path),
            });
        }

        // Check upper layer first (if present)
        if let Some(ref upper_node) = self.upper_mount_node {
            let upper_path = Self::combine_paths(&self.upper_relative_path, path);
            
            if let Ok(mount_point) = upper_node.get_mount_point() {
                if let Ok((fs, resolved_path)) = mount_point.resolve_fs(&upper_path) {
                    let fs_guard = fs.read();
                    if let Ok(metadata) = fs_guard.metadata(&resolved_path) {
                        return Ok(metadata);
                    }
                }
            }
        }

        // Check lower layers in order
        for (i, lower_node) in self.lower_mount_nodes.iter().enumerate() {
            let lower_path = Self::combine_paths(&self.lower_relative_paths[i], path);
            
            if let Ok(mount_point) = lower_node.get_mount_point() {
                if let Ok((fs, resolved_path)) = mount_point.resolve_fs(&lower_path) {
                    let fs_guard = fs.read();
                    if let Ok(metadata) = fs_guard.metadata(&resolved_path) {
                        return Ok(metadata);
                    }
                }
            }
        }

        Err(FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: format!("Path not found in any layer: {}", path),
        })
    }

    fn truncate(&self, path: &str, size: u64) -> Result<(), FileSystemError> {
        // If file exists only in lower layer, copy it up first
        if self.file_exists_in_lower_only(path) {
            self.copy_up(path)?;
        }

        // Check if file exists in upper layer first
        if let Some(ref upper_node) = self.upper_mount_node {
            let upper_path = Self::combine_paths(&self.upper_relative_path, path);
            
            if let Ok(mount_point) = upper_node.get_mount_point() {
                if let Ok((fs, resolved_path)) = mount_point.resolve_fs(&upper_path) {
                    let fs_guard = fs.read();
                    if fs_guard.metadata(&resolved_path).is_ok() {
                        // File exists in upper layer, truncate it
                        return fs_guard.truncate(&resolved_path, size);
                    }
                }
            }
        }

        // File doesn't exist in upper layer, check if it exists in lower layers
        for (i, lower_node) in self.lower_mount_nodes.iter().enumerate() {
            let lower_path = Self::combine_paths(&self.lower_relative_paths[i], path);
            
            if let Ok(mount_point) = lower_node.get_mount_point() {
                if let Ok((fs, resolved_path)) = mount_point.resolve_fs(&lower_path) {
                    let fs_guard = fs.read();
                    if fs_guard.metadata(&resolved_path).is_ok() {
                        // File exists in lower layer, copy it up and then truncate
                        self.copy_up(path)?;
                        
                        // Now truncate in upper layer
                        let (upper_node, upper_base_path) = self.get_upper_layer()?;
                        let upper_path = Self::combine_paths(&upper_base_path, path);
                        let upper_mount_point = upper_node.get_mount_point()?;
                        let (upper_fs, upper_resolved_path) = upper_mount_point.resolve_fs(&upper_path)?;
                        let upper_fs_guard = upper_fs.read();
                        return upper_fs_guard.truncate(&upper_resolved_path, size);
                    }
                }
            }
        }

        // File not found in any layer
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: format!("File not found: {}", path),
        })
    }
}

#[cfg(test)]
mod tests;