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
                                
                                // Copy file content (simplified - in reality would need to handle large files)
                                if let Ok(_source_file) = fs_guard.open(&resolved_path, 0) {
                                    if let Ok(_dest_file) = upper_fs_guard.open(&upper_resolved_path, 1) { // Write mode
                                        // In a real implementation, we would copy the file content here
                                        // This is a placeholder for the copy operation
                                    }
                                }
                            }
                            _ => {
                                // For other file types, just create a placeholder
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

        // Check lower layers in order
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

    fn read_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>, FileSystemError> {
        let mut entries = Vec::new();
        let mut seen_names = BTreeSet::new();

        // Read from upper layer first (if present)
        if let Some(ref upper_node) = self.upper_mount_node {
            let upper_path = Self::combine_paths(&self.upper_relative_path, path);
            
            if let Ok(mount_point) = upper_node.get_mount_point() {
                if let Ok((fs, resolved_path)) = mount_point.resolve_fs(&upper_path) {
                    // Since VirtualFileSystem auto-implements FileOperations, we can call read_dir directly
                    let fs_guard = fs.read();
                    if let Ok(upper_entries) = fs_guard.read_dir(&resolved_path) {
                        for entry in upper_entries {
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
                    if let Ok(lower_entries) = fs_guard.read_dir(&resolved_path) {
                        for entry in lower_entries {
                            // Only add if not already seen in a higher layer
                            if !seen_names.contains(&entry.name) {
                                seen_names.insert(entry.name.clone());
                                entries.push(entry);
                            }
                        }
                    }
                }
            }
        }

        Ok(entries)
    }

    fn create_file(&self, path: &str, file_type: FileType) -> Result<(), FileSystemError> {
        let (upper_node, upper_base_path) = self.get_upper_layer()?;
        let upper_path = Self::combine_paths(&upper_base_path, path);
        
        let upper_mount_point = upper_node.get_mount_point()?;
        let (upper_fs, resolved_path) = upper_mount_point.resolve_fs(&upper_path)?;
        
        let fs_guard = upper_fs.read();
        fs_guard.create_file(&resolved_path, file_type)
    }

    fn create_dir(&self, path: &str) -> Result<(), FileSystemError> {
        let (upper_node, upper_base_path) = self.get_upper_layer()?;
        let upper_path = Self::combine_paths(&upper_base_path, path);
        
        let upper_mount_point = upper_node.get_mount_point()?;
        let (upper_fs, resolved_path) = upper_mount_point.resolve_fs(&upper_path)?;
        
        let fs_guard = upper_fs.read();
        fs_guard.create_dir(&resolved_path)
    }

    fn remove(&self, path: &str) -> Result<(), FileSystemError> {
        let (upper_node, upper_base_path) = self.get_upper_layer()?;
        let upper_path = Self::combine_paths(&upper_base_path, path);
        
        let upper_mount_point = upper_node.get_mount_point()?;
        let (upper_fs, resolved_path) = upper_mount_point.resolve_fs(&upper_path)?;
        
        let fs_guard = upper_fs.read();
        fs_guard.remove(&resolved_path)
    }

    fn metadata(&self, path: &str) -> Result<FileMetadata, FileSystemError> {
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
}

