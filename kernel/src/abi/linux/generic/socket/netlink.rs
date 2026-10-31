//! Linux NETLINK_ROUTE adapter for native network state observations.
//!
//! Scarlet has no separate Linux network namespace yet. The socket therefore
//! exposes its native routing observations in one place instead of creating a
//! second network stack. No link is reported until a native device provides it.
use super::*;
use crate::fs::{FileMetadata, FilePermission, FileType, SeekFrom};
use crate::object::capability::{
    ControlOps, FileObject, MemoryMappingOps, ReadyInterest, ReadySet, SelectWaitOutcome,
    StreamOps,
};
use core::{
    any::Any,
    sync::atomic::{AtomicBool, AtomicI32, Ordering},
};

const SO_RCVBUF_COMPAT: i32 = 8;
const SO_RCVBUFFORCE_COMPAT: i32 = 33;
const SO_TIMESTAMP_COMPAT: i32 = 29;

pub struct RouteSocket {
    kind: i32,
    protocol: i32,
    bound: AtomicBool,
    passcred: AtomicBool,
    nonblocking: AtomicBool,
    receive_buffer: AtomicI32,
}

pub fn from_object(object: &KernelObject) -> Option<&RouteSocket> {
    object.as_file()?.as_any().downcast_ref()
}

pub fn create(abi: &mut LinuxAbi, task: &crate::task::Task, kind: i32, flags: i32, protocol: i32) -> usize {
    if !matches!(kind, SOCK_DGRAM | SOCK_RAW) || flags & !(SOCK_NONBLOCK | SOCK_CLOEXEC) != 0 {
        return errno::to_result(errno::EINVAL);
    }
    let socket = Arc::new(RouteSocket {
        kind,
        protocol,
        bound: AtomicBool::new(false),
        passcred: AtomicBool::new(false),
        nonblocking: AtomicBool::new(flags & SOCK_NONBLOCK != 0),
        receive_buffer: AtomicI32::new(212_992),
    });
    let handle = match task.handle_table.insert(KernelObject::File(socket)) {
        Ok(handle) => handle,
        Err(_) => return errno::to_result(errno::EMFILE),
    };
    match abi.allocate_fd(handle) {
        Ok(fd) => {
            let _ = abi.set_fd_flags(fd, if flags & SOCK_CLOEXEC != 0 { FD_CLOEXEC } else { 0 });
            let _ = abi.set_file_status_flags(fd, if flags & SOCK_NONBLOCK != 0 { O_NONBLOCK as u32 } else { 0 });
            fd
        }
        Err(_) => {
            let _ = task.handle_table.remove(handle);
            errno::to_result(errno::EMFILE)
        }
    }
}

impl RouteSocket {
    pub fn send_message(&self, task:&crate::task::Task, address:usize, length:usize, destination:usize, destination_length:usize)->usize {
        if length>65536{return errno::to_result(errno::EMSGSIZE);}
        if destination!=0 && destination_length>0 {
            if destination_length<2 || destination_length>128{return errno::to_result(errno::EINVAL);}
            let mut bytes=[0u8;128];
            if copy_from_user(task,destination,&mut bytes[..destination_length]).is_err(){return errno::to_result(errno::EFAULT);}
            if u16::from_ne_bytes(bytes[..2].try_into().unwrap())!=16{return errno::to_result(errno::EAFNOSUPPORT);}
        }
        let mut data=Vec::new();
        if data.try_reserve_exact(length).is_err(){return errno::to_result(errno::ENOMEM);}
        data.resize(length,0);
        if copy_from_user(task,address,&mut data).is_err(){return errno::to_result(errno::EFAULT);}
        length
    }

    pub fn bind(&self, task: &crate::task::Task, address: usize, length: usize) -> usize {
        if length != 12 {
            return errno::to_result(errno::EINVAL);
        }
        let mut bytes = [0u8; 12];
        if copy_from_user(task, address, &mut bytes).is_err() {
            return errno::to_result(errno::EFAULT);
        }
        let pid = u32::from_ne_bytes(bytes[4..8].try_into().unwrap());
        if u16::from_ne_bytes(bytes[..2].try_into().unwrap()) != 16
            || (pid != 0
                && task.get_namespace().resolve_local_id(task.get_thread_group_id()) != Some(pid as usize))
        {
            return errno::to_result(errno::EINVAL);
        }
        if self.bound.swap(true, Ordering::AcqRel) {
            return errno::to_result(errno::EINVAL);
        }
        0
    }

    pub fn set_option(&self, task: &crate::task::Task, level: i32, name: i32, address: usize, length: usize) -> usize {
        if level != SOL_SOCKET || !matches!(name, SO_PASSCRED | SO_RCVBUF_COMPAT | SO_RCVBUFFORCE_COMPAT | SO_TIMESTAMP_COMPAT) {
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
        if name == SO_PASSCRED || name == SO_TIMESTAMP_COMPAT {
            self.passcred.store(value != 0, Ordering::Release);
        } else {
            if value < 0 {
                return errno::to_result(errno::EINVAL);
            }
            self.receive_buffer.store(value.max(4096), Ordering::Release);
        }
        0
    }

    pub fn get_option(&self, task: &crate::task::Task, level: i32, name: i32, address: usize, length_ptr: usize) -> usize {
        if level != SOL_SOCKET {
            return errno::to_result(errno::ENOPROTOOPT);
        }
        let value = match name {
            SO_RCVBUF_COMPAT => self.receive_buffer.load(Ordering::Acquire),
            SO_PASSCRED => i32::from(self.passcred.load(Ordering::Acquire)),
            SO_TYPE => self.kind,
            SO_ERROR => 0,
            _ => return errno::to_result(errno::ENOPROTOOPT),
        };
        let mut length_bytes = [0; 4];
        if copy_from_user(task, length_ptr, &mut length_bytes).is_err() {
            return errno::to_result(errno::EFAULT);
        }
        let count = (u32::from_ne_bytes(length_bytes) as usize).min(4);
        if copy_to_user(task, address, &value.to_ne_bytes()[..count]).is_err()
            || copy_to_user(task, length_ptr, &(count as u32).to_ne_bytes()).is_err()
        {
            return errno::to_result(errno::EFAULT);
        }
        0
    }

    pub fn receive_message(&self, _task: &crate::task::Task, _address: usize, _flags: i32) -> usize {
        errno::to_result(errno::EAGAIN)
    }
}

impl StreamOps for RouteSocket {
    fn read(&self, _: &mut [u8]) -> Result<usize, StreamError> {
        Err(StreamError::WouldBlock)
    }
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        let _ = self.protocol;
        Ok(buffer.len())
    }
}
impl Selectable for RouteSocket {
    fn current_ready(&self, _: ReadyInterest) -> ReadySet {
        ReadySet::none()
    }
    fn wait_until_ready(&self, _: ReadyInterest, _: &mut Trapframe, _: Option<u64>, _: u64) -> SelectWaitOutcome {
        SelectWaitOutcome::TimedOut
    }
    fn set_nonblocking(&self, value: bool) {
        self.nonblocking.store(value, Ordering::Release);
    }
    fn is_nonblocking(&self) -> bool {
        self.nonblocking.load(Ordering::Acquire)
    }
}
impl ControlOps for RouteSocket {}
impl MemoryMappingOps for RouteSocket {
    fn get_mapping_info(&self, _: usize, _: usize) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("netlink route socket is not memory mapped")
    }
}
impl FileObject for RouteSocket {
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
            permissions: FilePermission { read: true, write: false, execute: false },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: 0,
            link_count: 1,
        })
    }
}
