//! cgroup2 filesystem translation over native ResourceGroup membership.
//! No resource controller is advertised until its native enforcement exists.
use super::errno;
use crate::{
    fs::{
        FileMetadata, FilePermission, FileSystemDriver, FileSystemError, FileSystemErrorKind as K,
        FileSystemType, FileType, SeekFrom,
        vfs_v2::core::{DirectoryEntryInternal, FileSystemId, FileSystemOperations, VfsNode},
    },
    object::capability::{
        ControlOps, FileObject, MemoryMappingOps, Selectable, StreamError, StreamOps,
    },
    shield::identity::{ActiveIdentity, Group, MANAGE_RESOURCES, Principal},
    sync::{IrqRwSpinLock, IrqSpinLock, sequence::IdSequence},
    task::{Task, TaskState, TaskType, resource_group::ResourceGroup},
};
use alloc::{
    boxed::Box,
    collections::BTreeMap,
    format,
    string::{String, ToString},
    sync::{Arc, Weak},
    vec::Vec,
};
use core::any::Any;
const NAMES: [&str; 6] = [
    "cgroup.procs",
    "cgroup.controllers",
    "cgroup.subtree_control",
    "cgroup.events",
    "cgroup.stat",
    "cgroup.type",
];
fn error(kind: K) -> FileSystemError {
    FileSystemError::new(kind, "native resource-group view")
}
fn owner() -> ActiveIdentity {
    crate::task::mytask()
        .map(|task| task.credentials.read().identity())
        .unwrap_or(ActiveIdentity {
            principal: Principal(0),
            group: Group(0),
        })
}
struct Node {
    labels: Arc<crate::shield::file::Labels>,
    id: u64,
    group: Arc<ResourceGroup>,
    kind: u8,
    owner: IrqRwSpinLock<ActiveIdentity>,
    fs: Weak<CgroupFs>,
}
impl Node {
    fn writable(&self, task: &Task) -> bool {
        task.credentials.read().permits(MANAGE_RESOURCES)
            || task.credentials.read().identity().principal == self.owner.read().principal
    }
}
impl VfsNode for Node {
    fn shield_labels(&self)->Option<Arc<crate::shield::file::Labels>> {Some(self.labels.clone())}

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
    fn owner_identity(&self) -> Option<ActiveIdentity> {
        Some(*self.owner.read())
    }
    fn set_owner_identity(
        &self,
        principal: Option<Principal>,
        group: Option<Group>,
    ) -> Result<(), FileSystemError> {
        let task = crate::task::mytask().ok_or_else(|| error(K::PermissionDenied))?;
        if !task
            .credentials
            .read()
            .permits(crate::shield::identity::MANAGE_OWNERS)
        {
            return Err(error(K::PermissionDenied));
        }
        if !self.group.exists() {
            return Err(error(K::NotFound));
        }
        let mut current = self.owner.write();
        if let Some(principal) = principal {
            current.principal = principal;
        }
        if let Some(group) = group {
            current.group = group;
        }
        Ok(())
    }
    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        Ok(FileMetadata {
            file_type: if self.kind == 0 {
                FileType::Directory
            } else {
                FileType::RegularFile
            },
            size: 0,
            permissions: FilePermission {
                read: true,
                write: matches!(self.kind, 0 | 1 | 3),
                execute: self.kind == 0,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: self.id,
            link_count: 1,
        })
    }
}
pub(crate) struct CgroupFs {
    id: FileSystemId,
    root: Arc<Node>,
    nodes: IrqRwSpinLock<BTreeMap<(u64, u8), Arc<Node>>>,
}
impl CgroupFs {
    pub(crate) fn new(group: Arc<ResourceGroup>) -> Arc<Self> {
        Arc::new_cyclic(|weak| {
            let node = Arc::new(Node {
                    labels: crate::shield::file::Labels::new(),
                id: node_id(),
                group: group.clone(),
                kind: 0,
                owner: IrqRwSpinLock::new(owner()),
                fs: weak.clone(),
            });
            let mut nodes = BTreeMap::new();
            nodes.insert((group.identity(), 0), node.clone());
            Self {
                id: FileSystemId::new(),
                root: node,
                nodes: IrqRwSpinLock::new(nodes),
            }
        })
    }
    fn get(&self, group: Arc<ResourceGroup>, kind: u8, owner: ActiveIdentity) -> Arc<Node> {
        let key = (group.identity(), kind);
        let mut nodes = self.nodes.write();
        nodes
            .entry(key)
            .or_insert_with(|| {
                Arc::new(Node {
                    labels: crate::shield::file::Labels::new(),
                    id: node_id(),
                    group,
                    kind,
                    owner: IrqRwSpinLock::new(owner),
                    fs: self.root.fs.clone(),
                })
            })
            .clone()
    }
    fn node<'a>(&self, node: &'a Arc<dyn VfsNode>) -> Result<&'a Node, FileSystemError> {
        let node = node
            .as_any()
            .downcast_ref::<Node>()
            .ok_or_else(|| error(K::InvalidOperation))?;
        if !node
            .fs
            .upgrade()
            .is_some_and(|fs| core::ptr::eq(fs.as_ref(), self))
        {
            return Err(error(K::InvalidOperation));
        }
        Ok(node)
    }
}
fn node_id() -> u64 {
    static IDS: IdSequence = IdSequence::new();
    IDS.reserve().expect("cgroup node IDs exhausted").get()
}
impl FileSystemOperations for CgroupFs {
    fn fs_id(&self) -> FileSystemId {
        self.id
    }
    fn name(&self) -> &str {
        "cgroup2"
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
        let parent = self.node(parent)?;
        if parent.kind != 0 {
            return Err(error(K::NotADirectory));
        }
        if !parent.group.exists() {
            return Err(error(K::NotFound));
        }
        if name == "." {
            return Ok(self.get(parent.group.clone(), 0, *parent.owner.read()));
        }
        if name == ".." {
            let group = if parent.group.same_group(&self.root.group) {
                self.root.group.clone()
            } else {
                parent.group.parent().ok_or_else(|| error(K::NotFound))?
            };
            return Ok(self.get(group, 0, *parent.owner.read()));
        }
        if let Some(index) = NAMES.iter().position(|entry| *entry == name) {
            return Ok(self.get(parent.group.clone(), index as u8 + 1, *parent.owner.read()));
        }
        let group = parent.group.child(name).ok_or_else(|| error(K::NotFound))?;
        Ok(self.get(group, 0, *parent.owner.read()))
    }
    fn create(
        &self,
        parent: &Arc<dyn VfsNode>,
        name: &String,
        kind: FileType,
        _mode: u32,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        self.create_labeled(parent,name,kind,_mode,None)
    }
    fn create_labeled(&self,parent:&Arc<dyn VfsNode>,name:&String,kind:FileType,_mode:u32,label:Option<&crate::shield::file::CreateContext>)->Result<Arc<dyn VfsNode>,FileSystemError> {
        let node = self.node(parent)?;
        let task = crate::task::mytask().ok_or_else(|| error(K::PermissionDenied))?;
        if node.kind != 0 || !node.writable(&task) {
            return Err(error(K::PermissionDenied));
        }
        if kind != FileType::Directory || name.starts_with("cgroup.") {
            return Err(error(K::NotSupported));
        }
        if node.group.child(name).is_some() {
            return Err(error(K::FileExists));
        }
        let mut nodes=self.nodes.write();
        let group = node
            .group
            .create(name)
            .map_err(|_| error(K::InvalidOperation))?;
        let child=Arc::new(Node {labels:crate::shield::file::Labels::new(),id:node_id(),group:group.clone(),kind:0,owner:IrqRwSpinLock::new(owner()),fs:self.root.fs.clone()});
        if let Some(label)=label {if let Err(reason)=label.initialize(child.as_ref()) {let _=group.remove();return Err(FileSystemError::new(K::PermissionDenied,reason));}}
        nodes.insert((group.identity(),0),child.clone());Ok(child)
    }
    fn remove(&self, parent: &Arc<dyn VfsNode>, name: &String) -> Result<(), FileSystemError> {
        let node = self.node(parent)?;
        let task = crate::task::mytask().ok_or_else(|| error(K::PermissionDenied))?;
        if node.kind != 0 || !node.writable(&task) {
            return Err(error(K::PermissionDenied));
        }
        let group = node.group.child(name).ok_or_else(|| error(K::NotFound))?;
        group.remove().map_err(|_| error(K::Busy))?;
        let mut nodes = self.nodes.write();
        nodes.retain(|(id, _), node| {
            if *id == group.identity() {
                super::mode::clear(&(node.clone() as Arc<dyn VfsNode>));
                false
            } else {
                true
            }
        });
        Ok(())
    }
    fn readdir(
        &self,
        node: &Arc<dyn VfsNode>,
    ) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        let node = self.node(node)?;
        if node.kind != 0 {
            return Err(error(K::NotADirectory));
        }
        if !node.group.exists() {
            return Err(error(K::NotFound));
        }
        let mut entries = Vec::new();
        for (index, name) in NAMES.iter().enumerate() {
            let file = self.get(node.group.clone(), index as u8 + 1, *node.owner.read());
            entries.push(DirectoryEntryInternal {
                name: (*name).into(),
                file_type: FileType::RegularFile,
                file_id: file.id,
            });
        }
        for (name, group) in node.group.children() {
            let child = self.get(group, 0, *node.owner.read());
            entries.push(DirectoryEntryInternal {
                name,
                file_type: FileType::Directory,
                file_id: child.id,
            });
        }
        Ok(entries)
    }
    fn open(
        &self,
        node: &Arc<dyn VfsNode>,
        flags: u32,
    ) -> Result<Arc<dyn FileObject>, FileSystemError> {
        let node = self.node(node)?;
        if !node.group.exists() {
            return Err(error(K::NotFound));
        }
        let node = self.get(node.group.clone(), node.kind, *node.owner.read());
        if flags & 3 == 3 || (flags & 3 != 0 && (node.kind == 0 || !matches!(node.kind, 1 | 3))) {
            return Err(error(K::PermissionDenied));
        }
        if flags & 3 != 0 && !crate::task::mytask().is_some_and(|task| node.writable(&task)) {
            return Err(error(K::PermissionDenied));
        }
        Ok(Arc::new(File {
            node,
            readable: flags & 3 != 1,
            writable: flags & 3 != 0,
            state: IrqSpinLock::new(State {
                position: 0,
                bytes: None,
                entries: None,
            }),
        }))
    }
}
struct State {
    position: usize,
    bytes: Option<Vec<u8>>,
    entries: Option<Vec<DirectoryEntryInternal>>,
}
struct File {
    node: Arc<Node>,
    readable: bool,
    writable: bool,
    state: IrqSpinLock<State>,
}
impl File {
    fn content(&self) -> Result<Vec<u8>, StreamError> {
        if !self.node.group.exists() {
            return Err(StreamError::InvalidArgument);
        }
        let text = match self.node.kind {
            1 => {
                let task = crate::task::mytask().ok_or(StreamError::PermissionDenied)?;
                let namespace = task.get_namespace();
                let mut pids = alloc::collections::BTreeSet::new();
                for id in crate::sched::scheduler::get_task_pool().task_ids_snapshot() {
                    if let Some(peer) = crate::sched::scheduler::get_task_by_id(id) {
                        if peer.task_type == TaskType::User
                            && !matches!(
                                peer.get_state(),
                                TaskState::Zombie | TaskState::Terminated
                            )
                            && peer
                                .resource_membership
                                .group()
                                .same_group(&self.node.group)
                        {
                            if let Some(pid) =
                                namespace.resolve_local_id(peer.get_thread_group_id())
                            {
                                pids.insert(pid);
                            }
                        }
                    }
                }
                let mut text = String::new();
                for pid in pids {
                    text.push_str(&format!("{pid}\n"));
                }
                text
            }
            2 | 3 => "\n".into(),
            4 => format!(
                "populated {}\nfrozen 0\n",
                usize::from(self.node.group.process_count(true) != 0)
            ),
            5 => format!(
                "nr_descendants {}\nnr_dying_descendants 0\n",
                self.node.group.descendants()
            ),
            6 => "domain\n".into(),
            _ => return Err(StreamError::InvalidArgument),
        };
        Ok(text.into_bytes())
    }
}
impl StreamOps for File {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        if !self.readable {
            return Err(StreamError::PermissionDenied);
        }
        // Build snapshots before taking the IRQ-off per-description cursor lock.
        if self.node.kind == 0 {
            let fs = self.node.fs.upgrade().ok_or(StreamError::InvalidArgument)?;
            let entries = fs
                .readdir(&(self.node.clone() as Arc<dyn VfsNode>))
                .map_err(|_| StreamError::InvalidArgument)?;
            let mut state = self.state.lock();
            if state.entries.is_none() {
                state.entries = Some(entries);
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
            // SAFETY: the initialized native directory record contains length bytes.
            buffer[..length].copy_from_slice(unsafe {
                core::slice::from_raw_parts(
                    (&record as *const crate::fs::DirectoryEntry).cast::<u8>(),
                    length,
                )
            });
            state.position += 1;
            return Ok(length);
        }
        let bytes = self.content()?;
        let mut state = self.state.lock();
        if state.bytes.is_none() {
            state.bytes = Some(bytes);
        }
        let bytes = state.bytes.as_ref().unwrap();
        let start = state.position.min(bytes.len());
        let length = buffer.len().min(bytes.len() - start);
        buffer[..length].copy_from_slice(&bytes[start..start + length]);
        state.position += length;
        Ok(length)
    }
    fn write(&self, bytes: &[u8]) -> Result<usize, StreamError> {
        if !self.writable || !self.node.group.exists() {
            return Err(StreamError::PermissionDenied);
        }
        let task = crate::task::mytask().ok_or(StreamError::PermissionDenied)?;
        if !self.node.writable(&task) {
            return Err(StreamError::PermissionDenied);
        }
        if bytes.len() > 4096 {
            return Err(StreamError::InvalidArgument);
        }
        let text = core::str::from_utf8(bytes)
            .map_err(|_| StreamError::InvalidArgument)?
            .trim();
        if self.node.kind == 3 {
            return if text.is_empty() {
                Ok(bytes.len())
            } else {
                Err(StreamError::NotSupported)
            };
        }
        if self.node.kind != 1 {
            return Err(StreamError::PermissionDenied);
        }
        let pid = text
            .parse::<usize>()
            .map_err(|_| StreamError::InvalidArgument)?;
        let id = if pid == 0 {
            task.get_id()
        } else {
            task.get_namespace()
                .resolve_global_id(pid)
                .ok_or(StreamError::InvalidArgument)?
        };
        let target =
            crate::sched::scheduler::get_task_by_id(id).ok_or(StreamError::InvalidArgument)?;
        let fs = self.node.fs.upgrade().ok_or(StreamError::InvalidArgument)?;
        if target.task_type != TaskType::User
            || !fs.root.group.contains(&target.resource_membership.group())
            || !target.resource_membership.is_live()
        {
            return Err(StreamError::PermissionDenied);
        }
        target
            .resource_membership
            .migrate(self.node.group.clone())
            .map_err(|_| StreamError::InvalidArgument)?;
        Ok(bytes.len())
    }
}
impl ControlOps for File {}
impl Selectable for File {
    fn wait_until_ready(
        &self,
        _interest: crate::object::capability::ReadyInterest,
        _frame: &mut crate::arch::Trapframe,
        _timeout: Option<u64>,
        _flags: u64,
    ) -> crate::object::capability::SelectWaitOutcome {
        crate::object::capability::SelectWaitOutcome::Ready
    }
}
impl MemoryMappingOps for File {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("resource control files cannot be mapped")
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
        let position = match whence {
            SeekFrom::Start(value) => value as i128,
            SeekFrom::Current(value) => state.position as i128 + value as i128,
            SeekFrom::End(_) => return Err(StreamError::NotSupported),
        };
        state.position = usize::try_from(position).map_err(|_| StreamError::InvalidArgument)?;
        if state.position == 0 {
            state.bytes = None;
            state.entries = None;
        }
        Ok(state.position as u64)
    }
    fn truncate(&self, _length: u64) -> Result<(), StreamError> {
        Err(StreamError::PermissionDenied)
    }
}
struct Driver;
impl FileSystemDriver for Driver {
    fn name(&self) -> &'static str {
        "linux-cgroup2"
    }
    fn filesystem_type(&self) -> FileSystemType {
        FileSystemType::Virtual
    }
    fn create(&self) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        Err(error(K::PermissionDenied))
    }
    fn create_from_option_string(
        &self,
        options: &str,
    ) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        let task = crate::task::mytask().ok_or_else(|| error(K::PermissionDenied))?;
        crate::task::resource_group::syscall::authorized(&task)
            .map_err(|_| error(K::PermissionDenied))?;
        let handle = options
            .strip_prefix("group=")
            .and_then(|number| number.parse::<usize>().ok())
            .ok_or_else(|| error(K::InvalidData))?;
        let group = crate::task::resource_group::syscall::group(&task, handle)
            .map_err(|_| error(K::PermissionDenied))?;
        Ok(CgroupFs::new(group))
    }
}
fn register() {
    crate::fs::get_fs_driver_manager().register_driver(Box::new(Driver));
}
crate::driver_initcall!(register);

pub(crate) fn mount(task: &Task, path: &str, flags: usize, options: &str) -> Result<(), usize> {
    if flags & !14 != 0 || !options.is_empty() {
        return Err(errno::EOPNOTSUPP);
    }
    crate::task::resource_group::syscall::authorized(task).map_err(|_| errno::EPERM)?;
    let fs = task.get_vfs().ok_or(errno::EIO)?;
    let _composition = crate::executor::syscall::lock_composition();
    if crate::executor::syscall::is_root_path(&fs, path) {
        return Err(errno::EBUSY);
    }
    fs.mount(
        CgroupFs::new(task.resource_membership.group()),
        path,
        flags as u32,
    )
    .map_err(|error| errno::from_fs_error(&error))
}
