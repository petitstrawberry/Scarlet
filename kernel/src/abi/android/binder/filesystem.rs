//! Explicit Android View devices backed by the native RPC adapter.
//! The initial binderfs subset provides the three standard devices. Dynamic
//! device allocation and global statistics are not advertised or accepted.
use crate::{
    fs::{
        FileMetadata, FileObject, FilePermission, FileSystemDriver, FileSystemError,
        FileSystemErrorKind, FileSystemType, FileType, SeekFrom,
        vfs_v2::core::{DirectoryEntryInternal, FileSystemId, FileSystemOperations, VfsNode},
    },
    object::capability::{ControlOps, MemoryMappingOps, Selectable, StreamError, StreamOps},
};
use alloc::{
    boxed::Box,
    string::String,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::any::Any;

const NAMES: [&str; 3] = ["binder", "hwbinder", "vndbinder"];
const MAGIC: i64 = 0x6c6f6f70;
fn error(kind: FileSystemErrorKind) -> FileSystemError {
    FileSystemError::new(kind, "Android Binder filesystem")
}

struct Node {
    id: u64,
    device: Option<u8>,
    fs: Weak<BinderFs>,
}
impl VfsNode for Node {
    fn id(&self) -> u64 {
        self.id
    }
    fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>> {
        self.fs
            .upgrade()
            .map(|fs| Arc::downgrade(&(fs as Arc<dyn FileSystemOperations>)))
    }
    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        let fs = self
            .fs
            .upgrade()
            .ok_or_else(|| error(FileSystemErrorKind::IoError))?;
        let kind = match self.device {
            None => FileType::Directory,
            Some(device) if fs.aliases => {
                FileType::SymbolicLink(alloc::format!("/dev/binderfs/{}", NAMES[device as usize]))
            }
            Some(device) => FileType::CharDevice(crate::fs::DeviceFileInfo {
                device_id: device as usize,
                device_type: crate::device::DeviceType::Char,
            }),
        };
        Ok(FileMetadata {
            file_type: kind,
            size: 0,
            permissions: FilePermission {
                read: true,
                write: self.device.is_some() && !fs.aliases,
                execute: self.device.is_none(),
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: self.id,
            link_count: 1,
        })
    }
    fn read_link(&self) -> Result<String, FileSystemError> {
        let fs = self
            .fs
            .upgrade()
            .ok_or_else(|| error(FileSystemErrorKind::IoError))?;
        if !fs.aliases {
            return Err(error(FileSystemErrorKind::InvalidOperation));
        }
        let device = self
            .device
            .ok_or_else(|| error(FileSystemErrorKind::InvalidOperation))?;
        Ok(alloc::format!("/dev/binderfs/{}", NAMES[device as usize]))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct BinderFs {
    id: FileSystemId,
    aliases: bool,
    nodes: Vec<Arc<Node>>,
}
impl BinderFs {
    fn new(aliases: bool) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            id: FileSystemId::new(),
            aliases,
            nodes: (0..4)
                .map(|index| {
                    Arc::new(Node {
                        id: index + 1,
                        device: if index == 0 {
                            None
                        } else {
                            Some((index - 1) as u8)
                        },
                        fs: weak.clone(),
                    })
                })
                .collect(),
        })
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
}
impl FileSystemOperations for BinderFs {
    fn fs_id(&self) -> FileSystemId {
        self.id
    }
    fn supports_retained_node_identity(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        if self.aliases {
            "android-binder-aliases"
        } else {
            "android-binder"
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn root_node(&self) -> Arc<dyn VfsNode> {
        self.nodes[0].clone()
    }
    fn is_read_only(&self) -> bool {
        self.aliases
    }
    fn open_requires_namespace_lock(&self) -> bool {
        false
    }
    fn lookup(
        &self,
        parent: &Arc<dyn VfsNode>,
        name: &String,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        if self.node(parent)?.device.is_some() {
            return Err(error(FileSystemErrorKind::NotADirectory));
        }
        let index = NAMES
            .iter()
            .position(|candidate| *candidate == name.as_str())
            .ok_or_else(|| error(FileSystemErrorKind::NotFound))?;
        Ok(self.nodes[index + 1].clone())
    }
    fn readdir(
        &self,
        node: &Arc<dyn VfsNode>,
    ) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        if self.node(node)?.device.is_some() {
            return Err(error(FileSystemErrorKind::NotADirectory));
        }
        NAMES
            .iter()
            .enumerate()
            .map(|(index, name)| {
                Ok(DirectoryEntryInternal {
                    name: (*name).into(),
                    file_type: self.nodes[index + 1].metadata()?.file_type,
                    file_id: self.nodes[index + 1].id,
                })
            })
            .collect()
    }
    fn open(
        &self,
        node: &Arc<dyn VfsNode>,
        flags: u32,
    ) -> Result<Arc<dyn FileObject>, FileSystemError> {
        let node = self.node(node)?;
        let Some(device) = node.device else {
            if flags & 3 != 0 {
                return Err(error(FileSystemErrorKind::IsADirectory));
            }
            return Ok(Arc::new(Directory {
                metadata: node.metadata()?,
                entries: self.readdir(&self.root_node())?,
                cursor: crate::sync::IrqSpinLock::new(0),
            }));
        };
        if self.aliases {
            return Err(error(FileSystemErrorKind::InvalidOperation));
        }
        if flags & 3 != 2 {
            return Err(error(FileSystemErrorKind::PermissionDenied));
        }
        if flags & 0x10000 != 0 {
            return Err(error(FileSystemErrorKind::NotADirectory));
        }
        if flags & !(3 | 0x80000 | 0x800 | 0x20000) != 0 {
            return Err(error(FileSystemErrorKind::NotSupported));
        }
        let task =
            crate::task::mytask().ok_or_else(|| error(FileSystemErrorKind::PermissionDenied))?;
        super::new_file(&task, device)
            .map(|file| file as Arc<dyn FileObject>)
            .map_err(|_| error(FileSystemErrorKind::DeviceError))
    }
    fn create(
        &self,
        _: &Arc<dyn VfsNode>,
        _: &String,
        _: FileType,
        _: u32,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        Err(error(FileSystemErrorKind::NotSupported))
    }
    fn remove(&self, _: &Arc<dyn VfsNode>, _: &String) -> Result<(), FileSystemError> {
        Err(error(FileSystemErrorKind::NotSupported))
    }
}

struct Directory {
    metadata: FileMetadata,
    entries: Vec<DirectoryEntryInternal>,
    cursor: crate::sync::IrqSpinLock<usize>,
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
        let size = record.entry_size();
        if buffer.len() < size {
            return Err(StreamError::InvalidArgument);
        }
        // SAFETY: the initialized native directory record is copied within its size.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                (&record as *const crate::fs::DirectoryEntry).cast::<u8>(),
                size,
            )
        };
        buffer[..size].copy_from_slice(bytes);
        *cursor += 1;
        Ok(size)
    }
    fn write(&self, _: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::PermissionDenied)
    }
}
impl ControlOps for Directory {}
impl MemoryMappingOps for Directory {
    fn get_mapping_info(
        &self,
        _: usize,
        _: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("directory cannot be mapped")
    }
}
impl Selectable for Directory {
    fn wait_until_ready(
        &self,
        _: crate::object::capability::ReadyInterest,
        _: &mut crate::arch::Trapframe,
        _: Option<u64>,
        _: u64,
    ) -> crate::object::capability::SelectWaitOutcome {
        crate::object::capability::SelectWaitOutcome::Ready
    }
}
impl FileObject for Directory {
    fn seek(&self, position: SeekFrom) -> Result<u64, StreamError> {
        match position {
            SeekFrom::Start(0) => {
                *self.cursor.lock() = 0;
                Ok(0)
            }
            _ => Err(StreamError::NotSupported),
        }
    }
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(self.metadata.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
struct Driver;
impl FileSystemDriver for Driver {
    fn name(&self) -> &'static str {
        "android-binder"
    }
    fn filesystem_type(&self) -> FileSystemType {
        FileSystemType::Virtual
    }
    fn create(&self) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        self.create_from_option_string("")
    }
    fn create_from_option_string(
        &self,
        options: &str,
    ) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        let aliases = match options {
            "" => false,
            "aliases" => true,
            _ => return Err(error(FileSystemErrorKind::NotSupported)),
        };
        let task =
            crate::task::mytask().ok_or_else(|| error(FileSystemErrorKind::PermissionDenied))?;
        if !task
            .credentials
            .read()
            .permits(crate::shield::identity::MANAGE_MOUNTS)
        {
            return Err(error(FileSystemErrorKind::PermissionDenied));
        }
        Ok(BinderFs::new(aliases))
    }
}
fn register() {
    crate::fs::get_fs_driver_manager().register_driver(Box::new(Driver));
}
crate::driver_initcall!(register);
pub(crate) fn filesystem_type(fs: &dyn FileSystemOperations) -> Option<i64> {
    fs.as_any()
        .downcast_ref::<BinderFs>()
        .filter(|fs| !fs.aliases)
        .map(|_| MAGIC)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test_case]
    fn binderfs_nodes_are_distinct_stable_and_aliases_resolve_to_the_real_mount() {
        let fs = BinderFs::new(false);
        let other = BinderFs::new(false);
        let aliases = BinderFs::new(true);
        let root = fs.root_node();
        assert_eq!(fs.readdir(&root).unwrap().len(), 3);
        for (index, name) in NAMES.iter().enumerate() {
            let node = fs.lookup(&root, &(*name).into()).unwrap();
            assert_eq!(node.id(), index as u64 + 2);
            assert_eq!(fs.lookup(&root, &(*name).into()).unwrap().id(), node.id());
            assert!(matches!(
                node.metadata().unwrap().file_type,
                FileType::CharDevice(_)
            ));
            assert!(other.readdir(&node).is_err());
            let alias = aliases
                .lookup(&aliases.root_node(), &(*name).into())
                .unwrap();
            assert_eq!(
                alias.read_link().unwrap(),
                alloc::format!("/dev/binderfs/{name}")
            );
        }
        assert_eq!(filesystem_type(&*fs), Some(MAGIC));
        assert_eq!(filesystem_type(&*aliases), None);
        assert!(fs.lookup(&root, &"missing".into()).is_err());
    }
}
