//! OverlayFS v2 - Overlay filesystem implementation for VFS v2
//!
//! This provides a union/overlay view of multiple filesystems (upper/lower).
//! Only supports same-VfsManager overlays (no cross-vfs).

use alloc::string::ToString;
use alloc::{sync::Arc, string::String, vec::Vec, collections::BTreeSet, format};
use spin::RwLock;
use core::any::Any;

use crate::fs::vfs_v2::core::{VfsNode, FileSystemOperations, DirectoryEntryInternal, VfsEntry};
use crate::fs::{FileSystemError, FileSystemErrorKind, FileType, FileMetadata, FileObject, FilePermission, SeekFrom};
use crate::object::capability::{StreamOps, StreamError};
use crate::fs::vfs_v2::mount_tree::MountPoint;

/// OverlayFS implementation for VFS v2
pub struct OverlayFS {
    /// Upper layer for write operations (may be None for read-only overlay)
    upper: Option<(Arc<MountPoint>, Arc<VfsEntry>)>,
    /// Lower layers (in priority order, highest priority first)
    lower_layers: Vec<(Arc<MountPoint>, Arc<VfsEntry>)>,
    /// Filesystem name
    name: String,
    /// Root node (composite of all layers)
    root_node: Arc<OverlayNode>,
}

/// A composite node that represents a file/directory across overlay layers
pub struct OverlayNode {
    /// Node name
    name: String,
    /// Reference to overlay filesystem
    overlay_fs: RwLock<Option<Arc<OverlayFS>>>,
    /// Path in the overlay
    path: String,
    /// File type (resolved from layers)
    file_type: FileType,
    /// File ID
    file_id: u64,
}

impl OverlayNode {
    pub fn new(name: String, path: String, file_type: FileType, file_id: u64) -> Arc<Self> {
        Arc::new(Self {
            name,
            overlay_fs: RwLock::new(None),
            path,
            file_type,
            file_id,
        })
    }

    pub fn set_overlay_fs(&self, fs: Arc<OverlayFS>) {
        *self.overlay_fs.write() = Some(fs);
    }
}

impl VfsNode for OverlayNode {
    fn id(&self) -> u64 {
        self.file_id
    }

    fn filesystem(&self) -> Option<alloc::sync::Weak<dyn FileSystemOperations>> {
        self.overlay_fs.read().as_ref().map(|fs| Arc::downgrade(fs) as alloc::sync::Weak<dyn FileSystemOperations>)
    }

    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        if let Some(ref fs) = *self.overlay_fs.read() {
            fs.get_metadata_for_path(&self.path)
        } else {
            Err(FileSystemError::new(FileSystemErrorKind::NotSupported, "No filesystem reference"))
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl OverlayFS {
    pub fn new_with_dirs(
        upper: Option<(Arc<MountPoint>, Arc<VfsEntry>)>,
        lower_layers: Vec<(Arc<MountPoint>, Arc<VfsEntry>)>,
        name: String
    ) -> Result<Arc<Self>, FileSystemError> {
        let root_node = OverlayNode::new("/".to_string(), "/".to_string(), FileType::Directory, 1);
        let overlay = Arc::new(Self {
            upper,
            lower_layers,
            name,
            root_node: root_node.clone(),
        });
        root_node.set_overlay_fs(overlay.clone());
        Ok(overlay)
    }

    /// MountPointからFileSystemOperationsを取得
    fn fs_from_mount(mount: &Arc<MountPoint>) -> Arc<dyn FileSystemOperations> {
        mount.root.node().filesystem().unwrap().upgrade().unwrap()
    }

    /// Get metadata for a path by checking layers in priority order
    fn get_metadata_for_path(&self, path: &str) -> Result<FileMetadata, FileSystemError> {
        // Check for whiteout first
        if self.is_whiteout(path) {
            return Err(FileSystemError::new(FileSystemErrorKind::NotFound, "File is hidden by whiteout"));
        }

        // Check upper layer first
        if let Some((ref upper_fs, ref upper_node)) = self.upper {
            if let Ok(node) = self.resolve_in_layer(upper_fs, upper_node, path) {
                return node.metadata();
            }
        }

        // Check lower layers
        for (lower_fs, lower_node) in &self.lower_layers {
            if let Ok(node) = self.resolve_in_layer(lower_fs, lower_node, path) {
                return node.metadata();
            }
        }

        Err(FileSystemError::new(FileSystemErrorKind::NotFound, "File not found in any layer"))
    }

    /// Resolve a path in a specific layer, starting from the given node
    fn resolve_in_layer(&self, mount: &Arc<MountPoint>, entry: &Arc<VfsEntry>, path: &str) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        let mut current_mount = mount.clone();
        let mut current_node = entry.node();

        let parts: Vec<&str> = path.trim_start_matches('/').split('/').filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            return Ok(current_node);
        }

        for part in parts {
            let current_fs = current_node.filesystem()
                .and_then(|w| w.upgrade())
                .ok_or_else(|| FileSystemError::new(FileSystemErrorKind::NotSupported, "Node has no filesystem"))?;
            
            let next_node = current_fs.lookup(&current_node, &part.to_string())?;

            let child_mount_opt = current_mount.children.read().get(&next_node.id()).cloned();

            if let Some(child_mount) = child_mount_opt {
                current_mount = child_mount.clone();
                current_node = child_mount.root.node();
            } else {
                current_node = next_node;
            }
        }

        Ok(current_node)
    }

    /// Check if a file is hidden by a whiteout file
    fn is_whiteout(&self, path: &str) -> bool {
        if let Some((ref upper_fs, ref upper_node)) = self.upper {
            let whiteout_name = format!(".wh.{}", 
                path.split('/').last().unwrap_or(path));
            let parent_path = if let Some(pos) = path.rfind('/') {
                &path[..pos]
            } else {
                "/"
            };
            let whiteout_path = if parent_path == "/" {
                format!("/{}", whiteout_name)
            } else {
                format!("{}/{}", parent_path, whiteout_name)
            };
            
            self.resolve_in_layer(upper_fs, upper_node, &whiteout_path).is_ok()
        } else {
            false
        }
    }

    /// Get upper layer, error if not available
    fn get_upper_layer(&self) -> Result<(Arc<MountPoint>, Arc<VfsEntry>), FileSystemError> {
        self.upper.as_ref().map(|fs| fs.clone()).ok_or_else(|| 
            FileSystemError::new(FileSystemErrorKind::PermissionDenied, "Overlay is read-only (no upper layer)")
        )
    }

    /// Create a whiteout file to hide a file from lower layers
    fn create_whiteout(&self, path: &str) -> Result<(), FileSystemError> {
        let upper = self.get_upper_layer()?;
        let whiteout_name = format!(".wh.{}", 
            path.split('/').last().unwrap_or(path));
        let parent_path = if let Some(pos) = path.rfind('/') {
            &path[..pos]
        } else {
            "/"
        };
        let whiteout_path = if parent_path == "/" {
            format!("/{}", whiteout_name)
        } else {
            format!("{}/{}", parent_path, whiteout_name)
        };
        // Create parent directories if needed
        self.ensure_parent_dirs(&whiteout_path)?;
        let parent_node = self.resolve_in_layer(&upper.0, &upper.1, parent_path)?;
        let fs = Self::fs_from_mount(&upper.0);
        fs.create(&parent_node, &whiteout_name, FileType::RegularFile, 0o644)
            .map(|_| ())
    }

    /// Perform copy-up operation: copy a file from lower layer to upper layer
    fn copy_up(&self, path: &str) -> Result<(), FileSystemError> {
        let upper = self.get_upper_layer()?;
        let upper_fs = Self::fs_from_mount(&upper.0);
        // Check if file already exists in upper layer
        if self.resolve_in_layer(&upper.0, &upper.1, path).is_ok() {
            return Ok(());
        }
        // Find the file in lower layers
        for (lower_mount, lower_node) in &self.lower_layers {
            if let Ok(lower_node) = self.resolve_in_layer(lower_mount, lower_node, path) {
                let metadata = lower_node.metadata()?;
                // Ensure parent directories exist in upper layer
                self.ensure_parent_dirs(path)?;
                let parent_path = if let Some(pos) = path.rfind('/') {
                    &path[..pos]
                } else {
                    "/"
                };
                let filename = path.split('/').last().unwrap_or(path);
                let parent_node = self.resolve_in_layer(&upper.0, &upper.1, parent_path)?;
                match metadata.file_type {
                    FileType::Directory => {
                        upper_fs.create(&parent_node, &filename.to_string(), FileType::Directory, 0o755)?;
                    }
                    FileType::RegularFile => {
                        // Create file and copy content
                        let new_node = upper_fs.create(&parent_node, &filename.to_string(), FileType::RegularFile, 0o644)?;
                        // Copy file content
                        let lower_fs = Self::fs_from_mount(lower_mount);
                        if let Ok(source_file) = lower_fs.open(&lower_node, 0) { // Read-only
                            if let Ok(dest_file) = upper_fs.open(&new_node, 1) { // Write-only
                                let _ = dest_file.seek(SeekFrom::Start(0));
                                let mut buffer = [0u8; 4096];
                                loop {
                                    match source_file.read(&mut buffer) {
                                        Ok(bytes_read) if bytes_read > 0 => {
                                            if dest_file.write(&buffer[..bytes_read]).is_err() {
                                                break;
                                            }
                                        }
                                        _ => break,
                                    }
                                }
                            }
                        }
                    }
                    _ => {
                        // For other file types, create a placeholder
                        upper_fs.create(&parent_node, &filename.to_string(), metadata.file_type, 0o644)?;
                    }
                }
                return Ok(());
            }
        }
        Err(FileSystemError::new(FileSystemErrorKind::NotFound, "File not found for copy-up"))
    }

    /// Ensure parent directories exist in upper layer
    fn ensure_parent_dirs(&self, path: &str) -> Result<(), FileSystemError> {
        let upper = self.get_upper_layer()?;
        let upper_fs = Self::fs_from_mount(&upper.0);
        let parent_path = if let Some(pos) = path.rfind('/') {
            &path[..pos]
        } else {
            return Ok(());
        };
        if parent_path.is_empty() || parent_path == "/" {
            return Ok(());
        }
        // Try to resolve parent - if it fails, create it
        if self.resolve_in_layer(&upper.0, &upper.1, parent_path).is_err() {
            self.ensure_parent_dirs(parent_path)?;
            let grandparent_path = if let Some(pos) = parent_path.rfind('/') {
                &parent_path[..pos]
            } else {
                "/"
            };
            let dirname = parent_path.split('/').last().unwrap_or(parent_path);
            let grandparent_node = self.resolve_in_layer(&upper.0, &upper.1, if grandparent_path.is_empty() { "/" } else { grandparent_path })?;
            upper_fs.create(&grandparent_node, &dirname.to_string(), FileType::Directory, 0o755)?;
        }
        Ok(())
    }

    /// Check if file exists only in lower layers (not in upper)
    fn file_exists_in_lower_only(&self, path: &str) -> bool {
        // Check if exists in upper
        if let Some((ref upper_fs, ref upper_node)) = self.upper {
            if self.resolve_in_layer(upper_fs, upper_node, path).is_ok() {
                return false;
            }
        }
        
        // Check if exists in any lower layer
        for (lower_fs, lower_node) in &self.lower_layers {
            if self.resolve_in_layer(lower_fs, lower_node, path).is_ok() {
                return true;
            }
        }
        
        false
    }
}

impl FileSystemOperations for OverlayFS {
    fn lookup(&self, parent_node: &Arc<dyn VfsNode>, name: &String) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        let overlay_parent = parent_node.as_any()
            .downcast_ref::<OverlayNode>()
            .ok_or_else(|| FileSystemError::new(FileSystemErrorKind::NotSupported, "Invalid node type for OverlayFS"))?;

        let child_path = if overlay_parent.path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", overlay_parent.path, name)
        };

        // Handle special directory entries
        if name == "." {
            return Ok(parent_node.clone());
        }
        if name == ".." {
            let parent_path = if let Some(pos) = overlay_parent.path.rfind('/') {
                if pos == 0 { "/" } else { &overlay_parent.path[..pos] }
            } else {
                "/"
            };
            let parent_name = parent_path.split('/').last().unwrap_or("/");
            let node = OverlayNode::new(parent_name.to_string(), parent_path.to_string(), FileType::Directory, 0);
            if let Some(ref fs) = *overlay_parent.overlay_fs.read() {
                node.set_overlay_fs(Arc::clone(fs));
            }
            return Ok(node);
        }

        // Check for whiteout
        if self.is_whiteout(&child_path) {
            return Err(FileSystemError::new(FileSystemErrorKind::NotFound, "File is hidden by whiteout"));
        }

        // Try upper layer first
        if let Some((ref upper_fs, ref upper_node)) = self.upper {
            if let Ok(_) = self.resolve_in_layer(upper_fs, upper_node, &child_path) {
                let metadata = self.get_metadata_for_path(&child_path)?;
                let node = OverlayNode::new(name.clone(), child_path.clone(), metadata.file_type, metadata.file_id);
                if let Some(ref fs) = *overlay_parent.overlay_fs.read() {
                    node.set_overlay_fs(Arc::clone(fs));
                }
                return Ok(node);
            }
        }

        // Try lower layers
        for (lower_fs, lower_node) in &self.lower_layers {
            if let Ok(_) = self.resolve_in_layer(lower_fs, lower_node, &child_path) {
                let metadata = self.get_metadata_for_path(&child_path)?;
                let node = OverlayNode::new(name.clone(), child_path.clone(), metadata.file_type, metadata.file_id);
                if let Some(ref fs) = *overlay_parent.overlay_fs.read() {
                    node.set_overlay_fs(Arc::clone(fs));
                }
                return Ok(node);
            }
        }

        Err(FileSystemError::new(FileSystemErrorKind::NotFound, "File not found"))
    }

    fn open(&self, overlay_node: &Arc<dyn VfsNode>, flags: u32) -> Result<Arc<dyn FileObject>, FileSystemError> {
        // OverlayNodeにダウンキャスト
        let overlay_node = overlay_node.as_any()
            .downcast_ref::<OverlayNode>()
            .ok_or_else(|| FileSystemError::new(FileSystemErrorKind::NotSupported, "Invalid node type for OverlayFS"))?;
        // Check if this is a write operation
        let is_write_operation = (flags & 0x3) != 0; // O_WRONLY=1, O_RDWR=2
        // If writing to a file that exists only in lower layer, copy it up first
        if is_write_operation && self.file_exists_in_lower_only(&overlay_node.path) {
            self.copy_up(&overlay_node.path)?;
        }
        // Try upper layer first
        if let Some((ref upper_mount, ref upper_node)) = self.upper {
            if let Ok(upper_node) = self.resolve_in_layer(upper_mount, upper_node, &overlay_node.path) {
                let fs = Self::fs_from_mount(upper_mount);
                if let Ok(file) = fs.open(&upper_node, flags) {
                    return Ok(file);
                }
            }
        }
        // For write operations, we need an upper layer
        if is_write_operation {
            return Err(FileSystemError::new(FileSystemErrorKind::PermissionDenied, "Cannot write to read-only overlay"));
        }
        // Try lower layers for read operations
        for (lower_mount, lower_node) in &self.lower_layers {
            if let Ok(lower_node) = self.resolve_in_layer(lower_mount, lower_node, &overlay_node.path) {
                let fs = Self::fs_from_mount(lower_mount);
                if let Ok(file) = fs.open(&lower_node, flags) {
                    return Ok(file);
                }
            }
        }
        Err(FileSystemError::new(FileSystemErrorKind::NotFound, "File not found"))
    }

    fn create(&self, parent_node: &Arc<dyn VfsNode>, name: &String, file_type: FileType, mode: u32) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        let upper = self.get_upper_layer()?;
        let upper_fs = Self::fs_from_mount(&upper.0);
        let overlay_parent = parent_node.as_any()
            .downcast_ref::<OverlayNode>()
            .ok_or_else(|| FileSystemError::new(FileSystemErrorKind::NotSupported, "Invalid node type for OverlayFS"))?;
        let child_path = if overlay_parent.path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", overlay_parent.path, name)
        };
        // Ensure parent exists in upper layer (copy-up if needed)
        if self.file_exists_in_lower_only(&overlay_parent.path) {
            self.copy_up(&overlay_parent.path)?;
        }
        // Remove any existing whiteout
        if self.is_whiteout(&child_path) {
            // Remove whiteout file by creating the actual file
        }
        let upper_parent = self.resolve_in_layer(&upper.0, &upper.1, &overlay_parent.path)?;
        let fs = Self::fs_from_mount(&upper.0);
        let new_node = fs.create(&upper_parent, name, file_type, mode)?;
        // Return overlay node
        let metadata = new_node.metadata()?;
        let overlay_node = OverlayNode::new(name.clone(), child_path, metadata.file_type, metadata.file_id);
        if let Some(ref fs) = *overlay_parent.overlay_fs.read() {
            overlay_node.set_overlay_fs(Arc::clone(fs));
        }
        Ok(overlay_node)
    }

    fn remove(&self, parent_node: &Arc<dyn VfsNode>, name: &String) -> Result<(), FileSystemError> {
        let overlay_parent = parent_node.as_any()
            .downcast_ref::<OverlayNode>()
            .ok_or_else(|| FileSystemError::new(FileSystemErrorKind::NotSupported, "Invalid node type for OverlayFS"))?;

        let child_path = if overlay_parent.path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", overlay_parent.path, name)
        };

        // If file exists in upper layer, remove it
        if let Some((ref upper_mount, ref upper_entry)) = self.upper {
            if let Ok(upper_parent) = self.resolve_in_layer(upper_mount, upper_entry, &overlay_parent.path) {
                let fs = Self::fs_from_mount(upper_mount);
                if fs.remove(&upper_parent, name).is_ok() {
                    // If file also exists in lower layers, create whiteout
                    for (lower_mount, lower_entry) in &self.lower_layers {
                        if self.resolve_in_layer(lower_mount, lower_entry, &child_path).is_ok() {
                            self.create_whiteout(&child_path)?;
                            break;
                        }
                    }
                    return Ok(());
                }
            }
        }

        // If file exists only in lower layers, create whiteout
        for (lower_mount, lower_node) in &self.lower_layers {
            if self.resolve_in_layer(lower_mount, lower_node, &child_path).is_ok() {
                self.create_whiteout(&child_path)?;
                return Ok(());
            }
        }

        Err(FileSystemError::new(FileSystemErrorKind::NotFound, "File not found"))
    }

    fn root_node(&self) -> Arc<dyn VfsNode> {
        Arc::clone(&self.root_node) as Arc<dyn VfsNode>
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn is_read_only(&self) -> bool {
        self.upper.is_none()
    }

    fn readdir(&self, node: &Arc<dyn VfsNode>) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        let overlay_node = node.as_any()
            .downcast_ref::<OverlayNode>()
            .ok_or_else(|| FileSystemError::new(FileSystemErrorKind::NotSupported, "Invalid node type for OverlayFS"))?;

        let mut entries = Vec::new();
        let mut seen_names = BTreeSet::new();

        // Add "." and ".." entries
        entries.push(DirectoryEntryInternal {
            name: ".".to_string(),
            file_type: FileType::Directory,
            file_id: overlay_node.file_id,
        });
        entries.push(DirectoryEntryInternal {
            name: "..".to_string(),
            file_type: FileType::Directory,
            file_id: 0,
        });
        seen_names.insert(".".to_string());
        seen_names.insert("..".to_string());

        // Read from upper layer first
        if let Some((ref upper_mount, ref upper_node)) = self.upper {
            if let Ok(upper_node) = self.resolve_in_layer(upper_mount, upper_node, &overlay_node.path) {
                let fs = upper_node.filesystem().and_then(|w| w.upgrade()).ok_or_else(|| FileSystemError::new(FileSystemErrorKind::NotSupported, "Node has no filesystem"))?;
                if let Ok(upper_entries) = fs.readdir(&upper_node) {
                    for entry in upper_entries {
                        // Skip whiteout files themselves and . .. entries
                        if entry.name.starts_with(".wh.") || entry.name == "." || entry.name == ".." {
                            continue;
                        }
                        if !seen_names.contains(&entry.name) {
                            seen_names.insert(entry.name.clone());
                            entries.push(entry);
                        }
                    }
                }
            }
        }

        // Read from lower layers (skip entries already seen in upper layers)
        for (lower_mount, lower_node) in &self.lower_layers {
            if let Ok(lower_node) = self.resolve_in_layer(lower_mount, lower_node, &overlay_node.path) {
                let fs = lower_node.filesystem().and_then(|w| w.upgrade()).ok_or_else(|| FileSystemError::new(FileSystemErrorKind::NotSupported, "Node has no filesystem"))?;
                if let Ok(lower_entries) = fs.readdir(&lower_node) {
                    for entry in lower_entries {
                        // Skip . .. entries
                        if entry.name == "." || entry.name == ".." {
                            continue;
                        }
                        let entry_full_path = if overlay_node.path == "/" {
                            format!("/{}", entry.name)
                        } else {
                            format!("{}/{}", overlay_node.path, entry.name)
                        };
                        // Only add if not already seen and not hidden by whiteout
                        if !seen_names.contains(&entry.name) && !self.is_whiteout(&entry_full_path) {
                            seen_names.insert(entry.name.clone());
                            entries.push(entry);
                        }
                    }
                }
            }
        }
        Ok(entries)
    }
}
