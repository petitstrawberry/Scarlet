//! Discretionary permissions shared by native and compatibility ABIs.
use super::{authorization::Target, file::CreateContext};
use crate::fs::vfs_v2::{core::VfsNode, xattr};
use crate::{
    fs::{FileMetadata, FileSystemError, FileSystemErrorKind, FileType},
    shield::identity::{ActiveIdentity, Credentials, MANAGE_OWNERS, OVERRIDE_DAC},
    task::mytask,
};
use scarlet_abi::shield as abi;

pub use scarlet_abi::shield::{VFS_EXECUTE as EXECUTE, VFS_READ as READ, VFS_WRITE as WRITE};

#[derive(Clone, Copy, Debug)]
pub struct InodeAccess {
    pub owner: ActiveIdentity,
    pub mode: u32,
}

impl InodeAccess {
    pub fn for_creation(mode: u32) -> Self {
        let owner = mytask()
            .map(|task| task.credentials.read().identity())
            .unwrap_or(ActiveIdentity {
                principal: crate::shield::identity::Principal(0),
                group: crate::shield::identity::Group(0),
            });
        Self {
            owner,
            mode: mode & 0o7777,
        }
    }

    fn permissions(self, caller: &Credentials, directory: bool) -> u32 {
        if caller.permits(OVERRIDE_DAC) {
            return READ | WRITE | u32::from(directory || self.mode & 0o111 != 0);
        }
        let identity = caller.identity();
        let shift = if identity.principal == self.owner.principal {
            6
        } else if identity.group == self.owner.group
            || caller.supplementary().contains(&self.owner.group)
        {
            3
        } else {
            0
        };
        (self.mode >> shift) & 7
    }

    pub fn check_chmod(self) -> Result<(), FileSystemError> {
        if mytask().is_some_and(|task| {
            let credentials = task.credentials.read();
            credentials.identity().principal != self.owner.principal
                && !credentials.permits(MANAGE_OWNERS)
        }) {
            return Err(denied());
        }
        Ok(())
    }
}

fn denied() -> FileSystemError {
    FileSystemError::new(
        FileSystemErrorKind::PermissionDenied,
        "Inode permission denied",
    )
}

pub fn check_metadata(metadata: &FileMetadata, requested: u32) -> Result<(), FileSystemError> {
    if let Some(access) = metadata.permissions.inode()
        && let Some(task) = mytask()
        && access.permissions(
            &task.credentials.read(),
            metadata.file_type == FileType::Directory,
        ) & requested
            != requested
    {
        return Err(denied());
    }
    Ok(())
}

/// Parse the complete value even after a matching entry. Invalid ACLs never
/// produce a partial grant. Strict ordering also rules out duplicate entries.
pub(crate) fn acl_permissions(
    bytes: &[u8],
    caller: Option<&Credentials>,
) -> Result<u32, xattr::Error> {
    if bytes.is_empty() || bytes.len() % 8 != 0 || bytes.len() / 8 > abi::ACL_MAX_ENTRIES {
        return Err(xattr::Error::InvalidName);
    }
    let mut previous = None;
    let mut grant = 0;
    for record in bytes.chunks_exact(8) {
        let kind = u16::from_le_bytes([record[0], record[1]]);
        let bits = u16::from_le_bytes([record[2], record[3]]);
        let id = u32::from_le_bytes(record[4..8].try_into().unwrap());
        if !matches!(kind, abi::ACL_PRINCIPAL | abi::ACL_GROUP)
            || bits == 0
            || bits > 7
            || previous.is_some_and(|key| key >= (kind, id))
        {
            return Err(xattr::Error::InvalidName);
        }
        previous = Some((kind, id));
        if caller.is_some_and(|c| match kind {
            abi::ACL_PRINCIPAL => c.identity().principal.0 == id,
            _ => c.identity().group.0 == id || c.supplementary().iter().any(|group| group.0 == id),
        }) {
            grant |= u32::from(bits);
        }
    }
    Ok(grant)
}

pub(crate) fn check_domain(node: &dyn VfsNode, requested: u32) -> Result<(), FileSystemError> {
    if mytask().is_some_and(|task| !Target::Inode(node).permits(&task, requested.into())) {
        return Err(denied());
    }
    Ok(())
}

pub struct InodeSecurity {
    pub access: InodeAccess,
    pub directory: bool,
    pub acl: Option<alloc::vec::Vec<u8>>,
}

pub fn check(node: &dyn VfsNode, requested: u32) -> Result<(), FileSystemError> {
    let Some(task) = mytask() else { return Ok(()) };
    let available = if let Some(inode) = node.inode_security()? {
        let credentials = task.credentials.read();
        let base = inode.access.permissions(&credentials, inode.directory);
        let additional = inode
            .acl
            .as_deref()
            .map(|acl| acl_permissions(acl, Some(&credentials)))
            .transpose()
            .map_err(|_| denied())?
            .unwrap_or(0);
        base | additional
    } else {
        requested
    };
    let granted = Target::Inode(node).grant(&task, available.into());
    if granted & u64::from(requested) != u64::from(requested) {
        return Err(denied());
    }
    Ok(())
}

/// An open description carries a Shield decision for its lifetime. Duplication
/// preserves it; a fresh transfer can only intersect it with another ceiling.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct FileGrant(u32);

impl FileGrant {
    pub(crate) fn acquire(node: &dyn VfsNode, requested: u32) -> Result<Self, FileSystemError> {
        check(node, requested)?;
        // The executable loader alone consumes the private readable description.
        Ok(Self(if requested == EXECUTE {
            READ
        } else {
            requested
        }))
    }

    pub(crate) fn for_backing(flags: u32) -> Result<Self, FileSystemError> {
        // A driver may narrow an already-authorized open to a backing inode.
        Ok(Self(open_access(flags)?))
    }

    pub(crate) fn permits(self, requested: u32) -> bool {
        self.0 & requested == requested
    }

    pub(crate) fn from_mode(mode: crate::object::handle::AccessMode) -> Self {
        use crate::object::handle::AccessMode::*;
        Self(match mode {
            ReadOnly => READ,
            WriteOnly => WRITE,
            ReadWrite => READ | WRITE,
        })
    }

    pub(crate) fn intersect(self, ceiling: Self) -> Self {
        Self(self.0 & ceiling.0)
    }

    pub(crate) fn mapping_ceiling(self, shared: bool) -> usize {
        if !self.permits(READ) {
            return 0;
        }
        if shared && !self.permits(WRITE) {
            return !(crate::vm::vmem::VirtualMemoryPermission::Write as usize);
        }
        usize::MAX
    }
}

/// Resolve both the label to initialize and the rights to return before the
/// filesystem publishes the child, including O_CREAT with a restrictive mode.
pub(crate) fn authorize_creation(
    parent: &dyn VfsNode,
    kind: &FileType,
    requested: u32,
) -> Result<(Option<CreateContext>, FileGrant), FileSystemError> {
    check(parent, WRITE | EXECUTE)?;
    let Some(task) = mytask() else {
        return Ok((None, FileGrant(requested)));
    };
    let label = CreateContext::prepare(&task, parent, kind)
        .map_err(|reason| FileSystemError::new(FileSystemErrorKind::PermissionDenied, reason))?;
    if !Target::NewInode(label.as_ref().map(CreateContext::target)).permits(&task, requested.into())
    {
        return Err(denied());
    }
    Ok((label, FileGrant(requested)))
}
pub fn open_access(flags: u32) -> Result<u32, FileSystemError> {
    use scarlet_abi::fs::*;
    let access = match flags & VFS_O_ACCMODE {
        VFS_O_RDONLY => READ,
        VFS_O_WRONLY => WRITE,
        VFS_O_RDWR => READ | WRITE,
        _ => {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidOperation,
                "Invalid access mode",
            ));
        }
    };
    if flags & VFS_O_TRUNC != 0 && access & WRITE == 0 {
        return Err(FileSystemError::new(
            FileSystemErrorKind::InvalidOperation,
            "Truncate requires write access",
        ));
    }
    Ok(access)
}

pub fn check_removal(parent: &dyn VfsNode, target: &dyn VfsNode) -> Result<(), FileSystemError> {
    let metadata = parent.metadata()?;
    check(parent, WRITE | EXECUTE)?;
    if let Some(dir) = metadata.permissions.inode()
        && dir.mode & 0o1000 != 0
        && let Some(task) = mytask()
    {
        let owner = target.owner_identity();
        let caller = task.credentials.read();
        if !caller.permits(MANAGE_OWNERS)
            && caller.identity().principal != dir.owner.principal
            && owner.is_none_or(|id| id.principal != caller.identity().principal)
        {
            return Err(denied());
        }
    }
    Ok(())
}

/// Receiving a file is a new grant on the retained inode. The receiver's
/// pathname permissions are irrelevant. Dup/fork keep the resulting object.
pub(crate) fn receive_handle(
    task: &crate::task::Task,
    object: crate::object::KernelObject,
    mut metadata: crate::object::handle::HandleMetadata,
) -> Result<
    (
        crate::object::KernelObject,
        crate::object::handle::HandleMetadata,
    ),
    FileSystemError,
> {
    use crate::fs::vfs_v2::core::VfsFileObject;
    use crate::object::{KernelObject, handle::AccessMode};
    let Some(file) = object
        .as_file()
        .and_then(|f| f.as_any().downcast_ref::<VfsFileObject>())
    else {
        if !Target::ReceivedCapability.permits(task, abi::RECEIVE_CAPABILITY) {
            return Err(denied());
        }
        return Ok((object, metadata));
    };
    let sender = FileGrant::from_mode(metadata.access_mode).0 & file.grant().0;
    let node = file.get_vfs_entry().node();
    let rights = Target::ReceivedFile(node.as_ref()).grant(task, sender.into()) as u32;
    let mode = match rights {
        READ => AccessMode::ReadOnly,
        WRITE => AccessMode::WriteOnly,
        rights if rights == READ | WRITE => AccessMode::ReadWrite,
        _ => return Err(denied()),
    };
    metadata.access_mode = mode;
    if file.grant().0 == rights {
        Ok((object, metadata))
    } else {
        Ok((
            KernelObject::File(alloc::sync::Arc::new(file.with_grant(FileGrant(rights)))),
            metadata,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shield::authorization::{Management, permits_management};
    use crate::{
        fs::{FileType, VfsManager},
        object::handle::{AccessMode, HandleMetadata},
        shield::{Policy, Rule},
        task::{Task, TaskType, clear_mock_current_task, set_mock_current_task},
    };
    use alloc::{sync::Arc, vec};
    #[cfg(feature = "network")]
    #[test_case]
    fn file_domain_limits_open_creation_and_received_descriptions() {
        let mut task = Task::new("file-domain".into(), 0, TaskType::User);
        task.set_id(91003);
        let task = Arc::new(task);
        set_mock_current_task(task.clone());
        let fs = VfsManager::new();
        fs.create_file_with_mode("/image", FileType::RegularFile, 0o600)
            .unwrap();
        let original = fs.open("/image", 2).unwrap();
        original.as_file().unwrap().write_at(0, b"img").unwrap();
        let rule = |target, bits| Rule {
            source: 1,
            target,
            class: abi::VFS_ACCESS_CLASS,
            permissions: bits as u64,
        };
        let policy = Policy::new(
            vec![(1, "viewer".into()), (2, "dir".into()), (3, "image".into())],
            vec![rule(2, EXECUTE | WRITE), rule(3, abi::VFS_RECEIVE_READ)],
        )
        .unwrap()
        .with_file_rules(Some(3), vec![], vec![])
        .unwrap();
        policy
            .bind_resource(fs.resolve_path("/").unwrap().0.node(), 2)
            .unwrap();
        *task.security_subject.write() = Some(policy.subject(1).unwrap());
        assert!(fs.open("/image", 0).is_err());
        assert!(
            fs.open_at(None, "/new", scarlet_abi::fs::VFS_O_CREAT, 0o600)
                .is_err()
        );
        assert!(fs.metadata("/new").is_err());
        let (received, metadata) = {
            use crate::{
                abi::linux::generic::test_support, network::local::LocalSocket,
                object::KernelObject,
            };
            let (sender, receiver) =
                LocalSocket::create_connected_pair("broker".into(), "viewer".into());
            sender
                .send_handle(original.clone(), HandleMetadata::default())
                .unwrap();
            let socket = task
                .handle_table
                .insert(KernelObject::from_socket_object(receiver))
                .unwrap();
            let mut tf = test_support::syscall_frame(&task);
            tf.set_arg(0, socket as usize);
            let handle =
                u32::try_from(crate::ipc::syscall::sys_socket_recv_handle(&mut tf)).unwrap();
            (
                task.handle_table.get(handle).unwrap(),
                task.handle_table.get_metadata(handle).unwrap(),
            )
        };
        assert_eq!(metadata.access_mode, AccessMode::ReadOnly);
        let f = received.as_file().unwrap();
        assert_eq!(f.read(&mut [0; 1]).unwrap(), 1);
        assert!(f.write_at(0, b"x").is_err());
        assert!(f.truncate(0).is_err());
        assert_eq!(
            f.new_mapping_permissions(true)
                & crate::vm::vmem::VirtualMemoryPermission::Write as usize,
            0
        );
        let (_, forwarded) =
            receive_handle(&task, received.clone(), HandleMetadata::default()).unwrap();
        assert_eq!(forwarded.access_mode, AccessMode::ReadOnly);
        assert!(original.as_file().unwrap().write_at(0, b"y").is_ok());
        assert!(!permits_management(&task, Management::Modules));
        clear_mock_current_task();
    }
}
