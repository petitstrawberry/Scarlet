//! Linux NETLINK_KOBJECT_UEVENT adapter over native endpoint observations.
use super::*;
use crate::{
    device::{
        DeviceType,
        lifecycle::{Action, Endpoint, Observation, Subscription},
        manager::DeviceManager,
    },
    fs::{FileMetadata, FilePermission, FileType, SeekFrom, vfs_v2::VfsManager},
    object::capability::{
        ControlOps, FileObject, MemoryMappingOps, ReadyInterest, ReadySet, SelectWaitOutcome,
        StreamOps,
    },
    sync::{IrqSpinLock, waker::WaitResult},
};
use alloc::{collections::BTreeSet, format, vec};
use core::{
    any::Any,
    sync::atomic::{AtomicBool, Ordering},
};

pub struct UeventSocket {
    subscription: Arc<Subscription>,
    view: Arc<VfsManager>,
    bound: AtomicBool,
    passcred: AtomicBool,
    kind: i32,
    nonblocking: AtomicBool,
    seen: IrqSpinLock<BTreeSet<usize>>,
    peek: IrqSpinLock<Option<Vec<u8>>>,
}
pub fn visible(view: &VfsManager, endpoint: &Endpoint) -> bool {
    if endpoint.name.is_empty()
        || endpoint.name.contains('/')
        || matches!(endpoint.name.as_str(), "." | "..")
    {
        return false;
    }
    view.resolve_path(&format!("/dev/{}",endpoint.name)).ok().and_then(|(entry,_)|entry.node().metadata().ok()).is_some_and(|metadata|
        matches!(metadata.file_type,FileType::CharDevice(info)|FileType::BlockDevice(info) if info.device_id==endpoint.id && info.device_type==endpoint.kind))
}
pub fn encode(record: &Observation) -> Vec<u8> {
    let endpoint = &record.endpoint;
    let id = endpoint.id as u64;
    let major = ((id >> 8) & 0xfff) | ((id >> 32) & 0xfffff000);
    let minor = (id & 0xff) | ((id >> 12) & 0xffffff00);
    let action = if record.action == Action::Removed {
        "remove"
    } else {
        "add"
    };
    let subsystem = if endpoint.kind == DeviceType::Block {
        "block"
    } else {
        "scarlet"
    };
    let path = format!("/devices/scarlet/{}", endpoint.name);
    format!("{action}@{path}\0ACTION={action}\0DEVPATH={path}\0SUBSYSTEM={subsystem}\0DEVNAME={}\0MAJOR={major}\0MINOR={minor}\0SEQNUM={}\0",endpoint.name,record.sequence).into_bytes()
}
pub fn from_object(object: &KernelObject) -> Option<&UeventSocket> {
    object.as_file()?.as_any().downcast_ref()
}
pub fn create(
    abi: &mut LinuxAbi,
    task: &crate::task::Task,
    kind: i32,
    flags: i32,
    protocol: i32,
) -> usize {
    if protocol != 15 {
        return errno::to_result(errno::EPROTONOSUPPORT);
    }
    if !matches!(kind, SOCK_DGRAM | SOCK_RAW) || flags & !(SOCK_NONBLOCK | SOCK_CLOEXEC) != 0 {
        return errno::to_result(errno::EINVAL);
    }
    let Some(view) = task.get_vfs() else {
        return errno::to_result(errno::EIO);
    };
    let manager = DeviceManager::get_manager();
    let subscription = manager.lifecycle.subscribe();
    let seen = manager
        .endpoint_snapshot()
        .0
        .iter()
        .filter(|endpoint| visible(&view, endpoint))
        .map(|endpoint| endpoint.id)
        .collect();
    let socket = Arc::new(UeventSocket {
        subscription,
        view,
        kind,
        bound: AtomicBool::new(false),
        passcred: AtomicBool::new(false),
        nonblocking: AtomicBool::new(flags & SOCK_NONBLOCK != 0),
        seen: IrqSpinLock::new(seen),
        peek: IrqSpinLock::new(None),
    });
    let handle = match task.handle_table.insert(KernelObject::File(socket)) {
        Ok(handle) => handle,
        Err(_) => return errno::to_result(errno::EMFILE),
    };
    match abi.allocate_fd(handle) {
        Ok(fd) => {
            let _ = abi.set_fd_flags(
                fd,
                if flags & SOCK_CLOEXEC != 0 {
                    FD_CLOEXEC
                } else {
                    0
                },
            );
            let _ = abi.set_file_status_flags(
                fd,
                if flags & SOCK_NONBLOCK != 0 {
                    O_NONBLOCK as u32
                } else {
                    0
                },
            );
            fd
        }
        Err(_) => {
            let _ = task.handle_table.remove(handle);
            errno::to_result(errno::EMFILE)
        }
    }
}
impl UeventSocket {
    pub fn bind(&self, task: &crate::task::Task, address: usize, length: usize) -> usize {
        if length != 12 {
            return errno::to_result(errno::EINVAL);
        }
        let mut bytes = [0u8; 12];
        if copy_from_user(task, address, &mut bytes).is_err() {
            return errno::to_result(errno::EFAULT);
        }
        let pid = u32::from_ne_bytes(bytes[4..8].try_into().unwrap());
        let groups = u32::from_ne_bytes(bytes[8..12].try_into().unwrap());
        if u16::from_ne_bytes(bytes[..2].try_into().unwrap()) != 16
            || (pid != 0
                && task
                    .get_namespace()
                    .resolve_local_id(task.get_thread_group_id())
                    != Some(pid as usize))
            || groups & 1 == 0
        {
            return errno::to_result(errno::EINVAL);
        }
        if self.bound.swap(true, Ordering::AcqRel) {
            return errno::to_result(errno::EINVAL);
        }
        0
    }
    pub fn set_option(
        &self,
        task: &crate::task::Task,
        level: i32,
        name: i32,
        address: usize,
        length: usize,
    ) -> usize {
        if level != SOL_SOCKET || !matches!(name, SO_PASSCRED | 8) {
            return errno::to_result(errno::ENOPROTOOPT);
        }
        if length != 4 {
            return errno::to_result(errno::EINVAL);
        }
        let mut bytes = [0; 4];
        if copy_from_user(task, address, &mut bytes).is_err() {
            return errno::to_result(errno::EFAULT);
        }
        let value = i32::from_ne_bytes(bytes);
        if name == SO_PASSCRED {
            self.passcred.store(value != 0, Ordering::Release);
            return 0;
        }
        if value < 128
            || self
                .subscription
                .resize((value as usize).saturating_mul(2))
                .is_err()
        {
            return errno::to_result(errno::EINVAL);
        }
        0
    }
    pub fn get_option(
        &self,
        task: &crate::task::Task,
        level: i32,
        name: i32,
        address: usize,
        length: usize,
    ) -> usize {
        if level != SOL_SOCKET {
            return errno::to_result(errno::ENOPROTOOPT);
        }
        let value = match name {
            8 => self.subscription.capacity() as i32,
            SO_PASSCRED => i32::from(self.passcred.load(Ordering::Acquire)),
            SO_TYPE => self.kind,
            SO_ERROR => 0,
            _ => return errno::to_result(errno::ENOPROTOOPT),
        };
        let mut bytes = [0; 4];
        if copy_from_user(task, length, &mut bytes).is_err() {
            return errno::to_result(errno::EFAULT);
        }
        let count = (u32::from_ne_bytes(bytes) as usize).min(4);
        if copy_to_user(task, address, &value.to_ne_bytes()[..count]).is_err()
            || copy_to_user(task, length, &(count as u32).to_ne_bytes()).is_err()
        {
            return errno::to_result(errno::EFAULT);
        }
        0
    }
    fn packet(&self, task: &crate::task::Task, flags: i32) -> Result<Vec<u8>, usize> {
        if flags & !(MSG_DONTWAIT | 0x2 | 0x20 | 0x40000000) != 0 {
            return Err(errno::EOPNOTSUPP);
        }
        if !self.bound.load(Ordering::Acquire) {
            return Err(errno::EINVAL);
        }
        loop {
            let buffered = { self.peek.lock().take() };
            if let Some(packet) = buffered {
                if flags & 2 != 0 {
                    *self.peek.lock() = Some(packet.clone());
                }
                return Ok(packet);
            }
            match self.subscription.next() {
                Err(()) => return Err(errno::ENOBUFS),
                Ok(Some(record)) => {
                    if !matches!(record.endpoint.kind, DeviceType::Char | DeviceType::Block) {
                        continue;
                    }
                    let allowed = if record.action == Action::Removed {
                        self.seen.lock().remove(&record.endpoint.id)
                    } else {
                        let allowed = visible(&self.view, &record.endpoint);
                        if allowed {
                            self.seen.lock().insert(record.endpoint.id);
                        }
                        allowed
                    };
                    if !allowed {
                        continue;
                    }
                    let packet = encode(&record);
                    if flags & 2 != 0 {
                        *self.peek.lock() = Some(packet.clone());
                    }
                    return Ok(packet);
                }
                Ok(None) => {}
            }
            if flags & MSG_DONTWAIT != 0 || self.nonblocking.load(Ordering::Acquire) {
                return Err(errno::EAGAIN);
            }
            if DeviceManager::get_manager()
                .lifecycle
                .changed
                .wait_with_condition(task.get_id(), task.get_trapframe(), None, 0, || {
                    self.subscription.pending()
                })
                == WaitResult::Interrupted
            {
                return Err(errno::EINTR);
            }
        }
    }
    pub fn receive_message(&self, task: &crate::task::Task, address: usize, flags: i32) -> usize {
        (|| -> Result<usize, usize> {
            let mut msg = read_linux_msghdr(task, address)?;
            if msg.msg_iovlen > 1024 {
                return Err(errno::EINVAL);
            }
            let iovecs = read_linux_iovecs(task, msg.msg_iov as usize, msg.msg_iovlen as usize)?;
            let packet = self.packet(task, flags)?;
            let mut copied = 0;
            for iov in iovecs {
                let count = (iov.iov_len as usize).min(packet.len() - copied);
                if count > 0 {
                    copy_to_user(task, iov.iov_base as usize, &packet[copied..copied + count])
                        .map_err(|_| errno::EFAULT)?;
                    copied += count;
                }
                if copied == packet.len() {
                    break;
                }
            }
            msg.msg_flags = if copied < packet.len() { 0x20 } else { 0 };
            if msg.msg_name != 0 {
                let mut source = [0u8; 12];
                source[..2].copy_from_slice(&16u16.to_ne_bytes());
                source[8..12].copy_from_slice(&1u32.to_ne_bytes());
                let count = (msg.msg_namelen as usize).min(12);
                copy_to_user(task, msg.msg_name as usize, &source[..count])
                    .map_err(|_| errno::EFAULT)?;
                msg.msg_namelen = 12;
            }
            let mut control = vec![0u8; 32];
            let wanted = self.passcred.load(Ordering::Acquire);
            if wanted && msg.msg_control != 0 && msg.msg_controllen >= 28 {
                control[..8].copy_from_slice(&28usize.to_ne_bytes());
                control[8..12].copy_from_slice(&SOL_SOCKET.to_ne_bytes());
                control[12..16].copy_from_slice(&SCM_CREDENTIALS.to_ne_bytes());
                let count = (msg.msg_controllen as usize).min(32);
                copy_to_user(task, msg.msg_control as usize, &control[..count])
                    .map_err(|_| errno::EFAULT)?;
                msg.msg_controllen = count as u64;
            } else {
                if wanted {
                    msg.msg_flags |= 8;
                }
                msg.msg_controllen = 0;
            }
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    (&msg as *const LinuxMsghdr).cast::<u8>(),
                    size_of::<LinuxMsghdr>(),
                )
            };
            copy_to_user(task, address, bytes).map_err(|_| errno::EFAULT)?;
            Ok(if flags & 0x20 != 0 {
                packet.len()
            } else {
                copied
            })
        })()
        .unwrap_or_else(errno::to_result)
    }
}
impl StreamOps for UeventSocket {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let task = mytask().ok_or(StreamError::InvalidArgument)?;
        let packet = self.packet(&task, 0).map_err(|error| match error {
            errno::EAGAIN => StreamError::WouldBlock,
            errno::EINTR => StreamError::Interrupted,
            _ => StreamError::InvalidArgument,
        })?;
        let count = buffer.len().min(packet.len());
        buffer[..count].copy_from_slice(&packet[..count]);
        Ok(count)
    }
    fn write(&self, _: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::PermissionDenied)
    }
}
impl Selectable for UeventSocket {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let mut set = ReadySet::none();
        set.read = interest.read
            && self.bound.load(Ordering::Acquire)
            && (self.peek.lock().is_some() || self.subscription.pending());
        set
    }
    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        frame: &mut Trapframe,
        timeout: Option<u64>,
        min_wait: u64,
    ) -> SelectWaitOutcome {
        let Some(task) = mytask() else {
            return SelectWaitOutcome::Ready;
        };
        if self.current_ready(interest).read {
            return SelectWaitOutcome::Ready;
        }
        match DeviceManager::get_manager()
            .lifecycle
            .changed
            .wait_with_condition(task.get_id(), frame, timeout, min_wait, || {
                self.current_ready(interest).read
            }) {
            WaitResult::Interrupted => SelectWaitOutcome::Interrupted,
            WaitResult::TimedOut => SelectWaitOutcome::TimedOut,
            WaitResult::Woken => SelectWaitOutcome::Ready,
        }
    }
    fn set_nonblocking(&self, value: bool) {
        self.nonblocking.store(value, Ordering::Release);
    }
    fn is_nonblocking(&self) -> bool {
        self.nonblocking.load(Ordering::Acquire)
    }
}
impl ControlOps for UeventSocket {}
impl MemoryMappingOps for UeventSocket {
    fn get_mapping_info(
        &self,
        _: usize,
        _: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("uevent is not memory mapped")
    }
}
impl FileObject for UeventSocket {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn seek(&self, _: SeekFrom) -> Result<u64, StreamError> {
        Err(StreamError::NotSupported)
    }
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(FileMetadata {
            file_type: FileType::Socket(crate::fs::SocketFileInfo { socket_id: 0 }),
            size: 0,
            permissions: FilePermission {
                read: true,
                write: false,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: 0,
            link_count: 1,
        })
    }
}
