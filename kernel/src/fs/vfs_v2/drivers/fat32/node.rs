//! FAT32 VFS Node Implementation
//!
//! This module implements the VfsNode trait for FAT32 filesystem nodes.
//! It provides the interface between the VFS layer and FAT32-specific node data.

use alloc::{
    boxed::Box,
    collections::BTreeMap,
    string::String,
    sync::{Arc, Weak},
    vec,
    vec::Vec,
};
use core::{any::Any, fmt::Debug};
use spin::{rwlock::RwLock, Mutex};

use crate::fs::{
    FileMetadata, FileObject, FilePermission, FileSystemError, FileSystemErrorKind, FileType,
    SeekFrom,
};
use crate::object::capability::{ControlOps, MemoryMappingOps, StreamError, StreamOps};

use crate::environment::PAGE_SIZE;
use crate::fs::vfs_v2::cache::PageCacheCapable;
use crate::fs::vfs_v2::core::{FileSystemOperations, VfsNode};
use crate::mem::{page::ContiguousPages, page_cache::PageCacheManager};
use crate::vm::addr::phys_to_virt;

/// FAT32 filesystem node
///
/// This structure represents a file or directory in the FAT32 filesystem.
/// It implements the VfsNode trait to integrate with the VFS v2 architecture.
/// Content is read/written directly from/to the block device, not stored in memory.
pub struct Fat32Node {
    /// Node name
    pub name: RwLock<String>,
    /// File type (file or directory)
    pub file_type: RwLock<FileType>,
    /// File metadata
    pub metadata: RwLock<FileMetadata>,
    /// Child nodes (for directories) - cached, but loaded from disk on demand
    pub children: RwLock<BTreeMap<String, Arc<dyn VfsNode>>>,
    /// Parent node (weak reference to avoid cycles)
    pub parent: RwLock<Option<Weak<Fat32Node>>>,
    /// Reference to filesystem
    pub filesystem: RwLock<Option<Weak<dyn FileSystemOperations>>>,
    /// Starting cluster number in FAT32
    pub cluster: RwLock<u32>,
    /// Directory entries loaded flag (for directories)
    pub children_loaded: RwLock<bool>,
}

impl Debug for Fat32Node {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Fat32Node")
            .field("name", &self.name.read())
            .field("file_type", &self.file_type.read())
            .field("metadata", &self.metadata.read())
            .field("cluster", &self.cluster.read())
            .field("children_loaded", &self.children_loaded.read())
            .field(
                "parent",
                &self.parent.read().as_ref().map(|p| p.strong_count()),
            )
            .finish()
    }
}

impl Fat32Node {
    /// Create a new regular file node
    pub fn new_file(name: String, file_id: u64, cluster: u32) -> Self {
        Self {
            name: RwLock::new(name),
            file_type: RwLock::new(FileType::RegularFile),
            metadata: RwLock::new(FileMetadata {
                file_type: FileType::RegularFile,
                size: 0,
                permissions: FilePermission {
                    read: true,
                    write: true,
                    execute: false,
                },
                created_time: 0, // TODO: Convert FAT32 timestamps
                modified_time: 0,
                accessed_time: 0,
                file_id,
                link_count: 1,
            }),
            children: RwLock::new(BTreeMap::new()),
            parent: RwLock::new(None),
            filesystem: RwLock::new(None),
            cluster: RwLock::new(cluster),
            children_loaded: RwLock::new(false),
        }
    }

    /// Create a new directory node
    pub fn new_directory(name: String, file_id: u64, cluster: u32) -> Self {
        Self {
            name: RwLock::new(name),
            file_type: RwLock::new(FileType::Directory),
            metadata: RwLock::new(FileMetadata {
                file_type: FileType::Directory,
                size: 0,
                permissions: FilePermission {
                    read: true,
                    write: true,
                    execute: true, // Directories need execute permission for traversal
                },
                created_time: 0, // TODO: Convert FAT32 timestamps
                modified_time: 0,
                accessed_time: 0,
                file_id,
                link_count: 1,
            }),
            children: RwLock::new(BTreeMap::new()),
            parent: RwLock::new(None),
            filesystem: RwLock::new(None),
            cluster: RwLock::new(cluster),
            children_loaded: RwLock::new(false),
        }
    }

    /// Set the parent node (weak reference)
    pub fn set_parent(&self, parent: Option<Weak<Fat32Node>>) {
        *self.parent.write() = parent;
    }

    /// Set the filesystem reference
    pub fn set_filesystem(&self, filesystem: Weak<dyn FileSystemOperations>) {
        *self.filesystem.write() = Some(filesystem);
    }

    /// Get the starting cluster number
    pub fn cluster(&self) -> u32 {
        *self.cluster.read()
    }

    /// Set the starting cluster number
    pub fn set_cluster(&self, cluster: u32) {
        *self.cluster.write() = cluster;
    }
}

impl VfsNode for Fat32Node {
    fn id(&self) -> u64 {
        self.metadata.read().file_id
    }

    fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>> {
        self.filesystem.read().clone()
    }

    fn file_type(&self) -> Result<FileType, FileSystemError> {
        Ok(self.file_type.read().clone())
    }

    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        Ok(self.metadata.read().clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Clone for Fat32Node {
    fn clone(&self) -> Self {
        Self {
            name: RwLock::new(self.name.read().clone()),
            file_type: RwLock::new(self.file_type.read().clone()),
            metadata: RwLock::new(self.metadata.read().clone()),
            children: RwLock::new(self.children.read().clone()),
            parent: RwLock::new(self.parent.read().clone()),
            filesystem: RwLock::new(self.filesystem.read().clone()),
            cluster: RwLock::new(*self.cluster.read()),
            children_loaded: RwLock::new(*self.children_loaded.read()),
        }
    }
}

/// FAT32 file object for regular files
pub struct Fat32FileObject {
    /// Reference to the FAT32 node
    node: Arc<Fat32Node>,
    /// Current file position
    position: RwLock<usize>,
    /// Parent directory cluster (for directory entry updates)
    parent_cluster: u32,
    /// File-level dirty flag to avoid unnecessary writeback
    dirty: Mutex<bool>,
    /// Page-aligned backing for mmap operations (lazy initialized)
    mmap_backing: RwLock<Option<ContiguousPages>>,
    /// Byte length of the mmap backing (file size snapshot)
    mmap_backing_len: Mutex<usize>,
    /// Active mmap ranges keyed by starting virtual address
    mmap_ranges: RwLock<BTreeMap<usize, MmapRange>>,
}

impl Fat32FileObject {
    pub fn new(node: Arc<Fat32Node>, parent_cluster: u32) -> Self {
        Self {
            node,
            position: RwLock::new(0),
            parent_cluster,
            dirty: Mutex::new(false),
            mmap_backing: RwLock::new(None),
            mmap_backing_len: Mutex::new(0),
            mmap_ranges: RwLock::new(BTreeMap::new()),
        }
    }

    /// Write back current page-cache-backed content to disk if dirty.
    fn sync_to_disk(&self) -> Result<(), StreamError> {
        let size = self.node.metadata.read().size;
        if size > 0 {
            let dirty = self.dirty.lock();
            if !*dirty {
                return Ok(());
            }
        }
        if size == 0 {
            let fs = self
                .node
                .filesystem
                .read()
                .as_ref()
                .and_then(|w| w.upgrade())
                .ok_or(StreamError::Closed)?;
            let fat32_fs = fs
                .as_any()
                .downcast_ref::<crate::fs::vfs_v2::drivers::fat32::Fat32FileSystem>()
                .ok_or(StreamError::NotSupported)?;
            let current_cluster = self.node.cluster();
            if current_cluster != 0 {
                self.update_directory_entry(fat32_fs, current_cluster, 0)?;
            }
            *self.dirty.lock() = false;
            return Ok(());
        }

        let fs = self
            .node
            .filesystem
            .read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(StreamError::Closed)?;
        let fat32_fs = fs
            .as_any()
            .downcast_ref::<crate::fs::vfs_v2::drivers::fat32::Fat32FileSystem>()
            .ok_or(StreamError::NotSupported)?;

        let mut buffer = Vec::with_capacity(size);
        buffer.resize(size, 0);
        let cache_id = self.cache_id();
        let page_count = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        for page_index in 0..(page_count as u64) {
            let start = page_index as usize * PAGE_SIZE;
            let len = core::cmp::min(PAGE_SIZE, size.saturating_sub(start));
            if len == 0 {
                break;
            }
            let pinned = if let Some(p) = PageCacheManager::global().try_pin(cache_id, page_index) {
                p
            } else {
                PageCacheManager::global()
                    .pin_or_load(cache_id, page_index, |paddr| {
                        let current_cluster = self.node.cluster();
                        if current_cluster == 0 {
                            unsafe {
                                core::ptr::write_bytes(paddr as *mut u8, 0, PAGE_SIZE);
                            }
                            return Ok(());
                        }
                        let data = fat32_fs
                            .read_file_content(current_cluster, size)
                            .map_err(|_| "io error")?;
                        let start = page_index as usize * PAGE_SIZE;
                        let len = core::cmp::min(PAGE_SIZE, data.len().saturating_sub(start));
                        unsafe {
                            core::ptr::write_bytes(paddr as *mut u8, 0, PAGE_SIZE);
                            if len > 0 {
                                core::ptr::copy_nonoverlapping(
                                    data.as_ptr().add(start),
                                    paddr as *mut u8,
                                    len,
                                );
                            }
                        }
                        Ok(())
                    })
                    .map_err(|_| StreamError::IoError)?
            };

            unsafe {
                core::ptr::copy_nonoverlapping(
                    phys_to_virt(pinned.paddr()) as *const u8,
                    buffer.as_mut_ptr().add(start),
                    len,
                );
            }
        }

        let current_cluster = self.node.cluster();
        let old_cache_id = self.cache_id();
        let new_cluster = if buffer.is_empty() {
            0
        } else {
            fat32_fs
                .write_file_content(current_cluster, &buffer)
                .map_err(|_| StreamError::IoError)?
        };

        if new_cluster != current_cluster {
            *self.node.cluster.write() = new_cluster;
            {
                let mut meta = self.node.metadata.write();
                if new_cluster != 0 {
                    meta.file_id = new_cluster as u64;
                }
            }
        } else if current_cluster != 0 {
            let mut meta = self.node.metadata.write();
            if meta.file_id != current_cluster as u64 {
                meta.file_id = current_cluster as u64;
            }
        }

        self.update_directory_entry(fat32_fs, new_cluster, buffer.len())?;
        PageCacheManager::global().invalidate(old_cache_id);

        {
            let mut metadata = self.node.metadata.write();
            metadata.size = buffer.len();
        }

        *self.dirty.lock() = false;
        Ok(())
    }

    fn ensure_mmap_backing(
        &self,
        file_size: usize,
        required_size: usize,
    ) -> Result<(), StreamError> {
        if file_size == 0 || required_size == 0 {
            return Err(StreamError::InvalidArgument);
        }

        let num_pages = (required_size + PAGE_SIZE - 1) / PAGE_SIZE;
        let mut backing_guard = self.mmap_backing.write();
        let needs_alloc = backing_guard
            .as_ref()
            .map(|buf| buf.len() < num_pages)
            .unwrap_or(true);
        if needs_alloc {
            *backing_guard = Some(ContiguousPages::new(num_pages).ok_or(StreamError::IoError)?);
        }

        let backing = backing_guard.as_mut().expect("mmap backing missing");
        *self.mmap_backing_len.lock() = file_size;

        let fs = self
            .node
            .filesystem
            .read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(StreamError::Closed)?;
        let fat32_fs = fs
            .as_any()
            .downcast_ref::<crate::fs::vfs_v2::drivers::fat32::Fat32FileSystem>()
            .ok_or(StreamError::NotSupported)?;

        let cache_id = self.cache_id();
        let backing_ptr = backing.as_ptr() as *mut u8;
        for page_index in 0..num_pages {
            let pinned = PageCacheManager::global()
                .pin_or_load(cache_id, page_index as u64, |paddr| {
                    fat32_fs
                        .read_page_content(self.node.cluster(), page_index as u64, paddr)
                        .map_err(|_| "fat32: read_page_content failed")
                })
                .map_err(|_| StreamError::IoError)?;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    phys_to_virt(pinned.paddr()) as *const u8,
                    backing_ptr.add(page_index * PAGE_SIZE),
                    PAGE_SIZE,
                );
            }
        }

        Ok(())
    }

    /// Update the directory entry for this file
    fn update_directory_entry(
        &self,
        fat32_fs: &crate::fs::vfs_v2::drivers::fat32::Fat32FileSystem,
        cluster: u32,
        size: usize,
    ) -> Result<(), StreamError> {
        // Determine the actual parent cluster to use
        let actual_parent_cluster = if self.parent_cluster == 0 {
            // For files in root directory, use the root cluster
            fat32_fs.root_cluster
        } else {
            self.parent_cluster
        };

        // crate::early_println!("[FAT32] Debug: parent_cluster={}, actual_parent_cluster={}, updating file with cluster={}, size={}",
        //                        self.parent_cluster, actual_parent_cluster, cluster, size);

        // Create updated directory entry
        let filename = self.node.name.read().clone();
        // crate::early_println!("[FAT32] Debug: Updating directory entry for filename: '{}'", filename);

        let dir_entry =
            crate::fs::vfs_v2::drivers::fat32::structures::Fat32DirectoryEntry::new_file(
                &filename,
                cluster,
                size as u32,
            );

        // Write the updated directory entry
        match fat32_fs.update_directory_entry(actual_parent_cluster, &filename, &dir_entry) {
            Ok(()) => {
                // crate::early_println!("[FAT32] Debug: Successfully updated directory entry");
                Ok(())
            }
            Err(e) => {
                crate::early_println!("[FAT32] Error: Failed to update directory entry: {:?}", e);
                Err(StreamError::IoError)
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct MmapRange {
    vaddr_start: usize,
    vaddr_end: usize,
    offset: usize,
}

impl Debug for Fat32FileObject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Fat32FileObject")
            .field("node", &self.node.name.read())
            .field("position", &self.position.read())
            .field("dirty", &self.dirty.lock())
            .finish()
    }
}

impl StreamOps for Fat32FileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let file_size = self.node.metadata.read().size;
        let mut pos = *self.position.read();
        if pos >= file_size {
            return Ok(0);
        }

        let mut total_read = 0usize;
        while total_read < buffer.len() && pos < file_size {
            let page_index = (pos / PAGE_SIZE) as u64;
            let offset_in_page = pos % PAGE_SIZE;
            let cache_id = self.cache_id();
            let start_cluster = self.node.cluster();
            let pinned = PageCacheManager::global()
                .pin_or_load(cache_id, page_index, |paddr| {
                    let fs = self
                        .node
                        .filesystem
                        .read()
                        .as_ref()
                        .and_then(|w| w.upgrade())
                        .ok_or("filesystem gone")
                        .map_err(|_| "filesystem gone")?;
                    let fat32_fs = fs
                        .as_any()
                        .downcast_ref::<crate::fs::vfs_v2::drivers::fat32::Fat32FileSystem>()
                        .ok_or("bad fs type")?;
                    fat32_fs
                        .read_page_content(start_cluster, page_index, paddr)
                        .map_err(|_| "io error")
                })
                .map_err(|_| StreamError::IoError)?;

            unsafe {
                let src = (phys_to_virt(pinned.paddr()) as *const u8).add(offset_in_page);
                let remaining_in_page = PAGE_SIZE - offset_in_page;
                let remaining_file = file_size - pos;
                let remaining_buf = buffer.len() - total_read;
                let chunk = core::cmp::min(
                    remaining_in_page,
                    core::cmp::min(remaining_file, remaining_buf),
                );
                core::ptr::copy_nonoverlapping(src, buffer.as_mut_ptr().add(total_read), chunk);
                total_read += chunk;
                pos += chunk;
            }
        }

        {
            let mut position = self.position.write();
            *position = pos;
        }
        Ok(total_read)
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        let cache_id = self.cache_id();
        let mut written = 0usize;
        let mut pos = *self.position.read();
        let start_cluster = self.node.cluster();

        while written < buffer.len() {
            let page_index = (pos / PAGE_SIZE) as u64;
            let page_off = pos % PAGE_SIZE;
            let remain_in_page = PAGE_SIZE - page_off;
            let chunk = core::cmp::min(buffer.len() - written, remain_in_page);

            let pinned = PageCacheManager::global()
                .pin_or_load(cache_id, page_index, |paddr| {
                    let fs = self
                        .node
                        .filesystem
                        .read()
                        .as_ref()
                        .and_then(|w| w.upgrade())
                        .ok_or("filesystem gone")?;
                    let fat32_fs = fs
                        .as_any()
                        .downcast_ref::<crate::fs::vfs_v2::drivers::fat32::Fat32FileSystem>()
                        .ok_or("bad fs type")?;
                    fat32_fs
                        .read_page_content(start_cluster, page_index, paddr)
                        .map_err(|_| "io error")
                })
                .map_err(|_| StreamError::IoError)?;

            unsafe {
                let dst = (phys_to_virt(pinned.paddr()) as *mut u8).add(page_off);
                let src = buffer.as_ptr().add(written);
                core::ptr::copy_nonoverlapping(src, dst, chunk);
            }
            pinned.mark_dirty();

            written += chunk;
            pos += chunk;
        }

        {
            let mut position = self.position.write();
            *position += written;
        }

        {
            let mut meta = self.node.metadata.write();
            let new_end = (*self.position.read()) as usize;
            if new_end > meta.size {
                meta.size = new_end;
            }
        }

        *self.dirty.lock() = true;

        Ok(written)
    }
}

impl ControlOps for Fat32FileObject {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported for FAT32 files")
    }
}

impl MemoryMappingOps for Fat32FileObject {
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        if offset % PAGE_SIZE != 0 {
            return Err("Offset not page aligned");
        }

        let file_size = self.node.metadata.read().size;
        if file_size == 0 || offset >= file_size {
            return Err("Offset beyond file size");
        }

        let required_size = offset.saturating_add(length).max(file_size);
        self.ensure_mmap_backing(file_size, required_size)
            .map_err(|_| "Failed to prepare mmap backing")?;

        let backing_guard = self.mmap_backing.read();
        let backing = backing_guard.as_ref().ok_or("mmap backing missing")?;
        let base = backing.as_ptr() as usize;
        let paddr = base + offset;
        if paddr % PAGE_SIZE != 0 {
            return Err("Backing address not aligned");
        }

        Ok((paddr, 0x3, true))
    }

    fn get_mapping_info_with(
        &self,
        offset: usize,
        length: usize,
        is_shared: bool,
    ) -> Result<(usize, usize, bool), &'static str> {
        if is_shared {
            if offset % PAGE_SIZE != 0 {
                return Err("Offset not page aligned");
            }

            let file_size = self.node.metadata.read().size;
            if file_size == 0 || offset >= file_size {
                return Err("Offset beyond file size");
            }

            let _ = length;
            return Ok((0, 0x3, true));
        }

        self.get_mapping_info(offset, length)
    }

    fn on_mapped(&self, vaddr: usize, _paddr: usize, length: usize, offset: usize) {
        if length == 0 {
            return;
        }
        let vaddr_end = vaddr.saturating_add(length - 1);
        let range = MmapRange {
            vaddr_start: vaddr,
            vaddr_end,
            offset,
        };
        self.mmap_ranges.write().insert(vaddr, range);
    }

    fn on_unmapped(&self, vaddr: usize, _length: usize) {
        self.mmap_ranges.write().remove(&vaddr);
        let backing_guard = self.mmap_backing.read();
        let backing = match backing_guard.as_ref() {
            Some(buf) => buf,
            None => {
                let _ = self.sync_to_disk();
                return;
            }
        };
        let backing_len = *self.mmap_backing_len.lock();
        if backing_len == 0 {
            return;
        }

        let fs = match self
            .node
            .filesystem
            .read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
        {
            Some(fs) => fs,
            None => return,
        };
        let fat32_fs = match fs
            .as_any()
            .downcast_ref::<crate::fs::vfs_v2::drivers::fat32::Fat32FileSystem>()
        {
            Some(fs) => fs,
            None => return,
        };

        let backing_ptr = backing.as_ptr() as *const u8;
        let data = unsafe { core::slice::from_raw_parts(backing_ptr, backing_len) };
        let _ = fat32_fs.write_file_content(self.node.cluster(), data);
        PageCacheManager::global().invalidate(self.cache_id());
    }

    fn supports_mmap(&self) -> bool {
        true
    }

    fn resolve_fault(
        &self,
        access: &crate::object::capability::memory_mapping::AccessKind,
        map: &crate::vm::vmem::VirtualMemoryMap,
    ) -> core::result::Result<
        crate::object::capability::memory_mapping::ResolveFaultResult,
        crate::object::capability::memory_mapping::ResolveFaultError,
    > {
        let range = self
            .mmap_ranges
            .read()
            .get(&map.vmarea.start)
            .copied()
            .ok_or(crate::object::capability::memory_mapping::ResolveFaultError::Invalid)?;
        if access.vaddr < range.vaddr_start || access.vaddr > range.vaddr_end {
            return Err(crate::object::capability::memory_mapping::ResolveFaultError::Invalid);
        }

        let file_size = self.node.metadata.read().size;
        let file_offset = range
            .offset
            .saturating_add(access.vaddr.saturating_sub(range.vaddr_start));
        if file_size == 0 || file_offset >= file_size {
            return Err(crate::object::capability::memory_mapping::ResolveFaultError::Invalid);
        }

        let fs = self
            .node
            .filesystem
            .read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(crate::object::capability::memory_mapping::ResolveFaultError::Invalid)?;
        let fat32_fs = fs
            .as_any()
            .downcast_ref::<crate::fs::vfs_v2::drivers::fat32::Fat32FileSystem>()
            .ok_or(crate::object::capability::memory_mapping::ResolveFaultError::Invalid)?;

        let page_index = (file_offset / PAGE_SIZE) as u64;
        let pinned = PageCacheManager::global()
            .pin_or_load(self.cache_id(), page_index, |paddr| {
                fat32_fs
                    .read_page_content(self.node.cluster(), page_index, paddr)
                    .map_err(|_| "fat32: read_page_content failed")
            })
            .map_err(|_| crate::object::capability::memory_mapping::ResolveFaultError::Invalid)?;

        if matches!(
            access.op,
            crate::object::capability::memory_mapping::AccessOp::Store
        ) {
            pinned.mark_dirty();
            *self.dirty.lock() = true;
        }

        Ok(
            crate::object::capability::memory_mapping::ResolveFaultResult {
                paddr_page_base: pinned.paddr(),
                is_tail: false,
            },
        )
    }
}

impl FileObject for Fat32FileObject {
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let file_size = self.node.metadata.read().size;
        let off = usize::try_from(offset).map_err(|_| StreamError::InvalidArgument)?;
        if off >= file_size {
            return Ok(0);
        }

        let mut total_read = 0usize;
        let cache_id = self.cache_id();
        while total_read < buffer.len() && off + total_read < file_size {
            let absolute = off + total_read;
            let page_index = (absolute / PAGE_SIZE) as u64;
            let offset_in_page = absolute % PAGE_SIZE;

            let pinned = PageCacheManager::global()
                .pin_or_load(cache_id, page_index, |paddr| {
                    let fs = self
                        .node
                        .filesystem
                        .read()
                        .as_ref()
                        .and_then(|w| w.upgrade())
                        .ok_or("filesystem gone")?;
                    let fat32_fs = fs
                        .as_any()
                        .downcast_ref::<crate::fs::vfs_v2::drivers::fat32::Fat32FileSystem>()
                        .ok_or("bad fs type")?;
                    fat32_fs
                        .read_page_content(self.node.cluster(), page_index, paddr)
                        .map_err(|_| "io error")
                })
                .map_err(|_| StreamError::IoError)?;

            unsafe {
                let src = (pinned.paddr() as *const u8).add(offset_in_page);
                let remaining_in_page = PAGE_SIZE - offset_in_page;
                let remaining_file = file_size - (off + total_read);
                let remaining_buf = buffer.len() - total_read;
                let chunk = core::cmp::min(
                    remaining_in_page,
                    core::cmp::min(remaining_file, remaining_buf),
                );
                core::ptr::copy_nonoverlapping(src, buffer.as_mut_ptr().add(total_read), chunk);
                total_read += chunk;
            }
        }

        Ok(total_read)
    }

    fn write_at(&self, offset: u64, buffer: &[u8]) -> Result<usize, StreamError> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let off = usize::try_from(offset).map_err(|_| StreamError::InvalidArgument)?;
        let mut written = 0usize;
        let cache_id = self.cache_id();

        while written < buffer.len() {
            let absolute = off + written;
            let page_index = (absolute / PAGE_SIZE) as u64;
            let page_off = absolute % PAGE_SIZE;
            let remain_in_page = PAGE_SIZE - page_off;
            let chunk = core::cmp::min(buffer.len() - written, remain_in_page);

            let pinned = PageCacheManager::global()
                .pin_or_load(cache_id, page_index, |paddr| {
                    let fs = self
                        .node
                        .filesystem
                        .read()
                        .as_ref()
                        .and_then(|w| w.upgrade())
                        .ok_or("filesystem gone")?;
                    let fat32_fs = fs
                        .as_any()
                        .downcast_ref::<crate::fs::vfs_v2::drivers::fat32::Fat32FileSystem>()
                        .ok_or("bad fs type")?;
                    fat32_fs
                        .read_page_content(self.node.cluster(), page_index, paddr)
                        .map_err(|_| "io error")
                })
                .map_err(|_| StreamError::IoError)?;

            unsafe {
                let dst = (pinned.paddr() as *mut u8).add(page_off);
                let src = buffer.as_ptr().add(written);
                core::ptr::copy_nonoverlapping(src, dst, chunk);
            }
            pinned.mark_dirty();
            written += chunk;
        }

        let new_end = off + written;
        {
            let mut meta = self.node.metadata.write();
            if new_end > meta.size {
                meta.size = new_end;
            }
        }

        *self.dirty.lock() = true;

        Ok(written)
    }

    fn truncate(&self, size: u64) -> Result<(), StreamError> {
        if *self.node.file_type.read() != FileType::RegularFile {
            return Err(StreamError::from(FileSystemError::new(
                FileSystemErrorKind::IsADirectory,
                "Cannot truncate non-regular file",
            )));
        }

        let new_size = usize::try_from(size).map_err(|_| StreamError::InvalidArgument)?;
        let old_size = self.node.metadata.read().size;
        if new_size == old_size {
            return Ok(());
        }

        let fs = self
            .node
            .filesystem
            .read()
            .as_ref()
            .and_then(|w| w.upgrade())
            .ok_or(StreamError::Closed)?;
        let fat32_fs = fs
            .as_any()
            .downcast_ref::<crate::fs::vfs_v2::drivers::fat32::Fat32FileSystem>()
            .ok_or(StreamError::NotSupported)?;

        let mut buffer = Vec::with_capacity(new_size);
        buffer.resize(new_size, 0);
        let copy_len = core::cmp::min(old_size, new_size);
        if copy_len > 0 {
            let cache_id = self.cache_id();
            let page_count = (copy_len + PAGE_SIZE - 1) / PAGE_SIZE;
            for page_index in 0..(page_count as u64) {
                let start = page_index as usize * PAGE_SIZE;
                let len = core::cmp::min(PAGE_SIZE, copy_len.saturating_sub(start));
                if len == 0 {
                    break;
                }
                let pinned = PageCacheManager::global()
                    .pin_or_load(cache_id, page_index, |paddr| {
                        fat32_fs
                            .read_page_content(self.node.cluster(), page_index, paddr)
                            .map_err(|_| "io error")
                    })
                    .map_err(|_| StreamError::IoError)?;
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        phys_to_virt(pinned.paddr()) as *const u8,
                        buffer.as_mut_ptr().add(start),
                        len,
                    );
                }
            }
        }

        let current_cluster = self.node.cluster();
        let old_cache_id = self.cache_id();
        let new_cluster = if buffer.is_empty() {
            0
        } else {
            fat32_fs
                .write_file_content(current_cluster, &buffer)
                .map_err(|_| StreamError::IoError)?
        };

        if new_cluster != current_cluster {
            *self.node.cluster.write() = new_cluster;
            {
                let mut meta = self.node.metadata.write();
                if new_cluster != 0 {
                    meta.file_id = new_cluster as u64;
                }
            }
            self.update_directory_entry(fat32_fs, new_cluster, buffer.len())?;
            PageCacheManager::global().invalidate(old_cache_id);
        } else if current_cluster != 0 {
            let mut meta = self.node.metadata.write();
            if meta.file_id != current_cluster as u64 {
                meta.file_id = current_cluster as u64;
                PageCacheManager::global().invalidate(old_cache_id);
            }
        }

        {
            let mut metadata = self.node.metadata.write();
            metadata.size = buffer.len();
        }
        *self.dirty.lock() = false;

        let mut position = self.position.write();
        if *position > size as usize {
            *position = size as usize;
        }

        Ok(())
    }

    fn seek(&self, from: SeekFrom) -> Result<u64, StreamError> {
        let metadata = self.node.metadata.read();
        let file_size = metadata.size;
        let mut pos = self.position.write();

        let new_pos = match from {
            SeekFrom::Start(offset) => offset as usize,
            SeekFrom::End(offset) => {
                if offset < 0 {
                    file_size.saturating_sub((-offset) as usize)
                } else {
                    file_size + offset as usize
                }
            }
            SeekFrom::Current(offset) => {
                if offset < 0 {
                    pos.saturating_sub((-offset) as usize)
                } else {
                    *pos + offset as usize
                }
            }
        };

        *pos = new_pos;
        Ok(new_pos as u64)
    }

    fn metadata(&self) -> Result<crate::fs::FileMetadata, StreamError> {
        Ok(self.node.metadata.read().clone())
    }

    fn sync(&self) -> Result<(), StreamError> {
        self.sync_to_disk()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl PageCacheCapable for Fat32FileObject {
    fn cache_id(&self) -> crate::fs::vfs_v2::cache::CacheId {
        let fs = self
            .node
            .filesystem
            .read()
            .as_ref()
            .and_then(|w| w.upgrade())
            .expect("Fat32FileObject: filesystem gone");
        let fat32_fs = fs
            .as_any()
            .downcast_ref::<crate::fs::vfs_v2::drivers::fat32::Fat32FileSystem>()
            .expect("Fat32FileObject: invalid filesystem type");

        let fs_id = fat32_fs.fs_id().get();
        let file_key = self.node.metadata.read().file_id;
        let cache_id = (fs_id << 32) | file_key;
        crate::fs::vfs_v2::cache::CacheId::new(cache_id)
    }
}

impl crate::object::capability::selectable::Selectable for Fat32FileObject {
    fn current_ready(
        &self,
        interest: crate::object::capability::selectable::ReadyInterest,
    ) -> crate::object::capability::selectable::ReadySet {
        let mut set = crate::object::capability::selectable::ReadySet::none();
        if interest.read {
            set.read = true;
        }
        if interest.write {
            set.write = true;
        }
        if interest.except {
            set.except = false;
        }
        set
    }

    fn wait_until_ready(
        &self,
        _interest: crate::object::capability::selectable::ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }

    fn is_nonblocking(&self) -> bool {
        true
    }
}

impl Drop for Fat32FileObject {
    fn drop(&mut self) {
        let _ = self.sync_to_disk();
    }
}

/// FAT32 directory object
pub struct Fat32DirectoryObject {
    /// Reference to the FAT32 node
    node: Arc<Fat32Node>,
    /// Current position in directory listing
    position: RwLock<usize>,
}

impl Fat32DirectoryObject {
    pub fn new(node: Arc<Fat32Node>) -> Self {
        Self {
            node,
            position: RwLock::new(0),
        }
    }
}

impl Debug for Fat32DirectoryObject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Fat32DirectoryObject")
            .field("node", &self.node.name.read())
            .field("position", &self.position.read())
            .finish()
    }
}

impl StreamOps for Fat32DirectoryObject {
    fn read(&self, _buffer: &mut [u8]) -> Result<usize, StreamError> {
        Err(StreamError::NotSupported)
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::NotSupported)
    }
}

impl ControlOps for Fat32DirectoryObject {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported for FAT32 directories")
    }
}

impl MemoryMappingOps for Fat32DirectoryObject {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported for FAT32 directories")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // Not supported
    }

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // Not supported
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl FileObject for Fat32DirectoryObject {
    fn seek(&self, from: SeekFrom) -> Result<u64, StreamError> {
        let children = self.node.children.read();
        let mut pos = self.position.write();

        let new_pos = match from {
            SeekFrom::Start(offset) => offset as usize,
            SeekFrom::End(offset) => {
                if offset < 0 {
                    children.len().saturating_sub((-offset) as usize)
                } else {
                    children.len() + offset as usize
                }
            }
            SeekFrom::Current(offset) => {
                if offset < 0 {
                    pos.saturating_sub((-offset) as usize)
                } else {
                    *pos + offset as usize
                }
            }
        };

        *pos = new_pos;
        Ok(new_pos as u64)
    }

    fn metadata(&self) -> Result<crate::fs::FileMetadata, StreamError> {
        Ok(self.node.metadata.read().clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl crate::object::capability::selectable::Selectable for Fat32DirectoryObject {
    fn current_ready(
        &self,
        interest: crate::object::capability::selectable::ReadyInterest,
    ) -> crate::object::capability::selectable::ReadySet {
        let mut set = crate::object::capability::selectable::ReadySet::none();
        if interest.read {
            set.read = true;
        }
        if interest.write {
            set.write = true;
        }
        if interest.except {
            set.except = false;
        }
        set
    }

    fn wait_until_ready(
        &self,
        _interest: crate::object::capability::selectable::ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }

    fn is_nonblocking(&self) -> bool {
        true
    }
}
