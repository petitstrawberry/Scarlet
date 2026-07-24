//! DevPTS - pseudo-terminal filesystem.
//!
//! DevPTS owns Unix98-style PTY allocation. Opening `ptmx` creates a fresh
//! master/slave pair, while active slave TTYs are exposed as numeric entries.

use crate::sync::{IrqRwSpinLock, IrqSpinLock};
use alloc::{
    boxed::Box,
    collections::BTreeMap,
    format,
    string::{String, ToString},
    sync::{Arc, Weak},
    vec::Vec,
};
use core::any::Any;

use crate::{
    device::{
        DeviceType,
        char::{
            CharDevice,
            pty::{PtyMasterDevice, PtyPair},
            tty::TtyDevice,
        },
    },
    driver_initcall,
    fs::{
        DeviceFileInfo, FileMetadata, FileObject, FilePermission, FileSystemDriver,
        FileSystemError, FileSystemErrorKind, FileSystemType, FileType, SeekFrom,
        get_fs_driver_manager,
    },
    object::capability::{
        ControlOps, MemoryMappingOps, StreamError, StreamOps,
        selectable::{ReadyInterest, ReadySet, SelectWaitOutcome, Selectable},
    },
};

use super::super::core::{DirectoryEntryInternal, FileSystemId, FileSystemOperations, VfsNode};

/// Scarlet-private control opcodes for PTY master endpoints.
pub mod pty_ctl {
    /// Return the PTY slave number for a master endpoint.
    pub const SCTL_PTY_GET_NUMBER: u32 = 0x5350_0001;
    /// Set the slave lock state for a master endpoint. `arg != 0` means locked.
    pub const SCTL_PTY_SET_LOCKED: u32 = 0x5350_0002;
    /// Return the slave lock state for a master endpoint.
    pub const SCTL_PTY_GET_LOCKED: u32 = 0x5350_0003;
}

const ROOT_ID: u64 = 1;
const PTMX_ID: u64 = 2;
const SLAVE_ID_BASE: u64 = 1024;

/// DevPTS filesystem instance.
pub struct DevPtsFS {
    fs_id: FileSystemId,
    root: IrqRwSpinLock<Arc<DevPtsNode>>,
    name: String,
    state: Arc<DevPtsState>,
}

impl DevPtsFS {
    /// Create a new DevPTS filesystem instance.
    ///
    /// # Returns
    ///
    /// A filesystem ready to mount below `/dev/pts`.
    pub fn new() -> Arc<Self> {
        let state = Arc::new(DevPtsState::new());
        let root = Arc::new(DevPtsNode::new_root(state.clone()));
        let fs = Arc::new(Self {
            fs_id: FileSystemId::new(),
            root: IrqRwSpinLock::new(root.clone()),
            name: "devpts".to_string(),
            state,
        });
        let fs_weak = Arc::downgrade(&(fs.clone() as Arc<dyn FileSystemOperations>));
        root.set_filesystem(fs_weak);
        fs
    }
}

impl FileSystemOperations for DevPtsFS {
    fn fs_id(&self) -> FileSystemId {
        self.fs_id
    }

    fn lookup(
        &self,
        parent: &Arc<dyn VfsNode>,
        name: &String,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        let devpts_node = Arc::downcast::<DevPtsNode>(parent.clone()).map_err(|_| {
            FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for DevPTS",
            )
        })?;

        if !matches!(devpts_node.kind, DevPtsNodeKind::Root) {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "DevPTS lookup is only supported at the root",
            ));
        }

        let fs_ref = devpts_node.filesystem();
        if name == "ptmx" {
            let node = Arc::new(DevPtsNode::new_ptmx(self.state.clone()));
            if let Some(fs_ref) = fs_ref {
                node.set_filesystem(fs_ref);
            }
            return Ok(node as Arc<dyn VfsNode>);
        }

        let number = parse_pty_number(name)?;
        if self.state.get_pair(number).is_none() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotFound,
                format!("PTY slave {} not found", number),
            ));
        }

        let node = Arc::new(DevPtsNode::new_slave(number, self.state.clone()));
        if let Some(fs_ref) = fs_ref {
            node.set_filesystem(fs_ref);
        }
        Ok(node as Arc<dyn VfsNode>)
    }

    fn open(
        &self,
        node: &Arc<dyn VfsNode>,
        _flags: u32,
    ) -> Result<Arc<dyn FileObject>, FileSystemError> {
        let devpts_node = Arc::downcast::<DevPtsNode>(node.clone()).map_err(|_| {
            FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for DevPTS",
            )
        })?;

        match devpts_node.kind {
            DevPtsNodeKind::Root => Ok(Arc::new(DevPtsDirectoryObject::new(devpts_node))),
            DevPtsNodeKind::Ptmx => {
                let pair = self.state.allocate_pair();
                let number = pair.number();
                Ok(Arc::new(DevPtsFileObject::new_master(
                    devpts_node,
                    pair,
                    self.state.clone(),
                    number,
                )))
            }
            DevPtsNodeKind::Slave(number) => {
                let pair = self.state.get_pair(number).ok_or_else(|| {
                    FileSystemError::new(
                        FileSystemErrorKind::NotFound,
                        format!("PTY slave {} not found", number),
                    )
                })?;
                if pair.is_slave_locked() {
                    return Err(FileSystemError::new(
                        FileSystemErrorKind::PermissionDenied,
                        "PTY slave is locked",
                    ));
                }
                if pair.slave().is_exclusive() {
                    return Err(FileSystemError::new(
                        FileSystemErrorKind::PermissionDenied,
                        "PTY slave is exclusive",
                    ));
                }
                Ok(Arc::new(DevPtsFileObject::new_slave(devpts_node, pair)))
            }
        }
    }

    fn create(
        &self,
        _parent_node: &Arc<dyn VfsNode>,
        _name: &String,
        _file_type: FileType,
        _mode: u32,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        Err(FileSystemError::new(
            FileSystemErrorKind::ReadOnly,
            "DevPTS is read-only: PTYs are created by opening ptmx",
        ))
    }

    fn remove(
        &self,
        _parent_node: &Arc<dyn VfsNode>,
        _name: &String,
    ) -> Result<(), FileSystemError> {
        Err(FileSystemError::new(
            FileSystemErrorKind::ReadOnly,
            "DevPTS is read-only: PTYs are removed when masters close",
        ))
    }

    fn readdir(
        &self,
        node: &Arc<dyn VfsNode>,
    ) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        let devpts_node = Arc::downcast::<DevPtsNode>(node.clone()).map_err(|_| {
            FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for DevPTS",
            )
        })?;
        devpts_node.readdir()
    }

    fn root_node(&self) -> Arc<dyn VfsNode> {
        self.root.read().clone() as Arc<dyn VfsNode>
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct DevPtsState {
    allocated: IrqSpinLock<Vec<bool>>,
    pairs: IrqRwSpinLock<BTreeMap<usize, Arc<PtyPair>>>,
}

impl DevPtsState {
    fn new() -> Self {
        Self {
            allocated: IrqSpinLock::new(Vec::new()),
            pairs: IrqRwSpinLock::new(BTreeMap::new()),
        }
    }

    fn allocate_pair(&self) -> Arc<PtyPair> {
        let number = {
            let mut allocated = self.allocated.lock();
            if let Some((index, slot)) = allocated
                .iter_mut()
                .enumerate()
                .find(|(_, allocated)| !**allocated)
            {
                *slot = true;
                index
            } else {
                let index = allocated.len();
                allocated.push(true);
                index
            }
        };

        let pair = Arc::new(PtyPair::new(number));
        self.pairs.write().insert(number, pair.clone());
        pair
    }

    fn release_pair(&self, number: usize) {
        self.pairs.write().remove(&number);
        if let Some(slot) = self.allocated.lock().get_mut(number) {
            *slot = false;
        }
    }

    fn get_pair(&self, number: usize) -> Option<Arc<PtyPair>> {
        self.pairs.read().get(&number).cloned()
    }

    fn active_numbers(&self) -> Vec<usize> {
        self.pairs.read().keys().copied().collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DevPtsNodeKind {
    Root,
    Ptmx,
    Slave(usize),
}

/// Node in the DevPTS filesystem.
pub struct DevPtsNode {
    kind: DevPtsNodeKind,
    state: Arc<DevPtsState>,
    filesystem: IrqRwSpinLock<Option<Weak<dyn FileSystemOperations>>>,
}

impl DevPtsNode {
    fn new_root(state: Arc<DevPtsState>) -> Self {
        Self {
            kind: DevPtsNodeKind::Root,
            state,
            filesystem: IrqRwSpinLock::new(None),
        }
    }

    fn new_ptmx(state: Arc<DevPtsState>) -> Self {
        Self {
            kind: DevPtsNodeKind::Ptmx,
            state,
            filesystem: IrqRwSpinLock::new(None),
        }
    }

    fn new_slave(number: usize, state: Arc<DevPtsState>) -> Self {
        Self {
            kind: DevPtsNodeKind::Slave(number),
            state,
            filesystem: IrqRwSpinLock::new(None),
        }
    }

    /// Set filesystem reference for this node.
    ///
    /// # Arguments
    ///
    /// * `fs` - Weak reference to the owning filesystem.
    pub fn set_filesystem(&self, fs: Weak<dyn FileSystemOperations>) {
        *self.filesystem.write() = Some(fs);
    }

    fn file_type_for(kind: DevPtsNodeKind) -> FileType {
        match kind {
            DevPtsNodeKind::Root => FileType::Directory,
            DevPtsNodeKind::Ptmx | DevPtsNodeKind::Slave(_) => {
                FileType::CharDevice(DeviceFileInfo {
                    device_id: 0,
                    device_type: DeviceType::Char,
                })
            }
        }
    }

    fn file_id_for(kind: DevPtsNodeKind) -> u64 {
        match kind {
            DevPtsNodeKind::Root => ROOT_ID,
            DevPtsNodeKind::Ptmx => PTMX_ID,
            DevPtsNodeKind::Slave(number) => SLAVE_ID_BASE + number as u64,
        }
    }

    fn readdir(&self) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        if !matches!(self.kind, DevPtsNodeKind::Root) {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Cannot read directory of non-directory DevPTS node",
            ));
        }

        let mut entries = Vec::new();
        entries.push(DirectoryEntryInternal {
            name: ".".to_string(),
            file_type: FileType::Directory,
            file_id: ROOT_ID,
        });
        entries.push(DirectoryEntryInternal {
            name: "..".to_string(),
            file_type: FileType::Directory,
            file_id: ROOT_ID,
        });
        entries.push(DirectoryEntryInternal {
            name: "ptmx".to_string(),
            file_type: Self::file_type_for(DevPtsNodeKind::Ptmx),
            file_id: PTMX_ID,
        });
        for number in self.state.active_numbers() {
            entries.push(DirectoryEntryInternal {
                name: number.to_string(),
                file_type: Self::file_type_for(DevPtsNodeKind::Slave(number)),
                file_id: Self::file_id_for(DevPtsNodeKind::Slave(number)),
            });
        }
        Ok(entries)
    }
}

impl VfsNode for DevPtsNode {
    fn id(&self) -> u64 {
        Self::file_id_for(self.kind)
    }

    fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>> {
        self.filesystem.read().clone()
    }

    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        Ok(FileMetadata {
            file_type: Self::file_type_for(self.kind),
            size: 0,
            permissions: FilePermission {
                read: true,
                write: !matches!(self.kind, DevPtsNodeKind::Root),
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: self.id(),
            link_count: 1,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

enum DevPtsEndpoint {
    Master(Arc<PtyMasterDevice>),
    Slave(Arc<TtyDevice>),
}

/// File object for PTY master and slave endpoints in DevPTS.
pub struct DevPtsFileObject {
    node: Arc<DevPtsNode>,
    endpoint: DevPtsEndpoint,
    position: IrqRwSpinLock<u64>,
    pair: Arc<PtyPair>,
    release: Option<(Arc<DevPtsState>, usize)>,
}

impl DevPtsFileObject {
    fn new_master(
        node: Arc<DevPtsNode>,
        pair: Arc<PtyPair>,
        state: Arc<DevPtsState>,
        number: usize,
    ) -> Self {
        Self {
            node,
            endpoint: DevPtsEndpoint::Master(pair.master()),
            position: IrqRwSpinLock::new(0),
            pair,
            release: Some((state, number)),
        }
    }

    fn new_slave(node: Arc<DevPtsNode>, pair: Arc<PtyPair>) -> Self {
        pair.open_slave_endpoint();
        Self {
            node,
            endpoint: DevPtsEndpoint::Slave(pair.slave()),
            position: IrqRwSpinLock::new(0),
            pair,
            release: None,
        }
    }

    /// Return the PTY number when this object is a master endpoint.
    ///
    /// # Returns
    ///
    /// `Some(number)` for master endpoints, otherwise `None`.
    pub fn pty_number(&self) -> Option<usize> {
        match self.endpoint {
            DevPtsEndpoint::Master(_) => Some(self.pair.number()),
            DevPtsEndpoint::Slave(_) => None,
        }
    }

    /// Return whether this object's slave endpoint is locked.
    ///
    /// # Returns
    ///
    /// `Some(lock_state)` for master endpoints, otherwise `None`.
    pub fn pty_slave_locked(&self) -> Option<bool> {
        match self.endpoint {
            DevPtsEndpoint::Master(_) => Some(self.pair.is_slave_locked()),
            DevPtsEndpoint::Slave(_) => None,
        }
    }

    /// Set the connected slave endpoint lock state.
    ///
    /// # Arguments
    ///
    /// * `locked` - New slave lock state.
    ///
    /// # Returns
    ///
    /// `true` when this object is a master endpoint and the state was updated.
    pub fn set_pty_slave_locked(&self, locked: bool) -> bool {
        match self.endpoint {
            DevPtsEndpoint::Master(_) => {
                self.pair.set_slave_locked(locked);
                true
            }
            DevPtsEndpoint::Slave(_) => false,
        }
    }

    /// Return the slave TTY endpoint when this object wraps one.
    ///
    /// # Returns
    ///
    /// `Some(TtyDevice)` for slave endpoints, otherwise `None`.
    pub fn tty_device(&self) -> Option<Arc<TtyDevice>> {
        match &self.endpoint {
            DevPtsEndpoint::Slave(tty) => Some(tty.clone()),
            DevPtsEndpoint::Master(_) => None,
        }
    }

    /// Return the connected slave TTY for either endpoint.
    ///
    /// # Returns
    ///
    /// The slave TTY for master and slave endpoints.
    pub fn connected_tty_device(&self) -> Arc<TtyDevice> {
        self.pair.slave()
    }

    /// Return whether this object is a PTY master endpoint.
    ///
    /// # Returns
    ///
    /// `true` for master endpoints.
    pub fn is_master_endpoint(&self) -> bool {
        matches!(self.endpoint, DevPtsEndpoint::Master(_))
    }

    /// Return queued input length for this endpoint.
    ///
    /// # Returns
    ///
    /// Bytes available to read from the endpoint.
    pub fn input_len(&self) -> usize {
        match &self.endpoint {
            DevPtsEndpoint::Master(master) => master.input_len(),
            DevPtsEndpoint::Slave(slave) => slave.input_len(),
        }
    }
}

impl Drop for DevPtsFileObject {
    fn drop(&mut self) {
        if matches!(self.endpoint, DevPtsEndpoint::Slave(_)) {
            self.pair.close_slave_endpoint();
        }
        if let Some((state, number)) = &self.release {
            state.release_pair(*number);
        }
    }
}

impl StreamOps for DevPtsFileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let count = match &self.endpoint {
            DevPtsEndpoint::Master(master) => master.read(buffer),
            DevPtsEndpoint::Slave(slave) => slave.read(buffer),
        };
        *self.position.write() += count as u64;
        Ok(count)
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        let count = match &self.endpoint {
            DevPtsEndpoint::Master(master) => master.write(buffer),
            DevPtsEndpoint::Slave(slave) => slave.write(buffer),
        }
        .map_err(|_| StreamError::DeviceError)?;
        *self.position.write() += count as u64;
        Ok(count)
    }
}

impl ControlOps for DevPtsFileObject {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            pty_ctl::SCTL_PTY_GET_NUMBER => {
                return self
                    .pty_number()
                    .map(|number| number as i32)
                    .ok_or("PTY number is only available on master endpoints");
            }
            pty_ctl::SCTL_PTY_SET_LOCKED => {
                if self.set_pty_slave_locked(arg != 0) {
                    return Ok(0);
                }
                return Err("PTY slave lock is only available on master endpoints");
            }
            pty_ctl::SCTL_PTY_GET_LOCKED => {
                return self
                    .pty_slave_locked()
                    .map(|locked| locked as i32)
                    .ok_or("PTY slave lock is only available on master endpoints");
            }
            _ => {}
        }

        match &self.endpoint {
            DevPtsEndpoint::Master(master) => master.control(command, arg),
            DevPtsEndpoint::Slave(slave) => slave.control(command, arg),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        let mut commands = match &self.endpoint {
            DevPtsEndpoint::Master(master) => master.supported_control_commands(),
            DevPtsEndpoint::Slave(slave) => slave.supported_control_commands(),
        };
        if matches!(self.endpoint, DevPtsEndpoint::Master(_)) {
            commands.push((pty_ctl::SCTL_PTY_GET_NUMBER, "Get PTY slave number"));
            commands.push((pty_ctl::SCTL_PTY_SET_LOCKED, "Set PTY slave lock state"));
            commands.push((pty_ctl::SCTL_PTY_GET_LOCKED, "Get PTY slave lock state"));
        }
        commands
    }
}

impl MemoryMappingOps for DevPtsFileObject {
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        match &self.endpoint {
            DevPtsEndpoint::Master(master) => master.get_mapping_info(offset, length),
            DevPtsEndpoint::Slave(slave) => slave.get_mapping_info(offset, length),
        }
    }

    fn on_mapped(&self, vaddr: usize, paddr: usize, length: usize, offset: usize) {
        match &self.endpoint {
            DevPtsEndpoint::Master(master) => master.on_mapped(vaddr, paddr, length, offset),
            DevPtsEndpoint::Slave(slave) => slave.on_mapped(vaddr, paddr, length, offset),
        }
    }

    fn on_unmapped(&self, vaddr: usize, length: usize) {
        match &self.endpoint {
            DevPtsEndpoint::Master(master) => master.on_unmapped(vaddr, length),
            DevPtsEndpoint::Slave(slave) => slave.on_unmapped(vaddr, length),
        }
    }

    fn supports_mmap(&self) -> bool {
        match &self.endpoint {
            DevPtsEndpoint::Master(master) => master.supports_mmap(),
            DevPtsEndpoint::Slave(slave) => slave.supports_mmap(),
        }
    }
}

impl FileObject for DevPtsFileObject {
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut position = self.position.write();
        let new_pos = match whence {
            SeekFrom::Start(offset) => offset,
            SeekFrom::Current(offset) => {
                if offset >= 0 {
                    position.saturating_add(offset as u64)
                } else {
                    position.saturating_sub((-offset) as u64)
                }
            }
            SeekFrom::End(_) => return Err(StreamError::NotSupported),
        };
        *position = new_pos;
        Ok(new_pos)
    }

    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        self.node.metadata().map_err(StreamError::from)
    }

    fn truncate(&self, _size: u64) -> Result<(), StreamError> {
        Err(StreamError::NotSupported)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Selectable for DevPtsFileObject {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        match &self.endpoint {
            DevPtsEndpoint::Master(master) => master.current_ready(interest),
            DevPtsEndpoint::Slave(slave) => slave.current_ready(interest),
        }
    }

    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        trapframe: &mut crate::arch::Trapframe,
        timeout_ticks: Option<u64>,
        min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        match &self.endpoint {
            DevPtsEndpoint::Master(master) => {
                master.wait_until_ready(interest, trapframe, timeout_ticks, min_wait_ticks)
            }
            DevPtsEndpoint::Slave(slave) => {
                slave.wait_until_ready(interest, trapframe, timeout_ticks, min_wait_ticks)
            }
        }
    }

    fn set_nonblocking(&self, enabled: bool) {
        match &self.endpoint {
            DevPtsEndpoint::Master(master) => master.set_nonblocking(enabled),
            DevPtsEndpoint::Slave(slave) => slave.set_nonblocking(enabled),
        }
    }

    fn is_nonblocking(&self) -> bool {
        match &self.endpoint {
            DevPtsEndpoint::Master(master) => master.is_nonblocking(),
            DevPtsEndpoint::Slave(slave) => slave.is_nonblocking(),
        }
    }
}

struct DevPtsDirectoryObject {
    node: Arc<DevPtsNode>,
    position: IrqRwSpinLock<usize>,
}

impl DevPtsDirectoryObject {
    fn new(node: Arc<DevPtsNode>) -> Self {
        Self {
            node,
            position: IrqRwSpinLock::new(0),
        }
    }
}

impl StreamOps for DevPtsDirectoryObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let entries = self.node.readdir().map_err(StreamError::from)?;
        let position = *self.position.read();
        if position >= entries.len() {
            return Ok(0);
        }

        let entry = &entries[position];
        let internal = crate::fs::DirectoryEntryInternal {
            name: entry.name.clone(),
            file_type: entry.file_type.clone(),
            size: 0,
            file_id: entry.file_id,
            metadata: None,
        };
        let dir_entry = crate::fs::DirectoryEntry::from_internal(&internal);
        let entry_size = dir_entry.entry_size();
        if buffer.len() < entry_size {
            return Err(StreamError::InvalidArgument);
        }

        let entry_bytes =
            unsafe { core::slice::from_raw_parts(&dir_entry as *const _ as *const u8, entry_size) };
        buffer[..entry_size].copy_from_slice(entry_bytes);
        *self.position.write() += 1;
        Ok(entry_size)
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::PermissionDenied)
    }
}

impl ControlOps for DevPtsDirectoryObject {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported on DevPTS directories")
    }
}

impl MemoryMappingOps for DevPtsDirectoryObject {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("Memory mapping not supported for DevPTS directories")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {}

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {}

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl FileObject for DevPtsDirectoryObject {
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let entries = self.node.readdir().map_err(StreamError::from)?;
        let entry_count = entries.len() as u64;
        let mut position = self.position.write();
        let new_pos = match whence {
            SeekFrom::Start(offset) => offset,
            SeekFrom::Current(offset) => {
                if offset >= 0 {
                    *position as u64 + offset as u64
                } else {
                    (*position as u64).saturating_sub((-offset) as u64)
                }
            }
            SeekFrom::End(offset) => {
                if offset >= 0 {
                    entry_count + offset as u64
                } else {
                    entry_count.saturating_sub((-offset) as u64)
                }
            }
        };
        *position = new_pos as usize;
        Ok(new_pos)
    }

    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        self.node.metadata().map_err(StreamError::from)
    }

    fn truncate(&self, _size: u64) -> Result<(), StreamError> {
        Err(StreamError::PermissionDenied)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Selectable for DevPtsDirectoryObject {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let mut set = ReadySet::none();
        if interest.read {
            set.read = true;
        }
        if interest.write {
            set.write = true;
        }
        set
    }

    fn wait_until_ready(
        &self,
        _interest: ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::Ready
    }

    fn is_nonblocking(&self) -> bool {
        true
    }
}

/// DevPTS filesystem driver.
pub struct DevPtsFSDriver;

impl FileSystemDriver for DevPtsFSDriver {
    fn name(&self) -> &'static str {
        "devpts"
    }

    fn filesystem_type(&self) -> FileSystemType {
        FileSystemType::Device
    }

    fn create(&self) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        Ok(DevPtsFS::new() as Arc<dyn FileSystemOperations>)
    }

    fn create_from_option_string(
        &self,
        _options: &str,
    ) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        self.create()
    }
}

fn parse_pty_number(name: &str) -> Result<usize, FileSystemError> {
    if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(FileSystemError::new(
            FileSystemErrorKind::NotFound,
            format!("Invalid DevPTS entry '{}'", name),
        ));
    }

    let mut value = 0usize;
    for byte in name.bytes() {
        value = value
            .checked_mul(10)
            .and_then(|value| value.checked_add((byte - b'0') as usize))
            .ok_or_else(|| {
                FileSystemError::new(FileSystemErrorKind::InvalidData, "PTY number overflow")
            })?;
    }
    Ok(value)
}

/// Register the DevPTS filesystem driver.
///
/// This is exposed so DevFS can force DevPTS availability for `/dev/pts`
/// mount setups, while the normal driver initcall path still registers it
/// during boot.
pub fn register_driver() {
    let fs_driver_manager = get_fs_driver_manager();
    fs_driver_manager.register_driver(Box::new(DevPtsFSDriver));
}

driver_initcall!(register_driver);

#[cfg(test)]
mod tests {
    use crate::device::char::TtyControl;

    use super::*;

    #[test_case]
    fn test_devpts_ptmx_open_allocates_slave_node() {
        let devpts = DevPtsFS::new();
        let root = devpts.root_node();
        let ptmx = devpts.lookup(&root, &"ptmx".to_string()).unwrap();
        let master = devpts.open(&ptmx, 0).unwrap();
        let master_devpts = master.as_any().downcast_ref::<DevPtsFileObject>().unwrap();
        let number = master_devpts.pty_number().unwrap();

        assert!(devpts.lookup(&root, &number.to_string()).is_ok());
    }

    #[test_case]
    fn test_devpts_slave_requires_unlock() {
        let devpts = DevPtsFS::new();
        let root = devpts.root_node();
        let ptmx = devpts.lookup(&root, &"ptmx".to_string()).unwrap();
        let master = devpts.open(&ptmx, 0).unwrap();
        let master_devpts = master.as_any().downcast_ref::<DevPtsFileObject>().unwrap();
        let number = master_devpts.pty_number().unwrap();
        let slave = devpts.lookup(&root, &number.to_string()).unwrap();

        assert!(devpts.open(&slave, 0).is_err());
        assert!(master_devpts.set_pty_slave_locked(false));
        assert!(devpts.open(&slave, 0).is_ok());
    }

    #[test_case]
    fn test_devpts_master_native_controls() {
        let devpts = DevPtsFS::new();
        let root = devpts.root_node();
        let ptmx = devpts.lookup(&root, &"ptmx".to_string()).unwrap();
        let master = devpts.open(&ptmx, 0).unwrap();

        let number = master.control(pty_ctl::SCTL_PTY_GET_NUMBER, 0).unwrap();
        assert_eq!(number, 0);
        assert_eq!(master.control(pty_ctl::SCTL_PTY_GET_LOCKED, 0).unwrap(), 1);
        assert_eq!(master.control(pty_ctl::SCTL_PTY_SET_LOCKED, 0).unwrap(), 0);
        assert_eq!(master.control(pty_ctl::SCTL_PTY_GET_LOCKED, 0).unwrap(), 0);
    }

    #[test_case]
    fn test_devpts_master_drop_releases_slave_node() {
        let devpts = DevPtsFS::new();
        let root = devpts.root_node();
        let ptmx = devpts.lookup(&root, &"ptmx".to_string()).unwrap();
        let master = devpts.open(&ptmx, 0).unwrap();
        let number = master
            .as_any()
            .downcast_ref::<DevPtsFileObject>()
            .unwrap()
            .pty_number()
            .unwrap();

        assert!(devpts.lookup(&root, &number.to_string()).is_ok());
        drop(master);
        assert!(devpts.lookup(&root, &number.to_string()).is_err());
    }

    #[test_case]
    fn test_devpts_reuses_released_number() {
        let devpts = DevPtsFS::new();
        let root = devpts.root_node();
        let ptmx = devpts.lookup(&root, &"ptmx".to_string()).unwrap();

        let first = devpts.open(&ptmx, 0).unwrap();
        assert_eq!(first.control(pty_ctl::SCTL_PTY_GET_NUMBER, 0).unwrap(), 0);
        drop(first);

        let second = devpts.open(&ptmx, 0).unwrap();
        assert_eq!(second.control(pty_ctl::SCTL_PTY_GET_NUMBER, 0).unwrap(), 0);
    }

    #[test_case]
    fn test_devpts_master_slave_io() {
        let devpts = DevPtsFS::new();
        let root = devpts.root_node();
        let ptmx = devpts.lookup(&root, &"ptmx".to_string()).unwrap();
        let master = devpts.open(&ptmx, 0).unwrap();
        let master_devpts = master.as_any().downcast_ref::<DevPtsFileObject>().unwrap();
        let number = master_devpts.pty_number().unwrap();
        assert!(master_devpts.set_pty_slave_locked(false));

        let slave_node = devpts.lookup(&root, &number.to_string()).unwrap();
        let slave = devpts.open(&slave_node, 0).unwrap();
        let slave_devpts = slave.as_any().downcast_ref::<DevPtsFileObject>().unwrap();
        slave_devpts.tty_device().unwrap().set_echo(false);

        master.write(b"hello\n").unwrap();
        let mut slave_buffer = [0u8; 6];
        assert_eq!(slave.read(&mut slave_buffer).unwrap(), 6);
        assert_eq!(&slave_buffer, b"hello\n");

        slave.write(b"out\n").unwrap();
        let mut master_buffer = [0u8; 5];
        assert_eq!(master.read(&mut master_buffer).unwrap(), 5);
        assert_eq!(&master_buffer, b"out\r\n");
    }

    #[test_case]
    fn test_devpts_slave_exclusive_rejects_new_open() {
        let devpts = DevPtsFS::new();
        let root = devpts.root_node();
        let ptmx = devpts.lookup(&root, &"ptmx".to_string()).unwrap();
        let master = devpts.open(&ptmx, 0).unwrap();
        let master_devpts = master.as_any().downcast_ref::<DevPtsFileObject>().unwrap();
        let number = master_devpts.pty_number().unwrap();
        assert!(master_devpts.set_pty_slave_locked(false));

        let slave_node = devpts.lookup(&root, &number.to_string()).unwrap();
        let slave = devpts.open(&slave_node, 0).unwrap();
        let slave_devpts = slave.as_any().downcast_ref::<DevPtsFileObject>().unwrap();
        let tty = slave_devpts.tty_device().unwrap();

        tty.set_exclusive(true);
        assert!(devpts.open(&slave_node, 0).is_err());
        tty.set_exclusive(false);
        assert!(devpts.open(&slave_node, 0).is_ok());
    }

    #[test_case]
    fn test_devpts_mount_readdir_crosses_mountpoint() {
        let vfs = crate::fs::VfsManager::new();
        vfs.create_dir("/dev").unwrap();
        vfs.mount(crate::fs::vfs_v2::drivers::devfs::DevFS::new(), "/dev", 0)
            .unwrap();
        vfs.mount(DevPtsFS::new(), "/dev/pts", 0).unwrap();

        let entries = vfs.readdir("/dev/pts").unwrap();
        assert!(
            entries.iter().any(|entry| entry.name == "ptmx"),
            "mounted devpts root should expose ptmx"
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.name == "." && entry.file_id == ROOT_ID),
            "readdir should cross into devpts root rather than devfs mountpoint"
        );
    }

    #[test_case]
    fn test_dev_ptmx_relative_symlink_opens_devpts_ptmx() {
        let vfs = crate::fs::VfsManager::new();
        vfs.create_dir("/dev").unwrap();
        vfs.mount(crate::fs::vfs_v2::drivers::devfs::DevFS::new(), "/dev", 0)
            .unwrap();
        vfs.mount(DevPtsFS::new(), "/dev/pts", 0).unwrap();

        let master = vfs.open("/dev/ptmx", 0).unwrap();
        let master_file = master.as_file().unwrap();
        assert_eq!(
            master_file
                .control(pty_ctl::SCTL_PTY_GET_NUMBER, 0)
                .unwrap(),
            0
        );
    }
}
