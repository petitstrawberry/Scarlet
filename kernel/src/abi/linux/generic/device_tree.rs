//! View-filtered Linux device discovery. Storage and lifecycle remain native.
use crate::{
    device::{
        DeviceType,
        lifecycle::Endpoint,
        manager::DeviceManager,
    },
    fs::{
        FileMetadata, FilePermission, FileSystemDriver, FileSystemError, FileSystemErrorKind as K,
        FileSystemType, FileType, SeekFrom,
        vfs_v2::core::{DirectoryEntryInternal, FileSystemId, FileSystemOperations, VfsNode},
    },
    object::capability::{
        ControlOps, FileObject, MemoryMappingOps, ReadyInterest, SelectWaitOutcome, Selectable,
        StreamError, StreamOps,
    },
    sync::IrqSpinLock,
};
use alloc::{
    boxed::Box,
    format,
    string::String,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::any::Any;
fn error(kind: K) -> FileSystemError {
    FileSystemError::new(kind, "native endpoint discovery")
}
fn endpoints() -> Vec<Endpoint> {
    let Some(view) = crate::task::mytask().and_then(|task| task.get_vfs()) else {
        return Vec::new();
    };
    DeviceManager::get_manager()
        .endpoint_snapshot()
        .0
        .into_iter()
        .filter(|endpoint| {
            matches!(endpoint.kind, DeviceType::Char | DeviceType::Block)
                && super::socket::uevent::visible(&view, endpoint)
        })
        .collect()
}
struct Node {
    id: u64,
    kind: u8,
    endpoint: Option<Endpoint>,
    fs: Weak<DeviceTree>,
}
impl Node {
    fn live(&self) -> bool {
        self.endpoint
            .as_ref()
            .is_none_or(|expected| endpoints().iter().any(|endpoint| endpoint == expected))
    }
}
impl VfsNode for Node {
    fn shield_labels(&self)->Option<Arc<crate::shield::file::Labels>> {let fs=self.fs.upgrade()?;let mut labels=fs.labels.write();Some(labels.entry(self.id).or_insert_with(crate::shield::file::Labels::new).clone())}

    fn id(&self) -> u64 {
        self.id
    }
    fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>> {
        self.fs
            .upgrade()
            .map(|fs| Arc::downgrade(&(fs as Arc<dyn FileSystemOperations>)))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        if !self.live() {
            return Err(error(K::NotFound));
        }
        Ok(FileMetadata {
            file_type: if self.kind < 3 {
                FileType::Directory
            } else {
                FileType::RegularFile
            },
            size: 0,
            permissions: FilePermission {
                read: true,
                write: self.kind == 3,
                execute: self.kind < 3,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: self.id,
            link_count: 1,
        })
    }
}
struct DeviceTree {
    labels: crate::sync::IrqRwSpinLock<alloc::collections::BTreeMap<u64,Arc<crate::shield::file::Labels>>>,
    id: FileSystemId,
    root: Arc<Node>,
}
impl DeviceTree {
    fn new() -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            labels:crate::sync::IrqRwSpinLock::new(alloc::collections::BTreeMap::new()),
            id: FileSystemId::new(),
            root: Arc::new(Node {
                id: 1,
                kind: 0,
                endpoint: None,
                fs: weak.clone(),
            }),
        })
    }
    fn node(&self, kind: u8, endpoint: Option<Endpoint>) -> Arc<Node> {
        let id = endpoint.as_ref().map_or(1 + u64::from(kind), |endpoint| {
            10 + (endpoint.id as u64) * 4 + u64::from(kind)
        });
        Arc::new(Node {
            id,
            kind,
            endpoint,
            fs: self.root.fs.clone(),
        })
    }
    fn own<'a>(&self, node: &'a Arc<dyn VfsNode>) -> Result<&'a Node, FileSystemError> {
        let node = node
            .as_any()
            .downcast_ref::<Node>()
            .ok_or_else(|| error(K::InvalidOperation))?;
        if !node
            .fs
            .upgrade()
            .is_some_and(|fs| core::ptr::eq(fs.as_ref(), self))
            || !node.live()
        {
            return Err(error(K::NotFound));
        }
        Ok(node)
    }
    fn children(&self, node: &Node) -> Result<Vec<(String, Arc<Node>)>, FileSystemError> {
        Ok(match node.kind {
            0 => alloc::vec![("scarlet".into(), self.node(1, None))],
            1 => endpoints()
                .into_iter()
                .map(|endpoint| (endpoint.name.clone(), self.node(2, Some(endpoint))))
                .collect(),
            2 => alloc::vec![
                ("uevent".into(), self.node(3, node.endpoint.clone())),
                ("dev".into(), self.node(4, node.endpoint.clone()))
            ],
            _ => return Err(error(K::NotADirectory)),
        })
    }
}
impl FileSystemOperations for DeviceTree {
    fn fs_id(&self) -> FileSystemId {
        self.id
    }
    fn name(&self) -> &str {
        "linux-device-tree"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn root_node(&self) -> Arc<dyn VfsNode> {
        self.root.clone()
    }
    fn lookup(
        &self,
        parent: &Arc<dyn VfsNode>,
        name: &String,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        let node = self.own(parent)?;
        if name == "." {
            return Ok(self.node(node.kind, node.endpoint.clone()));
        }
        if name == ".." {
            return Ok(match node.kind {
                0 | 1 => self.node(0, None),
                2 => self.node(1, None),
                _ => return Err(error(K::NotADirectory)),
            });
        }
        self.children(node)?
            .into_iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, node)| node as Arc<dyn VfsNode>)
            .ok_or_else(|| error(K::NotFound))
    }
    fn create(
        &self,
        _: &Arc<dyn VfsNode>,
        _: &String,
        _: FileType,
        _: u32,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        Err(error(K::ReadOnly))
    }
    fn remove(&self, _: &Arc<dyn VfsNode>, _: &String) -> Result<(), FileSystemError> {
        Err(error(K::ReadOnly))
    }
    fn readdir(
        &self,
        node: &Arc<dyn VfsNode>,
    ) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        self.children(self.own(node)?)?
            .into_iter()
            .map(|(name, node)| {
                Ok(DirectoryEntryInternal {
                    name,
                    file_type: node.metadata()?.file_type,
                    file_id: node.id,
                })
            })
            .collect()
    }
    fn open(
        &self,
        node: &Arc<dyn VfsNode>,
        flags: u32,
    ) -> Result<Arc<dyn FileObject>, FileSystemError> {
        let node = self.own(node)?;
        if flags & 3 == 3 || flags & 3 != 0 && node.kind != 3 {
            return Err(error(K::PermissionDenied));
        }
        Ok(Arc::new(File {
            node: self.node(node.kind, node.endpoint.clone()),
            readable: flags & 3 != 1,
            writable: flags & 3 != 0,
            state: IrqSpinLock::new(State {
                position: 0,
                entries: None,
            }),
        }))
    }
}
struct State {
    position: usize,
    entries: Option<Vec<DirectoryEntryInternal>>,
}
struct File {
    node: Arc<Node>,
    readable: bool,
    writable: bool,
    state: IrqSpinLock<State>,
}
impl StreamOps for File {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        if !self.readable {
            return Err(StreamError::PermissionDenied);
        }
        if !self.node.live() {
            return Err(StreamError::InvalidArgument);
        }
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.node.kind < 3 {
            let fs = self.node.fs.upgrade().ok_or(StreamError::InvalidArgument)?;
            let mut state = self.state.lock();
            if state.entries.is_none() {
                state.entries = Some(
                    fs.readdir(&(self.node.clone() as Arc<dyn VfsNode>))
                        .map_err(|_| StreamError::InvalidArgument)?,
                );
            }
            let Some(entry) = state.entries.as_ref().unwrap().get(state.position) else {
                return Ok(0);
            };
            let record =
                crate::fs::DirectoryEntry::from_internal(&crate::fs::DirectoryEntryInternal {
                    name: entry.name.clone(),
                    file_type: entry.file_type.clone(),
                    size: 0,
                    file_id: entry.file_id,
                    metadata: None,
                });
            let length = record.entry_size();
            if buffer.len() < length {
                return Err(StreamError::InvalidArgument);
            }
            buffer[..length].copy_from_slice(unsafe {
                core::slice::from_raw_parts(
                    (&record as *const crate::fs::DirectoryEntry).cast::<u8>(),
                    length,
                )
            });
            state.position += 1;
            return Ok(length);
        }
        let endpoint = self
            .node
            .endpoint
            .as_ref()
            .ok_or(StreamError::InvalidArgument)?;
        let id = endpoint.id as u64;
        let major = ((id >> 8) & 0xfff) | ((id >> 32) & 0xfffff000);
        let minor = (id & 0xff) | ((id >> 12) & 0xffffff00);
        let text = if self.node.kind == 4 {
            format!("{major}:{minor}\n")
        } else {
            format!("MAJOR={major}\nMINOR={minor}\nDEVNAME={}\n", endpoint.name)
        };
        let mut state = self.state.lock();
        let start = state.position.min(text.len());
        let length = buffer.len().min(text.len() - start);
        buffer[..length].copy_from_slice(&text.as_bytes()[start..start + length]);
        state.position += length;
        Ok(length)
    }
    fn write(&self, bytes: &[u8]) -> Result<usize, StreamError> {
        if !self.writable || self.node.kind != 3 || !self.node.live() {
            return Err(StreamError::PermissionDenied);
        }
        let task = crate::task::mytask().ok_or(StreamError::PermissionDenied)?;
        if !task
            .credentials
            .read()
            .permits(crate::shield::identity::MANAGE_DEVICES)
        {
            return Err(StreamError::PermissionDenied);
        }
        if !matches!(bytes, b"add" | b"add\n") {
            return Err(StreamError::InvalidArgument);
        }
        DeviceManager::get_manager()
            .observe_endpoint(self.node.endpoint.as_ref().unwrap().id)
            .map_err(|_| StreamError::InvalidArgument)?;
        Ok(bytes.len())
    }
}
impl ControlOps for File {}
impl MemoryMappingOps for File {
    fn get_mapping_info(
        &self,
        _: usize,
        _: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("device discovery files cannot be mapped")
    }
}
impl Selectable for File {
    fn wait_until_ready(
        &self,
        _: ReadyInterest,
        _: &mut crate::arch::Trapframe,
        _: Option<u64>,
        _: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::Ready
    }
}
impl FileObject for File {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        self.node
            .metadata()
            .map_err(|_| StreamError::InvalidArgument)
    }
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut state = self.state.lock();
        let next = match whence {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::Current(n) => state.position as i128 + n as i128,
            _ => return Err(StreamError::NotSupported),
        };
        state.position = usize::try_from(next).map_err(|_| StreamError::InvalidArgument)?;
        if state.position == 0 {
            state.entries = None;
        }
        Ok(state.position as u64)
    }
}
struct Driver;
impl FileSystemDriver for Driver {
    fn name(&self) -> &'static str {
        "linux-device-tree"
    }
    fn filesystem_type(&self) -> FileSystemType {
        FileSystemType::Virtual
    }
    fn create(&self) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        Ok(DeviceTree::new())
    }
    fn create_from_option_string(
        &self,
        options: &str,
    ) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        if !options.is_empty() {
            return Err(error(K::InvalidData));
        }
        self.create()
    }
}
fn register() {
    crate::fs::get_fs_driver_manager().register_driver(Box::new(Driver));
}
crate::driver_initcall!(register);
