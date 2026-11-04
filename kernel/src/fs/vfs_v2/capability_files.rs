//! Explicit publication of retained file capabilities in one View.
//! Names are immutable; opens share the source open description (including its
//! cursor). Each published open enforces its own read/write ceiling. Executing
//! and direct native mapping of these nodes are not supported. No global device registration.
use super::core::{
    DirectoryEntryInternal, FileSystemId, FileSystemOperations, VfsFileObject, VfsNode,
};
use crate::{
    arch::Trapframe,
    fs::{FileMetadata, FilePermission, FileSystemError, FileSystemErrorKind, FileType},
    object::capability::{
        ControlOps, FileObject, MemoryMappingInfo, MemoryMappingOps, ReadyInterest, ReadySet,
        SelectWaitOutcome, Selectable, StreamError, StreamOps,
    },
    sync::IrqSpinLock,
};
use alloc::{
    string::String,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::any::Any;
fn error(kind: FileSystemErrorKind) -> FileSystemError {
    FileSystemError::new(kind, "Published file capability")
}

pub struct Export {
    name: String,
    file: Arc<dyn FileObject>,
    access: u32,
    // Retain source storage, not its mount topology. A VfsFileObject retained
    // beneath its own source mount would make a strong reference cycle.
    _backing: Vec<Arc<dyn FileSystemOperations>>,
}
impl Export {
    pub fn new(
        name: String,
        mut file: Arc<dyn FileObject>,
        access: u32,
    ) -> Result<Self, FileSystemError> {
        if name.is_empty()
            || name.len() > 255
            || name == "."
            || name == ".."
            || name.bytes().any(|b| b == 0 || b == b'/')
            || access == 0
            || access & !3 != 0
        {
            return Err(error(FileSystemErrorKind::InvalidOperation));
        }
        let metadata = file
            .metadata()
            .map_err(|_| error(FileSystemErrorKind::IoError))?;
        if matches!(
            metadata.file_type,
            FileType::Directory | FileType::SymbolicLink(_)
        ) || access & 1 != 0 && !metadata.permissions.read
            || access & 2 != 0 && !metadata.permissions.write
        {
            return Err(error(FileSystemErrorKind::PermissionDenied));
        }
        let mut backing = Vec::new();
        while let Some(vfs) = file.as_any().downcast_ref::<VfsFileObject>() {
            let fs = vfs
                .get_vfs_entry()
                .node()
                .filesystem()
                .and_then(|fs| fs.upgrade())
                .ok_or_else(|| error(FileSystemErrorKind::IoError))?;
            backing.push(fs);
            file = vfs.inner().clone();
        }
        Ok(Self {
            name,
            file,
            access,
            _backing: backing,
        })
    }
}
struct Node {
    id: u64,
    fs: Weak<CapabilityFiles>,
}
pub struct CapabilityFiles {
    id: FileSystemId,
    exports: Vec<Export>,
    nodes: Vec<Arc<Node>>,
}
impl CapabilityFiles {
    pub fn new(exports: Vec<Export>) -> Result<Arc<Self>, FileSystemError> {
        if exports.is_empty() || exports.len() > 64 {
            return Err(error(FileSystemErrorKind::InvalidOperation));
        }
        for (i, export) in exports.iter().enumerate() {
            if exports[..i].iter().any(|other| other.name == export.name) {
                return Err(error(FileSystemErrorKind::AlreadyExists));
            }
        }
        Ok(Arc::new_cyclic(|weak| Self {
            id: FileSystemId::new(),
            nodes: (0..=exports.len())
                .map(|i| {
                    Arc::new(Node {
                        id: i as u64 + 1,
                        fs: weak.clone(),
                    })
                })
                .collect(),
            exports,
        }))
    }
    fn node<'a>(&self, node: &'a Arc<dyn VfsNode>) -> Result<&'a Node, FileSystemError> {
        let node = node
            .as_any()
            .downcast_ref::<Node>()
            .ok_or_else(|| error(FileSystemErrorKind::InvalidOperation))?;
        if !node
            .fs
            .upgrade()
            .is_some_and(|fs| core::ptr::eq(&*fs, self))
        {
            return Err(error(FileSystemErrorKind::InvalidOperation));
        }
        Ok(node)
    }
    fn metadata(&self, id: u64) -> Result<FileMetadata, FileSystemError> {
        if id == 1 {
            return Ok(FileMetadata {
                file_type: FileType::Directory,
                size: 0,
                permissions: FilePermission {
                    read: true,
                    write: false,
                    execute: true,
                },
                created_time: 0,
                modified_time: 0,
                accessed_time: 0,
                file_id: 1,
                link_count: 1,
            });
        }
        let export = &self.exports[id as usize - 2];
        let mut meta = export
            .file
            .metadata()
            .map_err(|_| error(FileSystemErrorKind::IoError))?;
        meta.file_id = id;
        meta.link_count = 1;
        meta.permissions = FilePermission {
            read: export.access & 1 != 0,
            write: export.access & 2 != 0,
            execute: false,
        };
        Ok(meta)
    }
}
impl VfsNode for Node {
    fn id(&self) -> u64 {
        self.id
    }
    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        self.fs
            .upgrade()
            .ok_or_else(|| error(FileSystemErrorKind::IoError))?
            .metadata(self.id)
    }
    fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>> {
        self.fs
            .upgrade()
            .map(|fs| Arc::downgrade(&(fs as Arc<dyn FileSystemOperations>)))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl FileSystemOperations for CapabilityFiles {
    fn fs_id(&self) -> FileSystemId {
        self.id
    }
    fn name(&self) -> &str {
        "capability-files"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn root_node(&self) -> Arc<dyn VfsNode> {
        self.nodes[0].clone()
    }
    fn supports_retained_node_identity(&self) -> bool {
        true
    }
    fn open_requires_namespace_lock(&self) -> bool {
        false
    }
    fn lookup(
        &self,
        parent: &Arc<dyn VfsNode>,
        name: &String,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        if self.node(parent)?.id != 1 {
            return Err(error(FileSystemErrorKind::NotADirectory));
        }
        let i = self
            .exports
            .iter()
            .position(|e| e.name == *name)
            .ok_or_else(|| error(FileSystemErrorKind::NotFound))?;
        Ok(self.nodes[i + 1].clone())
    }
    fn readdir(
        &self,
        node: &Arc<dyn VfsNode>,
    ) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        if self.node(node)?.id != 1 {
            return Err(error(FileSystemErrorKind::NotADirectory));
        }
        self.exports
            .iter()
            .enumerate()
            .map(|(i, e)| {
                Ok(DirectoryEntryInternal {
                    name: e.name.clone(),
                    file_type: self.metadata(i as u64 + 2)?.file_type,
                    file_id: i as u64 + 2,
                })
            })
            .collect()
    }
    fn open(
        &self,
        node: &Arc<dyn VfsNode>,
        flags: u32,
    ) -> Result<Arc<dyn FileObject>, FileSystemError> {
        let id = self.node(node)?.id;
        if flags & !(3 | 0x80000 | 0x800 | 0x10000 | 0x20000) != 0 {
            return Err(error(FileSystemErrorKind::NotSupported));
        }
        if id == 1 {
            if flags & 3 != 0 {
                return Err(error(FileSystemErrorKind::IsADirectory));
            }
            return Ok(Arc::new(Directory {
                metadata: self.metadata(1)?,
                entries: self.readdir(node)?,
                cursor: IrqSpinLock::new(0),
            }));
        }
        if flags & 0x10000 != 0 {
            return Err(error(FileSystemErrorKind::NotADirectory));
        }
        let access = match flags & 3 {
            0 => 1,
            1 => 2,
            2 => 3,
            _ => return Err(error(FileSystemErrorKind::InvalidOperation)),
        };
        let export = &self.exports[id as usize - 2];
        if access & !export.access != 0 {
            return Err(error(FileSystemErrorKind::PermissionDenied));
        }
        Ok(Arc::new(Published {
            inner: export.file.clone(),
            access,
            metadata: self.metadata(id)?,
            _backing: export._backing.clone(),
        }))
    }
    fn create(
        &self,
        _: &Arc<dyn VfsNode>,
        _: &String,
        _: FileType,
        _: u32,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        Err(error(FileSystemErrorKind::PermissionDenied))
    }
    fn remove(&self, _: &Arc<dyn VfsNode>, _: &String) -> Result<(), FileSystemError> {
        Err(error(FileSystemErrorKind::PermissionDenied))
    }
}
struct Published {
    inner: Arc<dyn FileObject>,
    access: u32,
    metadata: FileMetadata,
    _backing: Vec<Arc<dyn FileSystemOperations>>,
}
impl StreamOps for Published {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        if self.access & 1 == 0 {
            return Err(StreamError::PermissionDenied);
        }
        self.inner.read(buffer)
    }
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        if self.access & 2 == 0 {
            return Err(StreamError::PermissionDenied);
        }
        self.inner.write(buffer)
    }
}
impl ControlOps for Published {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        self.inner.control(command, arg)
    }
}
impl MemoryMappingOps for Published {
    fn get_mapping_info(&self, _: usize, _: usize) -> Result<MemoryMappingInfo, &'static str> {
        Err("Published streams cannot be mapped")
    }
}
impl Selectable for Published {
    fn is_hung_up(&self) -> bool {
        self.inner.is_hung_up()
    }
    fn current_ready(&self, mut interest: ReadyInterest) -> ReadySet {
        interest.read &= self.access & 1 != 0;
        interest.write &= self.access & 2 != 0;
        self.inner.current_ready(interest)
    }
    fn subscribe_readiness(
        &self,
        mut interest: ReadyInterest,
        waiter: &Arc<crate::sync::Waker>,
    ) -> Option<Vec<crate::sync::readiness::ReadinessRegistration>> {
        interest.read &= self.access & 1 != 0;
        interest.write &= self.access & 2 != 0;
        self.inner.subscribe_readiness(interest, waiter)
    }
    fn wait_until_ready(
        &self,
        mut interest: ReadyInterest,
        tf: &mut Trapframe,
        timeout: Option<u64>,
        min: u64,
    ) -> SelectWaitOutcome {
        interest.read &= self.access & 1 != 0;
        interest.write &= self.access & 2 != 0;
        self.inner.wait_until_ready(interest, tf, timeout, min)
    }
    fn set_nonblocking(&self, enabled: bool) {
        self.inner.set_nonblocking(enabled)
    }
    fn is_nonblocking(&self) -> bool {
        self.inner.is_nonblocking()
    }
}
impl FileObject for Published {
    fn allows_execution(&self) -> bool {
        false
    }
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        let mut meta = self.inner.metadata()?;
        meta.file_id = self.metadata.file_id;
        meta.link_count = 1;
        meta.permissions = self.metadata.permissions;
        Ok(meta)
    }
    fn seek(&self, position: crate::fs::SeekFrom) -> Result<u64, StreamError> {
        self.inner.seek(position)
    }
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, StreamError> {
        if self.access & 1 == 0 {
            return Err(StreamError::PermissionDenied);
        }
        self.inner.read_at(offset, buffer)
    }
    fn write_at(&self, offset: u64, buffer: &[u8]) -> Result<usize, StreamError> {
        if self.access & 2 == 0 {
            return Err(StreamError::PermissionDenied);
        }
        self.inner.write_at(offset, buffer)
    }
    fn seek_signed(&self, offset: i64, whence: u32) -> Result<u64, StreamError> {
        self.inner.seek_signed(offset, whence)
    }
    fn truncate(&self, size: u64) -> Result<(), StreamError> {
        if self.access & 2 == 0 {
            return Err(StreamError::PermissionDenied);
        }
        self.inner.truncate(size)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
struct Directory {
    metadata: FileMetadata,
    entries: Vec<DirectoryEntryInternal>,
    cursor: IrqSpinLock<usize>,
}
impl StreamOps for Directory {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let mut cursor = self.cursor.lock();
        let Some(entry) = self.entries.get(*cursor) else {
            return Ok(0);
        };
        let record = crate::fs::DirectoryEntry::from_internal(&crate::fs::DirectoryEntryInternal {
            name: entry.name.clone(),
            file_type: entry.file_type.clone(),
            size: 0,
            file_id: entry.file_id,
            metadata: None,
        });
        if buffer.len() < record.entry_size() {
            return Err(StreamError::InvalidArgument);
        }
        // Serialize fields explicitly; reserved bytes and the name tail are zero.
        let output = &mut buffer[..record.entry_size()];
        output.fill(0);
        output[..8].copy_from_slice(&record.file_id.to_ne_bytes());
        output[8..16].copy_from_slice(&record.size.to_ne_bytes());
        output[16] = record.file_type;
        output[17] = record.name_len;
        output[24..280].copy_from_slice(&record.name);
        *cursor += 1;
        Ok(record.entry_size())
    }
    fn write(&self, _: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::PermissionDenied)
    }
}
impl ControlOps for Directory {}
impl MemoryMappingOps for Directory {
    fn get_mapping_info(&self, _: usize, _: usize) -> Result<MemoryMappingInfo, &'static str> {
        Err("Directory cannot be mapped")
    }
}
impl Selectable for Directory {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        ReadySet {
            read: interest.read,
            write: false,
            except: false,
        }
    }
    fn subscribe_readiness(
        &self,
        _: ReadyInterest,
        _: &Arc<crate::sync::Waker>,
    ) -> Option<Vec<crate::sync::readiness::ReadinessRegistration>> {
        Some(Vec::new())
    }
    fn wait_until_ready(
        &self,
        _: ReadyInterest,
        _: &mut Trapframe,
        _: Option<u64>,
        _: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::Ready
    }
}
impl FileObject for Directory {
    fn allows_execution(&self) -> bool {
        false
    }
    fn seek(&self, position: crate::fs::SeekFrom) -> Result<u64, StreamError> {
        match position {
            crate::fs::SeekFrom::Start(0) => {
                *self.cursor.lock() = 0;
                Ok(0)
            }
            _ => Err(StreamError::SeekError),
        }
    }
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(self.metadata.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::input::virtual_device::InputEndpoint;
    use crate::fs::vfs_v2::manager::VfsManager;
    use crate::object::KernelObject;
    use alloc::vec;
    #[test_case]
    fn published_capabilities_are_view_scoped_and_cannot_gain_write_access() {
        let first = VfsManager::new();
        let second = VfsManager::new();
        first.create_dir("/input").unwrap();
        second.create_dir("/input").unwrap();
        let (send, receiver) = InputEndpoint::pair(1, 0, 0).unwrap();
        let source = Export::new("event0".into(), receiver, 1).unwrap();
        let files = CapabilityFiles::new(vec![source]).unwrap();
        first.mount(files, "/input", 10).unwrap();
        assert!(second.resolve_path("/input/event0").is_err());
        assert_eq!(
            first.open("/input/event0", 2).err().unwrap().kind,
            FileSystemErrorKind::PermissionDenied
        );
        let opened = first.open("/input/event0", 0).unwrap();
        let file = opened.as_file().unwrap();
        assert!(!file.allows_execution());
        assert!(matches!(
            file.write(&[0; 16]),
            Err(StreamError::PermissionDenied)
        ));
        let mut frame = [0; 32];
        frame[8..10].copy_from_slice(&1u16.to_ne_bytes());
        frame[10..12].copy_from_slice(&30u16.to_ne_bytes());
        frame[12..16].copy_from_slice(&1i32.to_ne_bytes());
        send.write(&frame).unwrap();
        let linux = crate::abi::linux::device::evdev::wrap(opened);
        let control = linux.as_control().unwrap();
        assert_eq!(
            control.control(crate::device::input::event_device::SCTL_INPUT_GET_KIND, 0),
            Ok(1)
        );
        // InputReader selects CLOCK_MONOTONIC; the isolated test machine need
        // not have a wall clock initialized.
        let task = crate::task::Task::new(
            "published-evdev-test".into(),
            0,
            crate::task::TaskType::User,
        );
        let address = crate::environment::USER_STACK_END - crate::environment::PAGE_SIZE;
        task.allocate_stack_pages(address, 1).unwrap();
        crate::library::std::usercopy::copy_to_user(&task, address, &1i32.to_ne_bytes()).unwrap();
        assert_eq!(
            crate::abi::linux::device::evdev::ioctl(&linux, &task, 0x400445a0, address),
            Some(0)
        );
        // A real native event crosses the published VFS node and evdev stream.
        let mut output = [0; 48];
        assert_eq!(linux.as_stream().unwrap().read(&mut output).unwrap(), 48);
        assert_eq!(i32::from_ne_bytes(output[20..24].try_into().unwrap()), 1);
    }
    #[test_case]
    fn published_storage_keeps_source_alive_without_retaining_its_mount_tree() {
        let vfs = VfsManager::new();
        vfs.create_dir("/exports").unwrap();
        let source = vfs.open_at(None, "/source", 0x42, 0o600).unwrap();
        source.as_file().unwrap().write(b"retained").unwrap();
        source
            .as_file()
            .unwrap()
            .seek(crate::fs::SeekFrom::Start(0))
            .unwrap();
        let KernelObject::File(source) = source else {
            panic!("file expected")
        };
        let export = Export::new("file".into(), source, 1).unwrap();
        let files = CapabilityFiles::new(vec![export]).unwrap();
        let weak_fs = Arc::downgrade(&files);
        vfs.mount(files, "/exports", 10).unwrap();
        let root = vfs.mount_tree.root_mount.read().clone();
        let weak_root = Arc::downgrade(&root);
        drop(root);
        let opened = vfs.open("/exports/file", 0).unwrap();
        let mut data = [0; 8];
        assert_eq!(opened.as_stream().unwrap().read(&mut data).unwrap(), 8);
        assert_eq!(&data, b"retained");
        drop(opened);
        drop(vfs);
        assert!(weak_root.upgrade().is_none());
        assert!(weak_fs.upgrade().is_none());
    }
    #[test_case]
    fn publication_rejects_duplicate_names_and_foreign_nodes() {
        let (_, one) = InputEndpoint::pair(1, 0, 0).unwrap();
        assert!(Export::new("../event0".into(), one.clone(), 1).is_err());
        assert!(Export::new("event0".into(), one.clone(), 2).is_err());
        assert!(
            CapabilityFiles::new(vec![
                Export::new("event0".into(), one.clone(), 1).unwrap(),
                Export::new("event0".into(), one.clone(), 1).unwrap()
            ])
            .is_err()
        );
        let a = CapabilityFiles::new(vec![Export::new("event0".into(), one.clone(), 1).unwrap()])
            .unwrap();
        let b = CapabilityFiles::new(vec![Export::new("event0".into(), one, 1).unwrap()]).unwrap();
        assert!(a.readdir(&b.root_node()).is_err());
        let directory = a.open(&a.root_node(), 0).unwrap();
        let mut bytes = [0; 280];
        assert_eq!(directory.read(&mut bytes).unwrap(), 280);
        assert_eq!(&bytes[18..24], &[0; 6]);
        assert_eq!(
            crate::fs::DirectoryEntry::parse(&bytes)
                .unwrap()
                .name_str()
                .unwrap(),
            "event0"
        );
    }
}
