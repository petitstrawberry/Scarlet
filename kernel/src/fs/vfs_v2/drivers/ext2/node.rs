//! ext2 VFS Node Implementation
//!
//! This module implements the VFS node interface for ext2 filesystem nodes,
//! providing file and directory objects that integrate with the VFS v2 architecture.

use crate::sync::{IrqRwSpinLock, IrqSpinLock};
use alloc::{boxed::Box, collections::BTreeMap, format, string::String, sync::Weak, vec, vec::Vec};
use core::{any::Any, fmt::Debug};

use crate::object::capability::selectable::{
    ReadyInterest, ReadySet, SelectWaitOutcome, Selectable,
};
use crate::{
    DeviceManager,
    environment::PAGE_SIZE,
    fs::{
        DeviceFileInfo, FileMetadata, FileObject, FilePermission, FileSystemError,
        FileSystemErrorKind, FileType, SeekFrom, SocketFileInfo, vfs_v2::cache::PageCacheCapable,
    },
    mem::{
        page::ContiguousPages,
        page_cache::{PageCacheManager, PageIndex},
    },
    object::capability::{ControlOps, MemoryMappingOps, StreamError, StreamOps},
    vm::addr::phys_to_virt,
};

use super::{
    Ext2FileSystem,
    structures::{EXT2_S_IFDIR, EXT2_S_IFMT, EXT2_S_IFREG},
};
use crate::fs::vfs_v2::core::{FileSystemOperations, VfsNode};

/// ext2 VFS Node
///
/// Represents a file or directory in the ext2 filesystem. This node
/// implements the VfsNode trait and provides access to ext2-specific
/// file operations.
#[derive(Debug)]
pub struct Ext2Node {
    /// Inode number in the ext2 filesystem
    inode_number: u32,
    /// File type (directory, regular file, etc.)
    file_type: FileType,
    /// Unique file ID for VFS
    file_id: u64,
    /// Weak reference to the filesystem
    filesystem: IrqRwSpinLock<Option<Weak<dyn FileSystemOperations>>>,
}

impl Ext2Node {
    /// Create a new ext2 node
    pub fn new(inode_number: u32, file_type: FileType, file_id: u64) -> Self {
        Self {
            inode_number,
            file_type,
            file_id,
            filesystem: IrqRwSpinLock::new(None),
        }
    }

    /// Get the inode number
    pub fn inode_number(&self) -> u32 {
        self.inode_number
    }

    /// Set the filesystem reference
    pub fn set_filesystem(&self, fs: Weak<dyn FileSystemOperations>) {
        *self.filesystem.write() = Some(fs);
    }

    /// Get the filesystem reference
    pub fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>> {
        self.filesystem.read().clone()
    }
}

impl VfsNode for Ext2Node {
    fn id(&self) -> u64 {
        self.file_id
    }

    fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>> {
        self.filesystem.read().clone()
    }

    fn file_type(&self) -> Result<FileType, FileSystemError> {
        Ok(self.file_type.clone())
    }

    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        crate::profile_scope!("ext2::node::metadata");

        // Read the actual inode to get real metadata
        let filesystem = self
            .filesystem()
            .and_then(|weak_fs| weak_fs.upgrade())
            .ok_or_else(|| {
                FileSystemError::new(
                    FileSystemErrorKind::NotSupported,
                    "Filesystem not available",
                )
            })?;

        let ext2_fs = filesystem
            .as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or_else(|| {
                FileSystemError::new(FileSystemErrorKind::NotSupported, "Invalid filesystem type")
            })?;

        let inode = ext2_fs.read_inode(self.inode_number)?;

        // Convert inode mode to permissions
        let mode = inode.get_mode();
        let permissions = FilePermission {
            read: (mode & 0o444) != 0,
            write: (mode & 0o222) != 0,
            execute: (mode & 0o111) != 0,
        };

        let cache_id =
            crate::fs::vfs_v2::cache::CacheId::new((ext2_fs.fs_id().get() << 32) | self.file_id);
        let size = PageCacheManager::global()
            .cached_object_size(cache_id)
            .unwrap_or_else(|| inode.get_size() as usize);

        Ok(FileMetadata {
            file_type: self.file_type.clone(),
            size,
            permissions,
            created_time: inode.get_ctime() as u64,
            modified_time: inode.get_mtime() as u64,
            accessed_time: 0,
            file_id: self.file_id,
            link_count: 1,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read_link(&self) -> Result<String, FileSystemError> {
        // Check if this is actually a symbolic link
        if !matches!(self.file_type, FileType::SymbolicLink(_)) {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Not a symbolic link",
            ));
        }

        // Get filesystem reference
        let filesystem = self
            .filesystem()
            .and_then(|weak_fs| weak_fs.upgrade())
            .ok_or_else(|| {
                FileSystemError::new(
                    FileSystemErrorKind::NotSupported,
                    "Filesystem not available",
                )
            })?;

        let ext2_fs = filesystem
            .as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or_else(|| {
                FileSystemError::new(FileSystemErrorKind::NotSupported, "Invalid filesystem type")
            })?;

        // Read the inode and use the new read_symlink_target method
        let inode = ext2_fs.read_inode(self.inode_number)?;
        inode.read_symlink_target(ext2_fs)
    }
}

/// ext2 File Object
///
/// Handles file operations for regular files in the ext2 filesystem.
#[derive(Debug)]
pub struct Ext2FileObject {
    /// Inode number of the file
    inode_number: u32,
    /// File ID
    file_id: u64,
    /// Current position in the file
    position: IrqSpinLock<u64>,
    /// Optional logical size override after in-memory writes (not yet flushed)
    size_override: IrqSpinLock<Option<usize>>,
    /// Dirty flag indicating in-memory changes not yet persisted to disk
    dirty: IrqSpinLock<bool>,
    /// Weak reference to the filesystem
    filesystem: IrqRwSpinLock<Option<Weak<dyn FileSystemOperations>>>,
    /// Page-aligned backing for mmap operations (lazy initialized)
    mmap_backing: IrqRwSpinLock<Option<ContiguousPages>>,
    /// Byte length of the mmap backing (file size snapshot)
    mmap_backing_len: IrqSpinLock<usize>,
    /// Active mmap ranges keyed by starting virtual address
    mmap_ranges: IrqRwSpinLock<BTreeMap<usize, MmapRange>>,
}

impl Ext2FileObject {
    /// Create a new ext2 file object
    pub fn new(inode_number: u32, file_id: u64) -> Self {
        Self {
            inode_number,
            file_id,
            position: IrqSpinLock::new(0),
            size_override: IrqSpinLock::new(None),
            dirty: IrqSpinLock::new(false),
            filesystem: IrqRwSpinLock::new(None),
            mmap_backing: IrqRwSpinLock::new(None),
            mmap_backing_len: IrqSpinLock::new(0),
            mmap_ranges: IrqRwSpinLock::new(BTreeMap::new()),
        }
    }

    /// Set the filesystem reference
    pub fn set_filesystem(&self, fs: Weak<dyn FileSystemOperations>) {
        *self.filesystem.write() = Some(fs);
    }

    /// Get the file ID
    pub fn file_id(&self) -> u64 {
        self.file_id
    }

    /// Flush current page-cache-backed content to disk.
    fn sync_to_disk(&self) -> Result<(), StreamError> {
        let no_size_override = self.size_override.lock().is_none();
        let is_dirty = *self.dirty.lock();
        if no_size_override && !is_dirty {
            return Ok(());
        }

        let fs = self
            .filesystem
            .read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(StreamError::Closed)?;
        let ext2_fs = fs
            .as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or(StreamError::NotSupported)?;

        let on_disk = ext2_fs
            .read_inode(self.inode_number)
            .map_err(|e| {
                crate::println!(
                    "[ext2] sync_to_disk: read_inode failed for inode {}: {:?}",
                    self.inode_number,
                    e
                );
                StreamError::IoError
            })?
            .size as usize;
        let eff_size = match *self.size_override.lock() {
            Some(ov) => core::cmp::max(on_disk, ov),
            None => on_disk,
        };

        let cache_id = self.cache_id();
        PageCacheManager::global()
            .flush_batch(cache_id, |pages| {
                ext2_fs
                    .write_cached_pages(self.inode_number, eff_size, pages)
                    .map_err(|_| "ext2 page writeback failed")
            })
            .map_err(|e| {
                crate::println!(
                    "[ext2] sync_to_disk: page writeback failed for inode {} (size {}): {:?}",
                    self.inode_number,
                    eff_size,
                    e
                );
                StreamError::IoError
            })?;

        if !PageCacheManager::global().has_dirty_pages(cache_id) {
            *self.size_override.lock() = None;
            *self.dirty.lock() = false;
        }
        Ok(())
    }

    fn effective_size(&self, inode_size: usize) -> usize {
        let mut file_size = inode_size;
        if let Some(override_size) = *self.size_override.lock() {
            if override_size > file_size {
                file_size = override_size;
            }
        }
        file_size
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
            .filesystem
            .read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(StreamError::Closed)?;
        let ext2_fs = fs
            .as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or(StreamError::NotSupported)?;

        let cache_id = self.cache_id();
        let backing_ptr = backing.as_ptr() as *mut u8;
        for page_index in 0..num_pages {
            let pinned = PageCacheManager::global()
                .pin_or_load(cache_id, page_index as u64, |paddr| {
                    ext2_fs
                        .read_page_content(self.inode_number, page_index as u64, paddr)
                        .map_err(|_| "ext2: read_page_content failed")
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
}

#[derive(Clone, Copy, Debug)]
struct MmapRange {
    vaddr_start: usize,
    vaddr_end: usize,
    offset: usize,
}

impl StreamOps for Ext2FileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        crate::profile_scope!("ext2::node::read");

        let fs = self
            .filesystem
            .read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(StreamError::Closed)?;
        let ext2_fs = fs
            .as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or(StreamError::NotSupported)?;

        let inode = ext2_fs
            .read_inode(self.inode_number)
            .map_err(|_| StreamError::IoError)?;
        let cache_id = self.cache_id();
        let mut file_size = PageCacheManager::global()
            .cached_object_size(cache_id)
            .unwrap_or(inode.size as usize);
        if let Some(override_size) = *self.size_override.lock() {
            if override_size > file_size {
                file_size = override_size;
            }
        }

        if buffer.is_empty() {
            return Ok(0);
        }

        loop {
            // Snapshot the shared file position without retaining an IRQ-off
            // guard across page-cache or block-device I/O.
            let current_pos =
                usize::try_from(*self.position.lock()).map_err(|_| StreamError::InvalidArgument)?;
            if current_pos >= file_size {
                return Ok(0);
            }

            let bytes_to_read = core::cmp::min(buffer.len(), file_size - current_pos);
            let end_pos = current_pos
                .checked_add(bytes_to_read)
                .ok_or(StreamError::InvalidArgument)?;
            let first_page = (current_pos / PAGE_SIZE) as PageIndex;
            let last_page = ((end_pos - 1) / PAGE_SIZE) as PageIndex;
            let page_count = usize::try_from(last_page - first_page + 1)
                .map_err(|_| StreamError::InvalidArgument)?;
            let mut pinned_pages = Vec::with_capacity(page_count);

            for page_index in first_page..=last_page {
                let pinned = PageCacheManager::global()
                    .pin_or_load(cache_id, page_index, |paddr| {
                        ext2_fs
                            .read_page_content(self.inode_number, page_index, paddr)
                            .map_err(|_| "Failed to load page")
                    })
                    .map_err(|_| StreamError::IoError)?;
                pinned_pages.push(pinned);
            }

            // Reserve exactly the range whose pages were loaded. If another
            // thread advanced this shared file description meanwhile, discard
            // these pins and retry from the new position.
            let reserved = {
                let mut position = self.position.lock();
                if usize::try_from(*position).ok() != Some(current_pos) {
                    false
                } else {
                    *position = u64::try_from(end_pos).map_err(|_| StreamError::InvalidArgument)?;
                    true
                }
            };
            if !reserved {
                continue;
            }

            let mut copied = 0usize;
            let mut pos = current_pos;
            while copied < bytes_to_read {
                let page_index = (pos / PAGE_SIZE) as PageIndex;
                let page_offset = pos % PAGE_SIZE;
                let bytes_in_page = core::cmp::min(bytes_to_read - copied, PAGE_SIZE - page_offset);
                let pinned = &pinned_pages[(page_index - first_page) as usize];

                unsafe {
                    let src = (phys_to_virt(pinned.paddr()) as *const u8).add(page_offset);
                    let dst = buffer.as_mut_ptr().add(copied);
                    core::ptr::copy_nonoverlapping(src, dst, bytes_in_page);
                }

                copied += bytes_in_page;
                pos += bytes_in_page;
            }

            return Ok(bytes_to_read);
        }
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        crate::profile_scope!("ext2::node::write");

        let fs = self
            .filesystem
            .read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(StreamError::Closed)?;
        let ext2_fs = fs
            .as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or(StreamError::NotSupported)?;

        let bytes_to_write = buffer.len();
        if bytes_to_write == 0 {
            return Ok(0);
        }

        let cache_id = self.cache_id();
        let end_pos = loop {
            let current_pos =
                usize::try_from(*self.position.lock()).map_err(|_| StreamError::InvalidArgument)?;
            let end_pos = current_pos
                .checked_add(bytes_to_write)
                .ok_or(StreamError::InvalidArgument)?;
            let first_page = (current_pos / PAGE_SIZE) as PageIndex;
            let last_page = ((end_pos - 1) / PAGE_SIZE) as PageIndex;
            let page_count = usize::try_from(last_page - first_page + 1)
                .map_err(|_| StreamError::InvalidArgument)?;
            let mut pinned_pages = Vec::with_capacity(page_count);

            for page_index in first_page..=last_page {
                let pinned = PageCacheManager::global()
                    .pin_or_load(cache_id, page_index, |paddr| {
                        ext2_fs
                            .read_page_content(self.inode_number, page_index, paddr)
                            .map_err(|_| "Failed to load page")
                    })
                    .map_err(|_| StreamError::IoError)?;
                pinned_pages.push(pinned);
            }

            let reserved = {
                let mut position = self.position.lock();
                if usize::try_from(*position).ok() != Some(current_pos) {
                    false
                } else {
                    *position = u64::try_from(end_pos).map_err(|_| StreamError::InvalidArgument)?;
                    true
                }
            };
            if !reserved {
                continue;
            }

            let mut written = 0usize;
            let mut pos = current_pos;
            while written < bytes_to_write {
                let page_index = (pos / PAGE_SIZE) as PageIndex;
                let page_offset = pos % PAGE_SIZE;
                let chunk = core::cmp::min(bytes_to_write - written, PAGE_SIZE - page_offset);
                let pinned = &pinned_pages[(page_index - first_page) as usize];

                unsafe {
                    let dst = (phys_to_virt(pinned.paddr()) as *mut u8).add(page_offset);
                    let src = buffer.as_ptr().add(written);
                    core::ptr::copy_nonoverlapping(src, dst, chunk);
                }

                pinned.mark_dirty();
                written += chunk;
                pos += chunk;
            }
            *self.dirty.lock() = true;
            break end_pos;
        };

        let mut override_size = self.size_override.lock();
        let new_end = end_pos;
        match *override_size {
            Some(cur) => {
                if new_end > cur {
                    *override_size = Some(new_end);
                }
            }
            None => {
                let base = ext2_fs
                    .read_inode(self.inode_number)
                    .map_err(|_| StreamError::IoError)?
                    .size as usize;
                if new_end > base {
                    *override_size = Some(new_end);
                }
            }
        }
        drop(override_size);

        let inode_size = ext2_fs
            .read_inode(self.inode_number)
            .map_err(|_| StreamError::IoError)?
            .size as usize;
        PageCacheManager::global().record_object_size(cache_id, self.effective_size(inode_size));

        Ok(bytes_to_write)
    }
}

impl ControlOps for Ext2FileObject {}

impl MemoryMappingOps for Ext2FileObject {
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        if offset % PAGE_SIZE != 0 {
            return Err("Offset not page aligned");
        }

        let fs = self
            .filesystem
            .read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or("Filesystem closed")?;
        let ext2_fs = fs
            .as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or("Invalid filesystem type")?;
        let inode = ext2_fs
            .read_inode(self.inode_number)
            .map_err(|_| "Read inode failed")?;
        let file_size = self.effective_size(inode.size as usize);

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

        Ok(crate::object::capability::MemoryMappingInfo::new(
            paddr, 0x3, true,
        ))
    }

    fn get_mapping_info_with(
        &self,
        offset: usize,
        length: usize,
        is_shared: bool,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        if is_shared {
            if offset % PAGE_SIZE != 0 {
                return Err("Offset not page aligned");
            }

            let fs = self
                .filesystem
                .read()
                .as_ref()
                .and_then(|weak| weak.upgrade())
                .ok_or("Filesystem closed")?;
            let ext2_fs = fs
                .as_any()
                .downcast_ref::<Ext2FileSystem>()
                .ok_or("Invalid filesystem type")?;
            let inode = ext2_fs
                .read_inode(self.inode_number)
                .map_err(|_| "Read inode failed")?;
            let file_size = self.effective_size(inode.size as usize);

            if file_size == 0 || offset >= file_size {
                return Err("Offset beyond file size");
            }

            let _ = length;
            return Ok(crate::object::capability::MemoryMappingInfo::new(
                0, 0x3, true,
            ));
        }

        self.get_mapping_info(offset, length)
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        if _length == 0 {
            return;
        }
        let vaddr_end = _vaddr.saturating_add(_length - 1);
        let range = MmapRange {
            vaddr_start: _vaddr,
            vaddr_end,
            offset: _offset,
        };
        self.mmap_ranges.write().insert(_vaddr, range);
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
            .filesystem
            .read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
        {
            Some(fs) => fs,
            None => return,
        };
        let ext2_fs = match fs.as_any().downcast_ref::<Ext2FileSystem>() {
            Some(fs) => fs,
            None => return,
        };

        let backing_ptr = backing.as_ptr() as *const u8;
        let data = unsafe { core::slice::from_raw_parts(backing_ptr, backing_len) };
        let _ = ext2_fs.write_file_content(self.inode_number, data);
        PageCacheManager::global().invalidate(self.cache_id());
    }

    fn supports_mmap(&self) -> bool {
        true
    }

    fn resolve_fault(
        &self,
        access: &crate::object::capability::memory_mapping::AccessKind,
        _page_idx: usize,
        vm_start: usize,
    ) -> core::result::Result<
        crate::object::capability::memory_mapping::ResolveFaultResult,
        crate::object::capability::memory_mapping::ResolveFaultError,
    > {
        let range = match self.mmap_ranges.read().get(&vm_start).copied() {
            Some(r) => r,
            None => {
                crate::early_println!(
                    "[ext2] resolve_fault: no mmap_range for vm_start={:#x} vaddr={:#x}",
                    vm_start,
                    access.vaddr
                );
                return Err(crate::object::capability::memory_mapping::ResolveFaultError::Invalid);
            }
        };
        if access.vaddr < range.vaddr_start || access.vaddr > range.vaddr_end {
            crate::early_println!(
                "[ext2] resolve_fault: vaddr={:#x} outside range={:#x}-{:#x}",
                access.vaddr,
                range.vaddr_start,
                range.vaddr_end
            );
            return Err(crate::object::capability::memory_mapping::ResolveFaultError::Invalid);
        }

        let fs = self
            .filesystem
            .read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(crate::object::capability::memory_mapping::ResolveFaultError::Invalid)?;
        let ext2_fs = fs
            .as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or(crate::object::capability::memory_mapping::ResolveFaultError::Invalid)?;
        let inode = ext2_fs
            .read_inode(self.inode_number)
            .map_err(|_| crate::object::capability::memory_mapping::ResolveFaultError::Invalid)?;
        let file_size = self.effective_size(inode.size as usize);

        let file_offset = range
            .offset
            .saturating_add(access.vaddr.saturating_sub(range.vaddr_start));

        let page_index = (file_offset / PAGE_SIZE) as u64;

        // BSS/zero-fill: pages beyond file content return zeroed pages (POSIX mmap behavior).
        let pinned = if file_size == 0 || file_offset >= file_size {
            PageCacheManager::global()
                .pin_or_load(self.cache_id(), page_index, |paddr| {
                    // SAFETY: paddr is a freshly-allocated page from the page cache.
                    unsafe {
                        core::ptr::write_bytes(phys_to_virt(paddr) as *mut u8, 0, PAGE_SIZE);
                    }
                    Ok(())
                })
                .map_err(|_| {
                    crate::object::capability::memory_mapping::ResolveFaultError::Invalid
                })?
        } else {
            let pinned = PageCacheManager::global()
                .pin_or_load(self.cache_id(), page_index, |paddr| {
                    ext2_fs
                        .read_page_content(self.inode_number, page_index, paddr)
                        .map_err(|_| "ext2: read_page_content failed")
                })
                .map_err(|_| {
                    crate::object::capability::memory_mapping::ResolveFaultError::Invalid
                })?;

            // Zero-fill the tail of the last page if the page extends beyond file_size.
            let page_start = (file_offset / PAGE_SIZE) * PAGE_SIZE;
            let page_end = page_start + PAGE_SIZE;
            if page_end > file_size {
                let zero_start = file_size - page_start;
                // SAFETY: paddr is a valid page-cache page; zero_start < PAGE_SIZE.
                unsafe {
                    core::ptr::write_bytes(
                        (phys_to_virt(pinned.paddr()) as *mut u8).add(zero_start),
                        0,
                        PAGE_SIZE - zero_start,
                    );
                }
            }
            pinned
        };

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

impl FileObject for Ext2FileObject {
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        // Get filesystem reference
        let fs = self
            .filesystem
            .read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(StreamError::Closed)?;

        // Downcast to Ext2FileSystem
        let ext2_fs = fs
            .as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or(StreamError::NotSupported)?;

        // Read inode metadata
        let inode = ext2_fs
            .read_inode(self.inode_number)
            .map_err(|_| StreamError::IoError)?;

        // Convert inode permissions to FilePermission
        let permissions = FilePermission {
            read: (inode.mode & 0o444) != 0,
            write: (inode.mode & 0o222) != 0,
            execute: (inode.mode & 0o111) != 0,
        };

        // Determine file type from inode mode
        let file_type = if (inode.mode & EXT2_S_IFMT) == EXT2_S_IFREG {
            FileType::RegularFile
        } else if (inode.mode & EXT2_S_IFMT) == EXT2_S_IFDIR {
            FileType::Directory
        } else {
            FileType::RegularFile // Default fallback
        };

        let inode_size = inode.size as usize;
        let size = PageCacheManager::global()
            .cached_object_size(self.cache_id())
            .unwrap_or_else(|| self.effective_size(inode_size));

        Ok(FileMetadata {
            file_type,
            size,
            permissions,
            created_time: inode.ctime as u64,
            modified_time: inode.mtime as u64,
            accessed_time: inode.atime as u64,
            file_id: self.file_id,
            link_count: inode.links_count as u32,
        })
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let cache_id = self.cache_id();
        let file_size = {
            let fs = self
                .filesystem
                .read()
                .as_ref()
                .and_then(|weak| weak.upgrade())
                .ok_or(StreamError::Closed)?;
            let ext2_fs = fs
                .as_any()
                .downcast_ref::<Ext2FileSystem>()
                .ok_or(StreamError::NotSupported)?;
            let inode_size = ext2_fs
                .read_inode(self.inode_number)
                .map_err(|_| StreamError::IoError)?
                .size as usize;
            PageCacheManager::global()
                .cached_object_size(cache_id)
                .unwrap_or(inode_size)
        };

        let off = usize::try_from(offset).map_err(|_| StreamError::InvalidArgument)?;
        if off >= file_size {
            return Ok(0);
        }

        let mut total_read = 0usize;
        while total_read < buffer.len() && off + total_read < file_size {
            let absolute = off + total_read;
            let page_index = (absolute / PAGE_SIZE) as PageIndex;
            let offset_in_page = absolute % PAGE_SIZE;

            let pinned = PageCacheManager::global()
                .pin_or_load(cache_id, page_index, |paddr| {
                    let fs = self
                        .filesystem
                        .read()
                        .as_ref()
                        .and_then(|weak| weak.upgrade())
                        .ok_or("filesystem gone")?;
                    let ext2_fs = fs
                        .as_any()
                        .downcast_ref::<Ext2FileSystem>()
                        .ok_or("bad fs type")?;
                    ext2_fs
                        .read_page_content(self.inode_number, page_index, paddr)
                        .map_err(|_| "Failed to load page")
                })
                .map_err(|_| StreamError::IoError)?;

            unsafe {
                let src = (phys_to_virt(pinned.paddr()) as *const u8).add(offset_in_page);
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
            let page_index = (absolute / PAGE_SIZE) as PageIndex;
            let page_off = absolute % PAGE_SIZE;
            let remain_in_page = PAGE_SIZE - page_off;
            let chunk = core::cmp::min(buffer.len() - written, remain_in_page);

            let pinned = PageCacheManager::global()
                .pin_or_load(cache_id, page_index, |paddr| {
                    let fs = self
                        .filesystem
                        .read()
                        .as_ref()
                        .and_then(|weak| weak.upgrade())
                        .ok_or("filesystem gone")?;
                    let ext2_fs = fs
                        .as_any()
                        .downcast_ref::<Ext2FileSystem>()
                        .ok_or("bad fs type")?;
                    ext2_fs
                        .read_page_content(self.inode_number, page_index, paddr)
                        .map_err(|_| "Failed to load page")
                })
                .map_err(|_| StreamError::IoError)?;

            unsafe {
                let dst = (phys_to_virt(pinned.paddr()) as *mut u8).add(page_off);
                let src = buffer.as_ptr().add(written);
                core::ptr::copy_nonoverlapping(src, dst, chunk);
            }

            pinned.mark_dirty();
            written += chunk;
        }

        let new_end = off + written;
        let mut size_override = self.size_override.lock();
        match *size_override {
            Some(cur) => {
                if new_end > cur {
                    *size_override = Some(new_end);
                }
            }
            None => {
                *size_override = Some(new_end);
            }
        }
        // `effective_size` reads `size_override` itself.  Do not call it
        // while the guard above is alive: `IrqSpinLock` is not re-entrant,
        // so that used to leave the CPU spinning forever with IRQs masked on
        // every non-empty write.
        drop(size_override);
        PageCacheManager::global().record_object_size(cache_id, self.effective_size(new_end));

        *self.dirty.lock() = true;

        Ok(written)
    }

    fn truncate(&self, size: u64) -> Result<(), StreamError> {
        let new_size = usize::try_from(size).map_err(|_| StreamError::InvalidArgument)?;
        let fs = self
            .filesystem
            .read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(StreamError::Closed)?;
        let ext2_fs = fs
            .as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or(StreamError::NotSupported)?;
        let inode_size = ext2_fs
            .read_inode(self.inode_number)
            .map_err(|_| StreamError::IoError)?
            .size as usize;
        let cur_size = self.effective_size(inode_size);
        if new_size == cur_size {
            return Ok(());
        }

        let mut buffer = Vec::with_capacity(new_size);
        buffer.resize(new_size, 0);
        let copy_len = core::cmp::min(cur_size, new_size);
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
                        ext2_fs
                            .read_page_content(self.inode_number, page_index, paddr)
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

        ext2_fs
            .write_file_content(self.inode_number, &buffer)
            .map_err(|_| StreamError::IoError)?;
        PageCacheManager::global().invalidate(self.cache_id());
        *self.size_override.lock() = None;
        *self.dirty.lock() = false;
        PageCacheManager::global().record_object_size(self.cache_id(), new_size);

        let mut position = self.position.lock();
        if *position > size {
            *position = size;
        }

        Ok(())
    }

    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut pos = self.position.lock();

        match whence {
            SeekFrom::Start(offset) => {
                *pos = offset;
                Ok(*pos)
            }
            SeekFrom::Current(offset) => {
                if offset >= 0 {
                    *pos += offset as u64;
                } else {
                    let abs_offset = (-offset) as u64;
                    if abs_offset > *pos {
                        *pos = 0;
                    } else {
                        *pos -= abs_offset;
                    }
                }
                Ok(*pos)
            }
            SeekFrom::End(offset) => {
                let file_size = self.metadata()?.size as u64;

                let new_pos = if offset >= 0 {
                    file_size.saturating_add(offset as u64)
                } else {
                    let abs_offset = (-offset) as u64;
                    if abs_offset > file_size {
                        0
                    } else {
                        file_size - abs_offset
                    }
                };

                *pos = new_pos;
                Ok(*pos)
            }
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn sync(&self) -> Result<(), StreamError> {
        self.sync_to_disk()
    }
}

impl PageCacheCapable for Ext2FileObject {
    fn cache_id(&self) -> crate::fs::vfs_v2::cache::CacheId {
        let fs = self
            .filesystem
            .read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .expect("Ext2FileObject: filesystem gone");
        let ext2_fs = fs
            .as_any()
            .downcast_ref::<Ext2FileSystem>()
            .expect("Ext2FileObject: invalid filesystem type");

        let fs_id = ext2_fs.fs_id().get();
        let cache_id = (fs_id << 32) | self.file_id;
        crate::fs::vfs_v2::cache::CacheId::new(cache_id)
    }
}

impl crate::object::capability::selectable::Selectable for Ext2FileObject {
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
        _min_wait_ticks: u64,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }

    fn is_nonblocking(&self) -> bool {
        true
    }
}

impl Drop for Ext2FileObject {
    fn drop(&mut self) {
        if let Err(e) = self.sync_to_disk() {
            crate::println!(
                "[ext2] Drop: sync_to_disk failed for inode {}: {:?}",
                self.inode_number,
                e
            );
        }
        #[cfg(test)]
        crate::println!(
            "[ext2] Drop: File object dropped for inode {}",
            self.inode_number
        );
    }
}

/// ext2 Directory Object
///
/// Handles directory operations for directories in the ext2 filesystem.
#[derive(Debug)]
pub struct Ext2DirectoryObject {
    /// Inode number of the directory
    inode_number: u32,
    /// File ID
    file_id: u64,
    /// Current position in directory listing
    position: IrqSpinLock<u64>,
    /// Weak reference to the filesystem
    filesystem: IrqRwSpinLock<Option<Weak<dyn FileSystemOperations>>>,
    /// Cached directory entries to avoid re-reading on every access
    cached_entries: IrqSpinLock<Option<Vec<crate::fs::DirectoryEntryInternal>>>,
    /// Cache generation (based on directory modification time) to detect stale cache
    cache_generation: IrqSpinLock<u32>,
}

impl Ext2DirectoryObject {
    /// Create a new ext2 directory object
    pub fn new(inode_number: u32, file_id: u64) -> Self {
        Self {
            inode_number,
            file_id,
            position: IrqSpinLock::new(0),
            filesystem: IrqRwSpinLock::new(None),
            cached_entries: IrqSpinLock::new(None),
            cache_generation: IrqSpinLock::new(0),
        }
    }

    /// Set the filesystem reference
    pub fn set_filesystem(&self, fs: Weak<dyn FileSystemOperations>) {
        *self.filesystem.write() = Some(fs);
    }

    /// Get cached directory entries or read them if not cached
    fn get_cached_entries(&self) -> Result<Vec<crate::fs::DirectoryEntryInternal>, StreamError> {
        let filesystem = self
            .filesystem
            .read()
            .as_ref()
            .and_then(|weak_fs| weak_fs.upgrade())
            .ok_or(StreamError::IoError)?;

        let ext2_fs = filesystem
            .as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or(StreamError::IoError)?;

        // Get current directory inode to check modification time
        let current_inode = match ext2_fs.read_inode(self.inode_number) {
            Ok(inode) => inode,
            Err(_) => return Err(StreamError::IoError),
        };

        let current_generation = current_inode.mtime;

        // Check if we have cached entries and if they're still valid
        {
            let cached = self.cached_entries.lock();
            let cache_gen = *self.cache_generation.lock();
            if let Some(ref entries) = *cached {
                if cache_gen == current_generation {
                    return Ok(entries.clone());
                }
            }
        }

        // Read directory entries
        let entries = match ext2_fs.read_directory_entries(&current_inode) {
            Ok(entries) => entries,
            Err(_) => return Err(StreamError::IoError),
        };

        // Convert to internal directory entries with detailed file type detection
        let mut all_entries = Vec::new();

        for entry in entries {
            if entry.entry.inode == 0 {
                continue; // Skip deleted entries
            }

            // Detailed file type detection based on ext2 file_type field
            let inode_num = entry.entry.inode; // Copy to avoid alignment issues
            let file_type = match entry.entry.file_type {
                1 => FileType::RegularFile, // EXT2_FT_REG_FILE
                2 => FileType::Directory,   // EXT2_FT_DIR
                3 => {
                    // EXT2_FT_CHRDEV - Character device
                    // For device files, we need device information
                    // Extract device ID from inode's block array
                    let device_id = match ext2_fs.read_inode(inode_num) {
                        Ok(inode) => {
                            // ext2 stores device ID in block[0] for special files
                            inode.block[0] as usize
                        }
                        Err(_) => 0,
                    };
                    FileType::CharDevice(DeviceFileInfo {
                        device_id,
                        device_type: crate::device::DeviceType::Char,
                    })
                }
                4 => {
                    // EXT2_FT_BLKDEV - Block device
                    // Extract device ID from inode's block array
                    let device_id = match ext2_fs.read_inode(inode_num) {
                        Ok(inode) => {
                            // ext2 stores device ID in block[0] for special files
                            inode.block[0] as usize
                        }
                        Err(_) => 0,
                    };
                    FileType::BlockDevice(DeviceFileInfo {
                        device_id,
                        device_type: crate::device::DeviceType::Block,
                    })
                }
                5 => FileType::Pipe, // EXT2_FT_FIFO
                6 => FileType::Socket(SocketFileInfo {
                    socket_id: crate::fs::UNBOUND_SOCKET_ID,
                }), // EXT2_FT_SOCK - Socket ID will be bound at runtime
                7 => {
                    // EXT2_FT_SYMLINK - Symbolic link
                    // Read the actual symlink target from the inode using the new method
                    let target = match ext2_fs.read_inode(inode_num) {
                        Ok(inode) => inode
                            .read_symlink_target(ext2_fs)
                            .unwrap_or_else(|_| format!("<symlink:{}>", inode_num)),
                        Err(_) => String::new(),
                    };
                    FileType::SymbolicLink(target)
                }
                _ => FileType::Unknown, // Unknown file type
            };

            all_entries.push(crate::fs::DirectoryEntryInternal {
                name: entry.name,
                file_type,
                size: 0,                   // Size not immediately available
                file_id: inode_num as u64, // Use copied inode number
                metadata: None,
            });
        }

        // Sort entries by file_id for consistent ordering
        all_entries.sort_by_key(|entry| entry.file_id);

        // Cache the entries with current generation
        {
            let mut cached = self.cached_entries.lock();
            let mut cache_gen = self.cache_generation.lock();
            *cached = Some(all_entries.clone());
            *cache_gen = current_generation;
        }

        Ok(all_entries)
    }
}

impl StreamOps for Ext2DirectoryObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        // Use cached entries to avoid re-reading directory on every call
        let all_entries = self.get_cached_entries()?;

        // position is the entry index
        let position = *self.position.lock() as usize;

        if position >= all_entries.len() {
            return Ok(0); // EOF
        }

        // Get current entry
        let internal_entry = &all_entries[position];

        // Convert to binary format
        let dir_entry = crate::fs::DirectoryEntry::from_internal(internal_entry);

        // Calculate actual entry size
        let entry_size = dir_entry.entry_size();

        // Check buffer size
        if buffer.len() < entry_size {
            return Err(StreamError::InvalidArgument); // Buffer too small
        }

        // Treat struct as byte array
        let entry_bytes =
            unsafe { core::slice::from_raw_parts(&dir_entry as *const _ as *const u8, entry_size) };

        // Copy to buffer
        buffer[..entry_size].copy_from_slice(entry_bytes);

        // Move to next entry
        *self.position.lock() += 1;

        Ok(entry_size)
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::IoError)
    }
}

impl ControlOps for Ext2DirectoryObject {}

impl MemoryMappingOps for Ext2DirectoryObject {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("Memory mapping not supported for directories")
    }
}

impl FileObject for Ext2DirectoryObject {
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        // Get filesystem reference
        let fs = self
            .filesystem
            .read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(StreamError::Closed)?;

        // Downcast to Ext2FileSystem
        let ext2_fs = fs
            .as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or(StreamError::NotSupported)?;

        // Read inode metadata
        let inode = ext2_fs
            .read_inode(self.inode_number)
            .map_err(|_| StreamError::IoError)?;

        // Convert inode permissions to FilePermission
        let permissions = FilePermission {
            read: (inode.mode & 0o444) != 0,
            write: (inode.mode & 0o222) != 0,
            execute: (inode.mode & 0o111) != 0,
        };

        Ok(FileMetadata {
            file_type: FileType::Directory,
            size: inode.size as usize,
            permissions,
            created_time: inode.ctime as u64,
            modified_time: inode.mtime as u64,
            accessed_time: inode.atime as u64,
            file_id: self.file_id,
            link_count: inode.links_count as u32,
        })
    }

    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut pos = self.position.lock();

        match whence {
            SeekFrom::Start(offset) => {
                *pos = offset;
                Ok(*pos)
            }
            _ => Err(StreamError::IoError),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl crate::object::capability::selectable::Selectable for Ext2DirectoryObject {
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
        _min_wait_ticks: u64,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }

    fn is_nonblocking(&self) -> bool {
        true
    }
}

/// ext2 Character Device File Object
///
/// Handles character device operations through ext2 device files.
#[derive(Debug)]
pub struct Ext2CharDeviceFileObject {
    /// Device file info
    device_info: DeviceFileInfo,
    /// File ID
    file_id: u64,
    /// Current position in the device (for seekable devices)
    position: IrqSpinLock<u64>,
    /// Weak reference to the filesystem
    filesystem: IrqRwSpinLock<Option<Weak<dyn FileSystemOperations>>>,
}

impl Ext2CharDeviceFileObject {
    /// Create a new ext2 character device file object
    pub fn new(device_info: DeviceFileInfo, file_id: u64) -> Self {
        Self {
            device_info,
            file_id,
            position: IrqSpinLock::new(0),
            filesystem: IrqRwSpinLock::new(None),
        }
    }

    /// Set the filesystem reference
    pub fn set_filesystem(&self, fs: Weak<dyn FileSystemOperations>) {
        *self.filesystem.write() = Some(fs);
    }
}

impl StreamOps for Ext2CharDeviceFileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        #[cfg(test)]
        crate::println!(
            "[ext2] CharDevice read: device_id={}",
            self.device_info.device_id
        );

        // Get the device from device manager
        let device = DeviceManager::get_manager()
            .get_device(self.device_info.device_id)
            .ok_or_else(|| {
                #[cfg(test)]
                crate::println!(
                    "[ext2] CharDevice: Device with ID {} not found in DeviceManager",
                    self.device_info.device_id
                );
                StreamError::NotSupported
            })?;

        #[cfg(test)]
        crate::println!(
            "[ext2] CharDevice: Found device with ID {}",
            self.device_info.device_id
        );

        // Try to cast to CharDevice
        if let Some(char_device) = device.as_char_device() {
            #[cfg(test)]
            crate::println!("[ext2] CharDevice: Successfully cast to CharDevice");
            // Use the fallible CharDevice read path so interrupted waits reach the ABI.
            char_device.try_read(buffer)
        } else {
            #[cfg(test)]
            crate::println!("[ext2] CharDevice: Device is not a CharDevice");
            Err(StreamError::NotSupported)
        }
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        #[cfg(test)]
        crate::println!(
            "[ext2] CharDevice write: device_id={}, buffer_len={}",
            self.device_info.device_id,
            buffer.len()
        );

        // Get the device from device manager
        let device = DeviceManager::get_manager()
            .get_device(self.device_info.device_id)
            .ok_or_else(|| {
                #[cfg(test)]
                crate::println!(
                    "[ext2] CharDevice: Device with ID {} not found in DeviceManager",
                    self.device_info.device_id
                );
                StreamError::NotSupported
            })?;

        #[cfg(test)]
        crate::println!(
            "[ext2] CharDevice: Found device with ID {}",
            self.device_info.device_id
        );

        // Try to cast to CharDevice
        if let Some(char_device) = device.as_char_device() {
            #[cfg(test)]
            crate::println!("[ext2] CharDevice: Successfully cast to CharDevice");
            // Use the CharDevice write method
            char_device.write(buffer).map_err(|_err| {
                #[cfg(test)]
                crate::println!("[ext2] CharDevice write error");
                StreamError::IoError
            })
        } else {
            #[cfg(test)]
            crate::println!("[ext2] CharDevice: Device is not a CharDevice");
            Err(StreamError::NotSupported)
        }
    }
}

impl ControlOps for Ext2CharDeviceFileObject {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        // Character devices can support control operations
        // For now, return not supported
        let _ = (command, arg);
        Err("Control operation not supported")
    }
}

impl MemoryMappingOps for Ext2CharDeviceFileObject {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        // Most character devices don't support memory mapping
        Err("Memory mapping not supported")
    }
}

impl FileObject for Ext2CharDeviceFileObject {
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(FileMetadata {
            file_type: FileType::CharDevice(self.device_info),
            size: 0, // Character devices don't have a meaningful size
            permissions: FilePermission {
                read: true,
                write: true,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: self.file_id,
            link_count: 1,
        })
    }

    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        // Get the device to check if it supports seeking
        let device = DeviceManager::get_manager()
            .get_device(self.device_info.device_id)
            .ok_or(StreamError::NotSupported)?;

        if let Some(char_device) = device.as_char_device() {
            if char_device.can_seek() {
                let mut pos = self.position.lock();
                match whence {
                    SeekFrom::Start(offset) => {
                        *pos = offset;
                        Ok(*pos)
                    }
                    SeekFrom::Current(offset) => {
                        if offset >= 0 {
                            *pos = (*pos).saturating_add(offset as u64);
                        } else {
                            *pos = (*pos).saturating_sub((-offset) as u64);
                        }
                        Ok(*pos)
                    }
                    SeekFrom::End(_) => {
                        // Most character devices don't have a meaningful end
                        Err(StreamError::NotSupported)
                    }
                }
            } else {
                Err(StreamError::NotSupported)
            }
        } else {
            Err(StreamError::NotSupported)
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Selectable for Ext2CharDeviceFileObject {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        // Delegate to underlying Device's Selectable implementation when available
        if let Some(device) = DeviceManager::get_manager().get_device(self.device_info.device_id) {
            return device.current_ready(interest);
        }
        // Fallback: conservative defaults (always ready, non-blocking)
        ReadySet {
            read: interest.read,
            write: interest.write,
            except: interest.except && false,
        }
    }

    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        trapframe: &mut crate::arch::Trapframe,
        timeout_ticks: Option<u64>,
        min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        if let Some(device) = DeviceManager::get_manager().get_device(self.device_info.device_id) {
            return device.wait_until_ready(interest, trapframe, timeout_ticks, min_wait_ticks);
        }
        // No device found: do not block
        SelectWaitOutcome::Ready
    }
}
