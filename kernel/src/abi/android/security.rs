//! Android service authorization over native immutable policies. This exposes
//! the libselinux query protocol for service_manager; it does not load SELinux
//! binary policies or implement filesystem MAC, transitions, MLS or permissive.
use crate::{
    abi::linux::{
        extensions::FileMapping,
        generic::{LinuxAbi, errno, fs::FD_CLOEXEC},
    },
    fs::{
        FileMetadata, FilePermission, FileSystemDriver, FileSystemError, FileSystemErrorKind,
        FileSystemType, FileType, SeekFrom,
        vfs_v2::core::{
            DirectoryEntryInternal, FileSystemId, FileSystemOperations, VfsFileObject, VfsNode,
        },
    },
    object::{
        KernelObject,
        capability::{
            ControlOps, FileObject, MemoryMappingInfo, MemoryMappingOps, Selectable, StreamError,
            StreamOps,
        },
    },
    shield::Policy,
    sync::IrqSpinLock,
    task::Task,
};
use alloc::{
    boxed::Box,
    collections::BTreeMap,
    format,
    string::String,
    sync::{Arc, Weak},
    vec,
    vec::Vec,
};
use core::any::Any;

pub const SERVICE_MANAGER: u32 = 1;
pub const ADD: u64 = 1;
pub const FIND: u64 = 2;
pub const LIST: u64 = 4;
const SERVICE_PERMISSIONS: u32 = (ADD | FIND | LIST) as u32;
const SELINUX_MAGIC: i64 = 0xf97c_ff8c;
const PAGE: usize = crate::environment::PAGE_SIZE;

// Categories are retained as part of an exact, explicitly registered label.
// This does not advertise an MLS lattice or imply dominance between labels.
fn valid_context_name(name: &str) -> bool {
    let parts: Vec<_> = name.split(':').collect();
    if !(4..=5).contains(&parts.len())
        || parts[3] != "s0"
        || parts[..3]
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
    {
        return false;
    }
    if parts.len() == 5 {
        let mut previous = None;
        for category in parts[4].split(',') {
            let Some(number) = category.strip_prefix('c') else {
                return false;
            };
            if number.is_empty()
                || (number.len() > 1 && number.starts_with('0'))
                || !number.bytes().all(|b| b.is_ascii_digit())
            {
                return false;
            }
            let Ok(value) = number.parse::<u16>() else {
                return false;
            };
            if value > 1023 || previous.is_some_and(|p| p >= value) {
                return false;
            }
            previous = Some(value);
        }
    }
    true
}

fn fs_error(kind: FileSystemErrorKind) -> FileSystemError {
    FileSystemError::new(kind, "Android security filesystem")
}
fn metadata(id: u64, size: usize, directory: bool, writable: bool) -> FileMetadata {
    FileMetadata {
        file_type: if directory {
            FileType::Directory
        } else {
            FileType::RegularFile
        },
        size,
        permissions: FilePermission {
            read: true,
            write: writable,
            execute: directory,
        },
        created_time: 0,
        modified_time: 0,
        accessed_time: 0,
        file_id: id,
        link_count: 1,
    }
}
#[derive(Clone)]
enum Content {
    Directory,
    Text(Vec<u8>),
    Status,
    Access,
    Context,
    Current(Weak<Task>),
}
struct Node {
    id: u64,
    path: String,
    content: Content,
    fs: Weak<SecurityFs>,
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
        Ok(metadata(
            self.id,
            match &self.content {
                Content::Text(b) => b.len(),
                Content::Status => PAGE,
                _ => 0,
            },
            matches!(self.content, Content::Directory),
            matches!(self.content, Content::Access | Content::Context),
        ))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
pub(super) struct SecurityFs {
    id: FileSystemId,
    policy: Arc<Policy>,
    nodes: BTreeMap<String, Arc<Node>>,
    status: Arc<crate::ipc::SharedMemory>,
}
impl SecurityFs {
    fn new(policy: Arc<Policy>) -> Result<Arc<Self>, FileSystemError> {
        // Context tokens are an Android encoding constraint, not a native
        // policy rule. No policy may silently collapse two native identities.
        if policy.labels().any(|(_, name)| !valid_context_name(name)) {
            return Err(fs_error(FileSystemErrorKind::InvalidData));
        }
        let status = Arc::new(
            crate::ipc::SharedMemory::new(PAGE, 1)
                .map_err(|_| fs_error(FileSystemErrorKind::NoSpace))?,
        );
        let info = status
            .get_mapping_info(0, PAGE)
            .map_err(|_| fs_error(FileSystemErrorKind::NoSpace))?;
        let values = [1u32, 0, 1, 1, 1]; // version, sequence, enforcing, policyload, deny_unknown
        // SAFETY: fresh, zeroed page owned exclusively here before publication.
        unsafe {
            core::ptr::copy_nonoverlapping(
                values.as_ptr().cast::<u8>(),
                crate::vm::addr::phys_to_virt(info.paddr) as *mut u8,
                20,
            );
        }
        Ok(Arc::new_cyclic(|weak| {
            let entries = [
                ("", Content::Directory),
                ("status", Content::Status),
                ("enforce", Content::Text(b"1\n".to_vec())),
                ("deny_unknown", Content::Text(b"1\n".to_vec())),
                ("reject_unknown", Content::Text(b"1\n".to_vec())),
                ("mls", Content::Text(b"0\n".to_vec())),
                ("access", Content::Access),
                ("context", Content::Context),
                ("class", Content::Directory),
                ("class/service_manager", Content::Directory),
                (
                    "class/service_manager/index",
                    Content::Text(b"1\n".to_vec()),
                ),
                ("class/service_manager/perms", Content::Directory),
                (
                    "class/service_manager/perms/add",
                    Content::Text(b"1\n".to_vec()),
                ),
                (
                    "class/service_manager/perms/find",
                    Content::Text(b"2\n".to_vec()),
                ),
                (
                    "class/service_manager/perms/list",
                    Content::Text(b"3\n".to_vec()),
                ),
            ];
            let nodes = entries
                .into_iter()
                .enumerate()
                .map(|(i, (path, content))| {
                    (
                        path.into(),
                        Arc::new(Node {
                            id: i as u64 + 1,
                            path: path.into(),
                            content,
                            fs: weak.clone(),
                        }),
                    )
                })
                .collect();
            Self {
                id: FileSystemId::new(),
                policy,
                nodes,
                status,
            }
        }))
    }
    fn node<'a>(&self, node: &'a Arc<dyn VfsNode>) -> Result<&'a Node, FileSystemError> {
        let node = node
            .as_any()
            .downcast_ref::<Node>()
            .ok_or_else(|| fs_error(FileSystemErrorKind::InvalidOperation))?;
        if !node
            .fs
            .upgrade()
            .is_some_and(|fs| core::ptr::eq(&*fs, self))
        {
            return Err(fs_error(FileSystemErrorKind::InvalidOperation));
        }
        Ok(node)
    }
}
impl SecurityFs {
    fn open_for(
        &self,
        node: &Arc<dyn VfsNode>,
        flags: u32,
        subject: Option<&crate::shield::Subject>,
    ) -> Result<Arc<dyn FileObject>, FileSystemError> {
        let node = self.node(node)?;
        if !subject.is_some_and(|s| Arc::ptr_eq(s.policy(), &self.policy)) {
            return Err(fs_error(FileSystemErrorKind::PermissionDenied));
        }
        let writable = matches!(node.content, Content::Access | Content::Context);
        if flags & 3 == 3 || (flags & 3 != 0 && !writable) || flags & (0x200 | 0x400) != 0 {
            return Err(fs_error(FileSystemErrorKind::PermissionDenied));
        }
        let bytes = match &node.content {
            Content::Text(b) => b.clone(),
            Content::Status => {
                let mut b = vec![0; PAGE];
                for (i, v) in [1u32, 0, 1, 1, 1].iter().enumerate() {
                    b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
                }
                b
            }
            _ => Vec::new(),
        };
        Ok(Arc::new(SecurityFile {
            content: node.content.clone(),
            policy: self.policy.clone(),
            status: Some(self.status.clone()),
            entries: if matches!(node.content, Content::Directory) {
                self.readdir(&(self.nodes[&node.path].clone() as Arc<dyn VfsNode>))?
            } else {
                Vec::new()
            },
            metadata: node.metadata()?,
            readable: flags & 3 != 1,
            writable: flags & 3 != 0,
            state: IrqSpinLock::new(FileState {
                bytes,
                position: 0,
                written: false,
            }),
        }))
    }
}

impl FileSystemOperations for SecurityFs {
    fn fs_id(&self) -> FileSystemId {
        self.id
    }
    fn name(&self) -> &str {
        "android-selinux"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn root_node(&self) -> Arc<dyn VfsNode> {
        self.nodes[""].clone()
    }
    fn lookup(
        &self,
        parent: &Arc<dyn VfsNode>,
        name: &String,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        let parent = self.node(parent)?;
        if !matches!(parent.content, Content::Directory) {
            return Err(fs_error(FileSystemErrorKind::NotADirectory));
        }
        let path = if parent.path.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", parent.path, name)
        };
        self.nodes
            .get(&path)
            .map(|n| n.clone() as Arc<dyn VfsNode>)
            .ok_or_else(|| fs_error(FileSystemErrorKind::NotFound))
    }
    fn open(
        &self,
        node: &Arc<dyn VfsNode>,
        flags: u32,
    ) -> Result<Arc<dyn FileObject>, FileSystemError> {
        let subject = crate::task::mytask().and_then(|task| task.security_subject.read().clone());
        self.open_for(node, flags, subject.as_ref())
    }
    fn create(
        &self,
        _: &Arc<dyn VfsNode>,
        _: &String,
        _: FileType,
        _: u32,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        Err(fs_error(FileSystemErrorKind::PermissionDenied))
    }
    fn remove(&self, _: &Arc<dyn VfsNode>, _: &String) -> Result<(), FileSystemError> {
        Err(fs_error(FileSystemErrorKind::PermissionDenied))
    }
    fn readdir(
        &self,
        node: &Arc<dyn VfsNode>,
    ) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        let node = self.node(node)?;
        if !matches!(node.content, Content::Directory) {
            return Err(fs_error(FileSystemErrorKind::NotADirectory));
        }
        let prefix = if node.path.is_empty() {
            String::new()
        } else {
            format!("{}/", node.path)
        };
        Ok(self
            .nodes
            .values()
            .filter_map(|n| {
                let name = n.path.strip_prefix(&prefix)?;
                if name.is_empty() || name.contains('/') {
                    return None;
                }
                Some(DirectoryEntryInternal {
                    name: name.into(),
                    file_type: n.metadata().ok()?.file_type,
                    file_id: n.id,
                })
            })
            .collect())
    }
}
struct FileState {
    bytes: Vec<u8>,
    position: usize,
    written: bool,
}
struct SecurityFile {
    content: Content,
    policy: Arc<Policy>,
    status: Option<Arc<crate::ipc::SharedMemory>>,
    metadata: FileMetadata,
    entries: Vec<DirectoryEntryInternal>,
    readable: bool,
    writable: bool,
    state: IrqSpinLock<FileState>,
}
fn decision(policy: &Policy, input: &[u8]) -> Result<Vec<u8>, StreamError> {
    let text = core::str::from_utf8(input).map_err(|_| StreamError::InvalidArgument)?;
    let fields: Vec<_> = text.split_ascii_whitespace().collect();
    if fields.len() != 4 {
        return Err(StreamError::InvalidArgument);
    }
    let source = policy
        .label_id(fields[0])
        .ok_or(StreamError::InvalidArgument)?;
    let target = policy
        .label_id(fields[1])
        .ok_or(StreamError::InvalidArgument)?;
    let class = fields[2]
        .parse::<u32>()
        .map_err(|_| StreamError::InvalidArgument)?;
    let _requested =
        u32::from_str_radix(fields[3], 16).map_err(|_| StreamError::InvalidArgument)?;
    // Unknown classes/bits always deny. A decision carries all known grants,
    // matching the immutable AVC cache semantics of libselinux.
    let allowed = if class == SERVICE_MANAGER {
        policy.allowed(source, target, class) as u32 & SERVICE_PERMISSIONS
    } else {
        0
    };
    Ok(format!("{allowed:x} ffffffff 0 ffffffff 1 0").into_bytes())
}
impl StreamOps for SecurityFile {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        if !self.readable {
            return Err(StreamError::PermissionDenied);
        }
        let mut state = self.state.lock();
        if matches!(self.content, Content::Directory) {
            let Some(entry) = self.entries.get(state.position) else {
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
            let size = record.entry_size();
            if buffer.len() < size {
                return Err(StreamError::InvalidArgument);
            }
            // SAFETY: DirectoryEntry is the initialized native directory record.
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    (&record as *const crate::fs::DirectoryEntry).cast::<u8>(),
                    size,
                )
            };
            buffer[..size].copy_from_slice(bytes);
            state.position += 1;
            return Ok(size);
        }
        let n = buffer
            .len()
            .min(state.bytes.len().saturating_sub(state.position));
        if n != 0 {
            buffer[..n].copy_from_slice(&state.bytes[state.position..state.position + n]);
            state.position += n;
        }
        Ok(n)
    }
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        if !self.writable {
            return Err(StreamError::PermissionDenied);
        }
        if buffer.is_empty() || buffer.len() > 4095 {
            return Err(StreamError::InvalidArgument);
        }
        if let Content::Current(owner) = &self.content {
            let caller = crate::task::mytask().ok_or(StreamError::PermissionDenied)?;
            let owner = owner.upgrade().ok_or(StreamError::PermissionDenied)?;
            if !core::ptr::eq(&*caller, &*owner) {
                return Err(StreamError::PermissionDenied);
            }
            let text = core::str::from_utf8(buffer).map_err(|_| StreamError::InvalidArgument)?;
            let text = text.strip_suffix('\0').unwrap_or(text);
            let target = self
                .policy
                .label_id(text)
                .ok_or(StreamError::InvalidArgument)?;
            let mut state = self.state.lock();
            if state.written || state.position != 0 {
                return Err(StreamError::InvalidArgument);
            }
            crate::shield::syscall::transition(&caller, &self.policy, target)
                .map_err(|_| StreamError::PermissionDenied)?;
            state.written = true;
            return Ok(buffer.len());
        }
        let response = match self.content {
            Content::Access => decision(&self.policy, buffer)?,
            Content::Context => {
                let text = core::str::from_utf8(buffer)
                    .map_err(|_| StreamError::InvalidArgument)?
                    .trim_end_matches('\0');
                self.policy
                    .label_id(text)
                    .ok_or(StreamError::InvalidArgument)?;
                let mut response = text.as_bytes().to_vec();
                response.push(0);
                response
            }
            _ => return Err(StreamError::PermissionDenied),
        };
        let mut state = self.state.lock();
        if state.written {
            return Err(StreamError::InvalidArgument);
        }
        state.bytes = response;
        state.position = 0;
        state.written = true;
        Ok(buffer.len())
    }
}
impl ControlOps for SecurityFile {}
impl Selectable for SecurityFile {
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
impl MemoryMappingOps for SecurityFile {
    fn supports_mmap(&self) -> bool {
        matches!(self.content, Content::Status)
    }
    fn supports_private_mmap(&self) -> bool {
        false
    }
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        if !matches!(self.content, Content::Status) || offset != 0 || length != PAGE {
            return Err("invalid status mapping");
        }
        self.status
            .as_ref()
            .ok_or("no status backing")?
            .get_mapping_info(offset, length)
    }
}
impl FileObject for SecurityFile {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(self.metadata.clone())
    }
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut state = self.state.lock();
        let pos = match whence {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::Current(n) => state.position as i128 + n as i128,
            SeekFrom::End(n) => state.bytes.len() as i128 + n as i128,
        };
        state.position = usize::try_from(pos).map_err(|_| StreamError::InvalidArgument)?;
        Ok(state.position as u64)
    }
    fn truncate(&self, _: u64) -> Result<(), StreamError> {
        Err(StreamError::PermissionDenied)
    }
}

struct Driver;
impl FileSystemDriver for Driver {
    fn name(&self) -> &'static str {
        "android-selinux"
    }
    fn filesystem_type(&self) -> FileSystemType {
        FileSystemType::Virtual
    }
    fn create(&self) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        Err(fs_error(FileSystemErrorKind::PermissionDenied))
    }
    fn create_from_option_string(
        &self,
        options: &str,
    ) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        let handle = options
            .strip_prefix("policy=")
            .and_then(|n| n.parse::<usize>().ok())
            .ok_or_else(|| fs_error(FileSystemErrorKind::InvalidData))?;
        let task =
            crate::task::mytask().ok_or_else(|| fs_error(FileSystemErrorKind::PermissionDenied))?;
        let policy = crate::shield::syscall::policy_handle(&task, handle, true)
            .map_err(|_| fs_error(FileSystemErrorKind::PermissionDenied))?;
        Ok(SecurityFs::new(policy)?)
    }
}
fn register() {
    crate::fs::get_fs_driver_manager().register_driver(Box::new(Driver));
}
crate::driver_initcall!(register);

pub(super) fn filesystem_type(fs: &dyn FileSystemOperations) -> Option<i64> {
    fs.as_any().is::<SecurityFs>().then_some(SELINUX_MAGIC)
}
pub(super) fn mmap(object: &KernelObject, mapping: &FileMapping) -> Option<usize> {
    let file = object.as_file()?;
    let file = file
        .as_any()
        .downcast_ref::<VfsFileObject>()?
        .inner()
        .as_any()
        .downcast_ref::<SecurityFile>()?;
    if !file.supports_mmap()
        || mapping.offset != 0
        || mapping.length != PAGE
        || mapping.protection != 1
        || mapping.flags & 3 != 1
    {
        return Some(errno::to_result(errno::EACCES));
    }
    None // validated immutable shared page; common Linux mmap installs it
}

pub(super) fn open(abi: &mut LinuxAbi, task: &Task, path: &str, flags: i32) -> Option<usize> {
    let pid = path.strip_prefix("/proc/")?.strip_suffix("/attr/current")?;
    let result = (|| -> Result<usize, usize> {
        if flags & 3 == 3 || flags & !(0x80000 | 0x800 | 0x20000 | 3) != 0 {
            return Err(errno::EACCES);
        }
        let writable = flags & 3 != 0;
        let target = if matches!(pid, "self" | "thread-self") {
            None
        } else {
            let local = pid
                .strip_prefix("self/task/")
                .unwrap_or(pid)
                .parse::<usize>()
                .map_err(|_| errno::ENOENT)?;
            let global = task
                .get_namespace()
                .resolve_global_id(local)
                .ok_or(errno::ESRCH)?;
            Some(crate::sched::scheduler::get_task_by_id(global).ok_or(errno::ESRCH)?)
        };
        let caller = task
            .security_subject
            .read()
            .clone()
            .ok_or(errno::EOPNOTSUPP)?;
        let subject = target
            .as_deref()
            .unwrap_or(task)
            .security_subject
            .read()
            .clone()
            .ok_or(errno::EACCES)?;
        if !Arc::ptr_eq(caller.policy(), subject.policy()) {
            return Err(errno::EACCES);
        }
        let bytes = format!("{}\0", subject.name()).into_bytes();
        let content = if writable {
            if !core::ptr::eq(target.as_deref().unwrap_or(task), task) || !caller.can_transition() {
                return Err(errno::EACCES);
            }
            let current = crate::task::mytask().ok_or(errno::ESRCH)?;
            if !core::ptr::eq(&*current, task) {
                return Err(errno::EACCES);
            }
            let owner = crate::sched::scheduler::get_task_by_id(current.get_id()).ok_or(errno::ESRCH)?;
            if !core::ptr::eq(owner.as_ref(), task) { return Err(errno::ESRCH); }
            Content::Current(Arc::downgrade(&owner))
        } else {
            Content::Text(bytes.clone())
        };
        let file = Arc::new(SecurityFile {
            content,
            policy: subject.policy().clone(),
            status: None,
            entries: Vec::new(),
            metadata: metadata(1, bytes.len(), false, writable),
            readable: flags & 3 != 1,
            writable,
            state: IrqSpinLock::new(FileState {
                bytes,
                position: 0,
                written: false,
            }),
        });
        let handle = task
            .handle_table
            .insert(KernelObject::File(file))
            .map_err(|_| errno::EMFILE)?;
        let fd = match abi.allocate_fd(handle) {
            Ok(fd) => fd,
            Err(_) => {
                let _ = task.handle_table.remove(handle);
                return Err(errno::EMFILE);
            }
        };
        if flags & 0x80000 != 0 {
            let _ = abi.set_fd_flags(fd, FD_CLOEXEC);
        }
        Ok(fd)
    })();
    Some(result.unwrap_or_else(errno::to_result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shield::Rule;
    fn policy() -> Arc<Policy> {
        Policy::new(
            vec![
                (1, "u:r:caller:s0".into()),
                (2, "u:object_r:service:s0".into()),
                (3, "u:r:denied:s0".into()),
            ],
            vec![Rule {
                source: 1,
                target: 2,
                class: SERVICE_MANAGER,
                permissions: ADD | FIND,
            }],
        )
        .unwrap()
    }
    #[test_case]
    fn android_shield_access_decisions_deny_unknown_classes_labels_and_permissions() {
        let p = policy();
        assert_eq!(
            decision(&p, b"u:r:caller:s0 u:object_r:service:s0 1 7").unwrap(),
            b"3 ffffffff 0 ffffffff 1 0"
        );
        assert_eq!(
            decision(&p, b"u:r:denied:s0 u:object_r:service:s0 1 1").unwrap(),
            b"0 ffffffff 0 ffffffff 1 0"
        );
        assert_eq!(
            decision(&p, b"u:r:caller:s0 u:object_r:service:s0 9 1").unwrap(),
            b"0 ffffffff 0 ffffffff 1 0"
        );
        for input in [
            b"bad".as_slice(),
            b"u:r:unknown:s0 u:object_r:service:s0 1 1",
            b"u:r:caller:s0 u:object_r:service:s0 1 ffffffff0",
            b"u:r:caller:s0 u:object_r:service:s0 1 1 extra",
        ] {
            assert!(decision(&p, input).is_err());
        }
    }
    #[test_case]
    fn android_shield_fs_binds_a_policy_and_keeps_access_transactions_per_open() {
        let p = policy();
        let fs = SecurityFs::new(p.clone()).unwrap();
        let subject = p.subject(1).unwrap();
        let access = fs.lookup(&fs.root_node(), &"access".into()).unwrap();
        assert!(fs.open_for(&access, 2, None).is_err());
        assert!(
            fs.open_for(&access, 2, Some(&policy().subject(1).unwrap()))
                .is_err()
        );
        let allowed = fs.open_for(&access, 2, Some(&subject)).unwrap();
        let denied = fs.open_for(&access, 2, Some(&subject)).unwrap();
        let query = b"u:r:caller:s0 u:object_r:service:s0 1 1";
        assert_eq!(allowed.write(query).unwrap(), query.len());
        assert!(allowed.write(query).is_err());
        denied
            .write(b"u:r:denied:s0 u:object_r:service:s0 1 1")
            .unwrap();
        let mut result = [0; 128];
        let n = allowed.read(&mut result).unwrap();
        assert_eq!(&result[..n], b"3 ffffffff 0 ffffffff 1 0");
        let n = denied.read(&mut result).unwrap();
        assert_eq!(&result[..n], b"0 ffffffff 0 ffffffff 1 0");
        assert_eq!(allowed.read(&mut result).unwrap(), 0);
        assert!(fs.lookup(&fs.root_node(), &"load".into()).is_err());
        let enforce = fs.lookup(&fs.root_node(), &"enforce".into()).unwrap();
        assert!(fs.open_for(&enforce, 1, Some(&subject)).is_err());
        assert!(
            fs.open_for(&enforce, 0, Some(&subject))
                .unwrap()
                .write(b"0")
                .is_err()
        );
    }
    #[test_case]
    fn android_shield_permissions_are_readable_directory_records_and_status_is_immutable() {
        let p = policy();
        let fs = SecurityFs::new(p.clone()).unwrap();
        let subject = p.subject(1).unwrap();
        let dir = fs.nodes["class/service_manager/perms"].clone() as Arc<dyn VfsNode>;
        let names: Vec<_> = fs
            .readdir(&dir)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(
            names,
            vec![
                String::from("add"),
                String::from("find"),
                String::from("list")
            ]
        );
        let file = fs.open_for(&dir, 0, Some(&subject)).unwrap();
        let mut bytes = vec![0; core::mem::size_of::<crate::fs::DirectoryEntry>()];
        assert!(file.read(&mut bytes[..1]).is_err());
        for _ in 0..3 {
            assert_eq!(file.read(&mut bytes).unwrap(), bytes.len());
        }
        assert_eq!(file.read(&mut bytes).unwrap(), 0);
        let status = fs.lookup(&fs.root_node(), &"status".into()).unwrap();
        let file = fs.open_for(&status, 0, Some(&subject)).unwrap();
        assert_eq!(file.get_mapping_info(0, PAGE).unwrap().permissions, 1);
        assert!(file.get_mapping_info(PAGE, PAGE).is_err());
        assert!(!file.supports_permission_changes());
        assert!(file.write(b"0").is_err());
        assert!(
            SecurityFs::new(Policy::new(vec![(1, "u:r:caller:s1:c1".into())], vec![]).unwrap())
                .is_err()
        );
    }
    #[test_case]
    fn android_context_categories_are_exact_canonical_labels_without_implicit_grants() {
        assert!(valid_context_name("u:r:app:s0:c1,c256,c512,c768"));
        for invalid in [
            "u:r:app:s0:",
            "u:r:app:s0:c1,c1",
            "u:r:app:s0:c2,c1",
            "u:r:app:s0:c01",
            "u:r:app:s0:c1024",
            "u:r:app:s0:c1.c9",
            "u:r:app:s0:c+1",
            "u:r:app:s1",
        ] {
            assert!(!valid_context_name(invalid));
        }
        let p = Policy::new(
            vec![
                (1, "u:r:app:s0:c1,c256,c512,c768".into()),
                (2, "u:r:app:s0:c2,c256,c512,c768".into()),
            ],
            vec![],
        )
        .unwrap();
        assert!(SecurityFs::new(p.clone()).is_ok());
        assert!(!p.subject(1).unwrap().permits(&p.subject(2).unwrap(), 1, 1));
    }
    #[test_case]
    fn android_setcon_checks_live_subject_uid_policy_owner_and_single_thread_at_write() {
        use crate::shield::{
            TransitionRule,
            identity::{Identities, Principal},
        };
        use crate::task::{
            CloneFlags, CloneFlagsDef, TaskType, clear_mock_current_task, set_mock_current_task,
        };
        let p = Policy::with_transitions(
            vec![
                (1, "u:r:zygote:s0".into()),
                (2, "u:r:app:s0:c1,c256,c512,c768".into()),
            ],
            vec![],
            vec![TransitionRule {
                source: 1,
                target: 2,
                principal: Principal(10001),
            }],
        )
        .unwrap();
        let task = Arc::new(Task::new("transition".into(), 1, TaskType::User));
        *task.security_subject.write() = Some(p.subject(1).unwrap());
        let mut abi = LinuxAbi::default();
        set_mock_current_task(task.clone());
        let file = SecurityFile {
            content: Content::Current(Arc::downgrade(&task)), policy:p.clone(), status:None,
            metadata:metadata(1,0,false,true), entries:Vec::new(), readable:false, writable:true,
            state:IrqSpinLock::new(FileState {bytes:Vec::new(),position:0,written:false}),
        };
        let text = b"u:r:app:s0:c1,c256,c512,c768\0";
        assert!(file.write(text).is_err()); // Open does not freeze credentials.
        let child = Arc::new(task.clone_task(CloneFlags::default()).unwrap());
        child
            .credentials
            .write()
            .set_principals(Identities::all(Principal(10001)))
            .unwrap();
        set_mock_current_task(child.clone());
        assert!(file.write(text).is_err()); // An inherited/transferred descriptor is not self.
        set_mock_current_task(task.clone());
        task.credentials
            .write()
            .set_principals(Identities::all(Principal(10001)))
            .unwrap();
        assert!(crate::shield::syscall::transition(&task, &policy(), 2).is_err());
        let mut flags = CloneFlags::default();
        flags.set(CloneFlagsDef::Vm);
        let sharing = task.clone_task(flags).unwrap();
        assert!(file.write(text).is_err());
        drop(sharing);
        assert_eq!(task.security_subject.read().as_ref().unwrap().label(), 1);
        assert_eq!(file.write(text).unwrap(), text.len());
        assert_eq!(task.security_subject.read().as_ref().unwrap().label(), 2);
        assert_eq!(child.security_subject.read().as_ref().unwrap().label(), 1);
        assert!(file.write(text).is_err());
        assert_eq!(
            open(&mut abi, &task, "/proc/self/attr/current", 1),
            Some(errno::to_result(errno::EACCES))
        );
        assert_eq!(
            task.new_exec_image()
                .security_subject
                .read()
                .as_ref()
                .unwrap()
                .label(),
            2
        );
        clear_mock_current_task();
    }
}
