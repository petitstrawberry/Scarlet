//! Linux log submission and legacy text reads over Scarlet's native log.
//!
//! /dev/kmsg keeps the native raw read format: the native ring has neither
//! timestamps nor Linux record sequence numbers. /proc/kmsg and syslog READ
//! share a consuming text cursor, independent of every native/device reader.

use alloc::{collections::VecDeque, sync::Arc, vec, vec::Vec};
use core::{
    any::Any,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use super::{
    LinuxAbi, errno,
    fs::{FD_CLOEXEC, O_CLOEXEC, O_DIRECTORY, O_NONBLOCK, O_PATH},
};
use crate::{
    arch::Trapframe,
    fs::{FileMetadata, FilePermission, FileType, SeekFrom},
    library::std::usercopy::{copy_from_user, copy_to_user},
    object::{
        KernelObject,
        capability::{
            ControlOps, FileObject, MemoryMappingInfo, MemoryMappingOps, ReadyInterest, ReadySet,
            SelectWaitOutcome, Selectable, StreamError, StreamOps,
        },
        handle::{AccessMode, HandleMetadata},
    },
    sync::{Mutex, ReadinessRegistration, Waker, waker::WaitResult},
    task::{Task, mytask},
};

const FRAGMENT_BYTES: usize = 4096;
const SYSLOG_ACTION_CLOSE: i32 = 0;
const SYSLOG_ACTION_OPEN: i32 = 1;
const SYSLOG_ACTION_READ: i32 = 2;
const SYSLOG_ACTION_READ_ALL: i32 = 3;
const SYSLOG_ACTION_SIZE_UNREAD: i32 = 9;
const SYSLOG_ACTION_SIZE_BUFFER: i32 = 10;

static SYSLOG_READER: LogReader = LogReader::new(0);

fn read_denied(task: &Task) -> bool {
    task.execution_restrictions.read().kernel_addresses_hidden()
}

fn access_mode(flags: i32) -> Result<AccessMode, usize> {
    match flags & 3 {
        0 => Ok(AccessMode::ReadOnly),
        1 => Ok(AccessMode::WriteOnly),
        2 => Ok(AccessMode::ReadWrite),
        _ => Err(errno::EINVAL),
    }
}

fn check_access(mode: AccessMode, permissions: FilePermission) -> Result<(), usize> {
    if (mode != AccessMode::WriteOnly && !permissions.read)
        || (mode != AccessMode::ReadOnly && !permissions.write)
    {
        Err(errno::EACCES)
    } else {
        Ok(())
    }
}

pub(super) fn path_metadata(path: &str) -> Option<FileMetadata> {
    (path.trim_end_matches('/') == "/proc/kmsg").then_some(FileMetadata {
        file_type: FileType::RegularFile,
        size: 0,
        permissions: FilePermission {
            read: true,
            write: false,
            execute: false,
        },
        created_time: 0,
        modified_time: 0,
        accessed_time: 0,
        file_id: 0x6b6d_7368,
        link_count: 1,
    })
}

pub(super) fn open(abi: &mut LinuxAbi, task: &Task, path: &str, flags: i32) -> Option<usize> {
    let metadata = path_metadata(path)?;
    let result = (|| {
        if flags & O_DIRECTORY != 0 {
            return Err(errno::ENOTDIR);
        }
        if flags & O_PATH != 0 {
            return Err(errno::EOPNOTSUPP);
        }
        let access = access_mode(flags)?;
        check_access(access, metadata.permissions)?;
        if read_denied(task) {
            return Err(errno::EPERM);
        }
        let object = KernelObject::File(Arc::new(LinuxLogFile {
            native: None,
            reader: LogReader::new(0),
            access,
            nonblocking: AtomicBool::new(flags & O_NONBLOCK != 0),
        }));
        let handle = task
            .handle_table
            .insert_with_metadata(
                object,
                HandleMetadata {
                    access_mode: access,
                    ..HandleMetadata::default()
                },
            )
            .map_err(|_| errno::ENFILE)?;
        let fd = match abi.allocate_fd(handle) {
            Ok(fd) => fd,
            Err(_) => {
                let _ = task.handle_table.remove(handle);
                return Err(errno::EMFILE);
            }
        };
        let _ = abi.set_file_status_flags(fd, (flags & (3 | O_NONBLOCK)) as u32);
        if flags & O_CLOEXEC != 0 {
            let _ = abi.set_fd_flags(fd, FD_CLOEXEC);
        }
        Ok(fd)
    })();
    Some(result.unwrap_or_else(errno::to_result))
}

/// Called only after the normal VFS open/access and mount checks. Identity is
/// the registered native device type, never a pathname or log message prefix.
/// Keep the opened object alive and preserve its metadata.
pub(super) fn wrap_device(object: KernelObject, flags: i32) -> Result<KernelObject, usize> {
    let Some(file) = object.as_file() else {
        return Ok(object);
    };
    let Some(vfs) = file
        .as_any()
        .downcast_ref::<crate::fs::vfs_v2::core::VfsFileObject>()
    else {
        return Ok(object);
    };
    if !vfs
        .inner()
        .as_any()
        .is::<crate::fs::vfs_v2::drivers::devfs::DevFileObject>()
    {
        return Ok(object);
    }
    let metadata = vfs.metadata().map_err(|_| errno::EIO)?;
    let FileType::CharDevice(info) = metadata.file_type else {
        return Ok(object);
    };
    if info.device_type != crate::device::DeviceType::Char {
        return Ok(object);
    }
    let Some(device) =
        crate::device::manager::DeviceManager::get_manager().get_device(info.device_id)
    else {
        return Ok(object);
    };
    if !device
        .as_any()
        .is::<crate::drivers::special::kmsg::KmsgDevice>()
    {
        return Ok(object);
    }
    if flags & O_PATH != 0 {
        return Err(errno::EOPNOTSUPP);
    }
    let access = access_mode(flags)?;
    check_access(access, metadata.permissions)?;
    let KernelObject::File(native) = object else {
        unreachable!()
    };
    Ok(KernelObject::File(Arc::new(LinuxLogFile {
        native: Some(native),
        reader: LogReader::new(0),
        access,
        nonblocking: AtomicBool::new(flags & O_NONBLOCK != 0),
    })))
}

/// Only the Linux write boundary interprets a priority prefix. Malformed
/// prefixes remain literal message bytes. Native stored text is never parsed.
fn priority_and_body(message: &[u8]) -> (u8, &[u8]) {
    if message.first() == Some(&b'<') {
        let mut priority = 0u16;
        for (index, &byte) in message.iter().enumerate().skip(1) {
            if byte == b'>' {
                if index > 1 && priority <= 191 {
                    return ((priority & 7) as u8, &message[index + 1..]);
                }
                break;
            }
            if !byte.is_ascii_digit() {
                break;
            }
            priority = priority
                .saturating_mul(10)
                .saturating_add((byte - b'0') as u16);
        }
    }
    (6, message)
}

fn submit(message: &[u8]) -> Result<usize, usize> {
    if message.len() > crate::log::MAX_USER_MESSAGE_BYTES {
        return Err(errno::EINVAL);
    }
    if message.is_empty() {
        return Ok(0);
    }
    let (severity, body) = priority_and_body(message);
    crate::log::submit_userspace_with_level(body, severity)
        .map(|_| message.len())
        .map_err(|error| match error {
            crate::log::UserspaceLogError::NoCurrentTask => errno::EPERM,
            crate::log::UserspaceLogError::InvalidArgument
            | crate::log::UserspaceLogError::TooLarge => errno::EINVAL,
        })
}

struct Fragment {
    bytes: Vec<u8>,
    next: usize,
    continuation: Option<u8>,
}

fn fragment(
    cursor: usize,
    end: usize,
    text: bool,
    continuation: Option<u8>,
) -> Result<Fragment, usize> {
    let mut raw = [0u8; FRAGMENT_BYTES];
    let (start, native_next, length, priority) = if text {
        // Bytes and metadata are copied under one native lock. The snapshot
        // stays valid after ring eviction and already ends at a metadata change.
        let record =
            crate::log::read_record_with_metadata(cursor, &mut raw).ok_or(errno::EAGAIN)?;
        let priority = match record.origin {
            crate::log::Origin::Userspace { .. } => record
                .severity
                .filter(|level| *level <= 7)
                .map(|level| 8 + level)
                .ok_or(errno::EIO)?,
            // Native printk has no severity metadata. This legacy text adapter
            // uses INFO as its documented default, never parses message text.
            crate::log::Origin::Kernel => 6,
        };
        (record.start, record.next, record.length, Some(priority))
    } else {
        let (length, next) = crate::log::read_record_at(cursor, &mut raw);
        (next.saturating_sub(length), next, length, None)
    };
    let length = length.min(end.saturating_sub(start));
    if length == 0 {
        return Err(errno::EAGAIN);
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(length + 5)
        .map_err(|_| errno::ENOMEM)?;
    let mut next_continuation = None;
    if let Some(priority) = priority {
        let continuation = continuation.filter(|_| start == cursor);
        if continuation != Some(priority) {
            if continuation.is_some() {
                output.push(b'\n');
            }
            output.push(b'<');
            if priority >= 10 {
                output.push(b'0' + priority / 10);
            }
            output.push(b'0' + priority % 10);
            output.push(b'>');
        }
        if raw[length - 1] != b'\n' {
            next_continuation = Some(priority);
        }
    }
    output.extend_from_slice(&raw[..length]);
    Ok(Fragment {
        bytes: output,
        next: native_next.min(end),
        continuation: next_continuation,
    })
}

struct ReadState {
    cursor: usize,
    continuation: Option<u8>,
    pending: Option<Fragment>,
    offset: usize,
}

impl ReadState {
    const fn new(cursor: usize) -> Self {
        Self {
            cursor,
            continuation: None,
            pending: None,
            offset: 0,
        }
    }

    fn copy_next(
        &mut self,
        count: usize,
        text: bool,
        copy: impl FnOnce(&[u8]) -> Result<(), usize>,
    ) -> Result<usize, usize> {
        if count == 0 {
            return Ok(0);
        }
        if self.pending.is_none() {
            self.pending = Some(fragment(
                self.cursor,
                crate::log::head(),
                text,
                self.continuation,
            )?);
        }
        let pending = self.pending.as_ref().unwrap();
        let length = count.min(pending.bytes.len() - self.offset);
        // Copy before committing. EFAULT retains both the cursor and the
        // staged bytes, even if the native ring subsequently overwrites them.
        copy(&pending.bytes[self.offset..self.offset + length])?;
        self.offset += length;
        if self.offset == pending.bytes.len() {
            self.cursor = pending.next;
            self.continuation = pending.continuation;
            self.pending = None;
            self.offset = 0;
        }
        Ok(length)
    }
}

struct LogReader {
    state: Mutex<ReadState>,
    // Readiness cannot take a sleepable mutex. Publication precedes wake_all.
    cursor: AtomicUsize,
    pending: AtomicBool,
}

impl LogReader {
    const fn new(cursor: usize) -> Self {
        Self {
            state: Mutex::new(ReadState::new(cursor)),
            cursor: AtomicUsize::new(cursor),
            pending: AtomicBool::new(false),
        }
    }

    fn ready(&self) -> bool {
        self.pending.load(Ordering::Acquire)
            || crate::log::has_data(self.cursor.load(Ordering::Acquire))
    }

    fn copy_next(
        &self,
        count: usize,
        text: bool,
        copy: impl FnOnce(&[u8]) -> Result<(), usize>,
    ) -> Result<usize, usize> {
        let mut state = self.state.lock();
        let result = state.copy_next(count, text, copy);
        self.pending
            .store(state.pending.is_some(), Ordering::Release);
        self.cursor.store(state.cursor, Ordering::Release);
        drop(state);
        if self.ready() {
            crate::log::READER_WAKER.wake_all();
        }
        result
    }

    fn read_with(
        &self,
        task: &Task,
        frame: &mut Trapframe,
        count: usize,
        text: bool,
        nonblocking: bool,
        mut copy: impl FnMut(&[u8]) -> Result<(), usize>,
    ) -> Result<usize, usize> {
        loop {
            if read_denied(task) {
                return Err(errno::EPERM);
            }
            match self.copy_next(count, text, &mut copy) {
                Err(errno::EAGAIN) if !nonblocking => {
                    if crate::log::READER_WAKER.wait_with_condition(
                        task.get_id(),
                        frame,
                        None,
                        0,
                        || read_denied(task) || self.ready(),
                    ) == WaitResult::Interrupted
                    {
                        return Err(errno::EINTR);
                    }
                }
                result => return result,
            }
        }
    }

    fn unread_len(&self) -> Result<usize, usize> {
        // Serialize with consuming reads while sampling a fixed native head.
        let state = self.state.lock();
        let (mut length, mut cursor, mut continuation) = {
            match &state.pending {
                Some(pending) => (
                    pending.bytes.len() - state.offset,
                    pending.next,
                    pending.continuation,
                ),
                None => (0, state.cursor, state.continuation),
            }
        };
        let end = crate::log::head();
        while cursor.max(crate::log::tail()) < end {
            let next = fragment(cursor, end, true, continuation)?;
            length += next.bytes.len();
            cursor = next.next;
            continuation = next.continuation;
        }
        drop(state);
        Ok(length)
    }
}

struct LinuxLogFile {
    native: Option<Arc<dyn FileObject>>,
    reader: LogReader,
    access: AccessMode,
    nonblocking: AtomicBool,
}

impl LinuxLogFile {
    fn text(&self) -> bool {
        self.native.is_none()
    }
    fn reader(&self) -> &LogReader {
        if self.text() {
            &SYSLOG_READER
        } else {
            &self.reader
        }
    }
    fn check_read(&self) -> Result<(), usize> {
        if self.access == AccessMode::WriteOnly {
            Err(errno::EBADF)
        } else {
            Ok(())
        }
    }
    fn check_write(&self) -> Result<(), usize> {
        if self.text() || self.access == AccessMode::ReadOnly {
            Err(errno::EBADF)
        } else {
            Ok(())
        }
    }
    fn read_with(
        &self,
        task: &Task,
        frame: &mut Trapframe,
        count: usize,
        copy: impl FnMut(&[u8]) -> Result<(), usize>,
    ) -> Result<usize, usize> {
        self.check_read()?;
        self.reader()
            .read_with(task, frame, count, self.text(), self.is_nonblocking(), copy)
    }
}

fn from_object(object: &KernelObject) -> Option<&LinuxLogFile> {
    object.as_file()?.as_any().downcast_ref::<LinuxLogFile>()
}

pub(super) fn is_log_file(object: &KernelObject) -> bool {
    from_object(object).is_some()
}

pub(super) fn read_to_user(
    object: &KernelObject,
    task: &Task,
    frame: &mut Trapframe,
    address: usize,
    count: usize,
) -> Option<usize> {
    let file = from_object(object)?;
    Some(
        file.read_with(task, frame, count, |bytes| {
            copy_to_user(task, address, bytes).map_err(|_| errno::EFAULT)
        })
        .unwrap_or_else(errno::to_result),
    )
}

pub(super) fn readv_to_user(
    object: &KernelObject,
    task: &Task,
    frame: &mut Trapframe,
    vectors: &[(usize, usize)],
    count: usize,
) -> Option<usize> {
    let file = from_object(object)?;
    Some(
        file.read_with(task, frame, count, |bytes| {
            let mut offset = 0;
            for &(address, length) in vectors {
                let length = length.min(bytes.len() - offset);
                copy_to_user(task, address, &bytes[offset..offset + length])
                    .map_err(|_| errno::EFAULT)?;
                offset += length;
                if offset == bytes.len() {
                    break;
                }
            }
            Ok(())
        })
        .unwrap_or_else(errno::to_result),
    )
}

pub(super) fn write_from_user(
    object: &KernelObject,
    task: &Task,
    address: usize,
    count: usize,
) -> Option<usize> {
    let file = from_object(object)?;
    Some(
        (|| {
            file.check_write()?;
            if count > crate::log::MAX_USER_MESSAGE_BYTES {
                return Err(errno::EINVAL);
            }
            let mut bytes = [0u8; crate::log::MAX_USER_MESSAGE_BYTES];
            copy_from_user(task, address, &mut bytes[..count]).map_err(|_| errno::EFAULT)?;
            submit(&bytes[..count])
        })()
        .unwrap_or_else(errno::to_result),
    )
}

pub(super) fn writev_from_user(
    object: &KernelObject,
    task: &Task,
    vectors: &[(usize, usize)],
    count: usize,
) -> Option<usize> {
    let file = from_object(object)?;
    Some(
        (|| {
            file.check_write()?;
            if count > crate::log::MAX_USER_MESSAGE_BYTES {
                return Err(errno::EINVAL);
            }
            let mut bytes = [0u8; crate::log::MAX_USER_MESSAGE_BYTES];
            let mut offset = 0;
            for &(address, length) in vectors {
                copy_from_user(task, address, &mut bytes[offset..offset + length])
                    .map_err(|_| errno::EFAULT)?;
                offset += length;
            }
            submit(&bytes[..count])
        })()
        .unwrap_or_else(errno::to_result),
    )
}

fn stream_error(error: usize) -> StreamError {
    match error {
        errno::EAGAIN => StreamError::WouldBlock,
        errno::EINTR => StreamError::Interrupted,
        errno::EBADF | errno::EPERM => StreamError::PermissionDenied,
        errno::EINVAL => StreamError::InvalidArgument,
        errno::ENOMEM => StreamError::NoSpace,
        _ => StreamError::IoError,
    }
}

impl StreamOps for LinuxLogFile {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let task = mytask().ok_or(StreamError::PermissionDenied)?;
        self.read_with(&task, task.get_trapframe(), buffer.len(), |bytes| {
            buffer[..bytes.len()].copy_from_slice(bytes);
            Ok(())
        })
        .map_err(stream_error)
    }
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        self.check_write()
            .and_then(|_| submit(buffer))
            .map_err(stream_error)
    }
}

impl Selectable for LinuxLogFile {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let mut ready = ReadySet::none();
        ready.read = interest.read
            && self.access != AccessMode::WriteOnly
            && (mytask().is_some_and(|task| read_denied(&task)) || self.reader().ready());
        ready.write = interest.write && self.check_write().is_ok();
        ready
    }
    fn subscribe_readiness(
        &self,
        interest: ReadyInterest,
        waiter: &Arc<Waker>,
    ) -> Option<Vec<ReadinessRegistration>> {
        Some(if interest.read && self.access != AccessMode::WriteOnly {
            vec![crate::log::READER_WAKER.subscribe_readiness(waiter)]
        } else {
            Vec::new()
        })
    }
    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        frame: &mut Trapframe,
        timeout: Option<u64>,
        minimum: u64,
    ) -> SelectWaitOutcome {
        let Some(task) = mytask() else {
            return SelectWaitOutcome::Interrupted;
        };
        match crate::log::READER_WAKER.wait_with_condition(
            task.get_id(),
            frame,
            timeout,
            minimum,
            || {
                let ready = self.current_ready(interest);
                ready.read || ready.write || ready.except
            },
        ) {
            WaitResult::Woken => SelectWaitOutcome::Ready,
            WaitResult::Interrupted => SelectWaitOutcome::Interrupted,
            WaitResult::TimedOut => SelectWaitOutcome::TimedOut,
        }
    }
    fn set_nonblocking(&self, enabled: bool) {
        self.nonblocking.store(enabled, Ordering::Release);
    }
    fn is_nonblocking(&self) -> bool {
        self.nonblocking.load(Ordering::Acquire)
    }
}

impl ControlOps for LinuxLogFile {}
impl MemoryMappingOps for LinuxLogFile {
    fn get_mapping_info(&self, _: usize, _: usize) -> Result<MemoryMappingInfo, &'static str> {
        Err("cannot map a kernel log stream")
    }
    fn supports_mmap(&self) -> bool {
        false
    }
}
impl FileObject for LinuxLogFile {
    fn allows_execution(&self) -> bool {
        false
    }
    fn allows_execution_privilege_gain(&self) -> bool {
        false
    }
    fn seek(&self, _: SeekFrom) -> Result<u64, StreamError> {
        Err(StreamError::SeekError)
    }
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        match &self.native {
            Some(native) => native.metadata(),
            None => Ok(path_metadata("/proc/kmsg").unwrap()),
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn read_all(cursor: usize, end: usize, limit: usize) -> Result<VecDeque<u8>, usize> {
    let limit = limit.min(crate::log::capacity());
    let mut output = VecDeque::new();
    output.try_reserve_exact(limit).map_err(|_| errno::ENOMEM)?;
    if limit == 0 {
        return Ok(output);
    }
    let mut cursor = cursor;
    let mut continuation = None;
    while cursor.max(crate::log::tail()) < end {
        let next = fragment(cursor, end, true, continuation)?;
        let bytes = &next.bytes[next.bytes.len().saturating_sub(limit)..];
        let excess = (output.len() + bytes.len()).saturating_sub(limit);
        output.drain(..excess);
        output.extend(bytes);
        cursor = next.next;
        continuation = next.continuation;
    }
    Ok(output)
}

fn syslog_for_task(
    task: &Task,
    frame: &mut Trapframe,
    action: i32,
    address: usize,
    count: i32,
) -> Result<usize, usize> {
    match action {
        // Clear has no Linux-local watermark yet. Console control has no native
        // implementation. Never clear the native ring or fake either operation.
        4..=8 => return Err(errno::EOPNOTSUPP),
        0..=3 | 9..=10 => {}
        _ => return Err(errno::EINVAL),
    }
    if read_denied(task) {
        return Err(errno::EPERM);
    }
    match action {
        SYSLOG_ACTION_CLOSE | SYSLOG_ACTION_OPEN => Ok(0),
        SYSLOG_ACTION_READ | SYSLOG_ACTION_READ_ALL if count < 0 => Err(errno::EINVAL),
        SYSLOG_ACTION_READ => {
            SYSLOG_READER.read_with(task, frame, count as usize, true, false, |bytes| {
                copy_to_user(task, address, bytes).map_err(|_| errno::EFAULT)
            })
        }
        SYSLOG_ACTION_READ_ALL => {
            let mut bytes = read_all(crate::log::tail(), crate::log::head(), count as usize)?;
            copy_to_user(task, address, bytes.make_contiguous()).map_err(|_| errno::EFAULT)?;
            Ok(bytes.len())
        }
        SYSLOG_ACTION_SIZE_UNREAD => SYSLOG_READER.unread_len(),
        SYSLOG_ACTION_SIZE_BUFFER => Ok(crate::log::capacity()),
        _ => Err(errno::EINVAL),
    }
}

pub(super) fn sys_syslog(_abi: &mut LinuxAbi, frame: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let action = frame.get_arg(0) as i32;
    let address = frame.get_arg(1);
    let count = frame.get_arg(2) as i32;
    frame.increment_pc_next(&task);
    syslog_for_task(&task, frame, action, address, count).unwrap_or_else(errno::to_result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        environment::{PAGE_SIZE, USER_STACK_END},
        fs::vfs_v2::{drivers::devfs::DevFS, manager::VfsManager},
        task::{TaskType, clear_mock_current_task, set_mock_current_task},
    };

    fn user_task() -> Arc<Task> {
        let mut task = Task::new("linux-log-test".into(), 1, TaskType::User);
        task.set_id(usize::MAX - 10);
        let task = Arc::new(task);
        task.allocate_stack_pages(USER_STACK_END - PAGE_SIZE * 2, 2)
            .unwrap();
        set_mock_current_task(task.clone());
        task
    }

    fn frame(task: &Task) -> Trapframe {
        let code = USER_STACK_END - 2 * PAGE_SIZE;
        copy_to_user(task, code, &0x73u32.to_ne_bytes()).unwrap();
        let mut frame = Trapframe::new();
        frame.set_pc(code as u64);
        frame
    }

    fn reset_reader(reader: &LogReader, cursor: usize) {
        *reader.state.lock() = ReadState::new(cursor);
        reader.cursor.store(cursor, Ordering::Release);
        reader.pending.store(false, Ordering::Release);
    }

    fn read_bytes(reader: &LogReader, count: usize, text: bool) -> Result<Vec<u8>, usize> {
        let mut output = Vec::new();
        reader.copy_next(count, text, |bytes| {
            output.extend_from_slice(bytes);
            Ok(())
        })?;
        Ok(output)
    }

    #[test_case]
    fn priority_is_decimal_and_comes_from_metadata_not_native_text() {
        let start = crate::log::head();
        crate::log::write_bytes(b"<0>userspace[pid=1] literal kernel text\n");
        crate::log::submit_userspace_for_test(42, 4, b"<0>literal user body\nsecond line").unwrap();
        let reader = LogReader::new(start);
        assert_eq!(
            read_bytes(&reader, 256, true).unwrap(),
            b"<6><0>userspace[pid=1] literal kernel text\n"
        );
        assert_eq!(
            read_bytes(&reader, 256, true).unwrap(),
            b"<12>userspace[pid=42] <0>literal user body\n"
        );
        assert_eq!(
            read_bytes(&reader, 256, true).unwrap(),
            b"<12>second line\n"
        );
        // A native fragment without a newline must not classify the following
        // user bytes as kernel, even when both fit in one native line read.
        let start = crate::log::head();
        crate::log::write_bytes(b"partial kernel text");
        crate::log::submit_userspace_for_test(42, 0, b"user emergency").unwrap();
        let reader = LogReader::new(start);
        assert_eq!(
            read_bytes(&reader, 256, true).unwrap(),
            b"<6>partial kernel text"
        );
        assert_eq!(
            read_bytes(&reader, 256, true).unwrap(),
            b"\n<8>userspace[pid=42] user emergency\n"
        );
    }

    #[test_case]
    fn short_reads_keep_remainder_and_unread_size_and_raw_format() {
        let start = crate::log::head();
        crate::log::submit_userspace_for_test(8, 6, b"small-buffer").unwrap();
        let reader = LogReader::new(start);
        let expected = b"<14>userspace[pid=8] small-buffer\n";
        assert_eq!(reader.unread_len(), Ok(expected.len()));
        let mut output = Vec::new();
        for remaining in (1..=expected.len()).rev() {
            assert_eq!(reader.unread_len(), Ok(remaining));
            output.extend(read_bytes(&reader, 1, true).unwrap());
        }
        assert_eq!(&output, expected);
        assert_eq!(reader.unread_len(), Ok(0));
        assert_eq!(read_bytes(&reader, 1, true), Err(errno::EAGAIN));
        let raw = LogReader::new(start);
        assert_eq!(
            read_bytes(&raw, 128, false).unwrap(),
            b"userspace[pid=8] small-buffer\n"
        );
        // READ_ALL returns the newest bytes, independently of consuming reads.
        let mut recent = read_all(start, crate::log::head(), 7).unwrap();
        assert_eq!(recent.make_contiguous(), &expected[expected.len() - 7..]);
        assert_eq!(reader.unread_len(), Ok(0));
    }

    #[test_case]
    fn long_native_line_does_not_get_extra_priority_at_staging_boundary() {
        let start = crate::log::head();
        crate::log::write_bytes(&[b'x'; FRAGMENT_BYTES + 8]);
        crate::log::write_bytes(b"\n");
        let reader = LogReader::new(start);
        let first = read_bytes(&reader, FRAGMENT_BYTES + 5, true).unwrap();
        assert_eq!(first.len(), FRAGMENT_BYTES + 3);
        assert_eq!(&first[..3], b"<6>");
        assert_eq!(read_bytes(&reader, 128, true).unwrap(), b"xxxxxxxx\n");
    }

    #[test_case]
    fn malformed_priority_is_preserved_and_submission_reports_original_length() {
        let task = user_task();
        for message in [
            b"<>bad".as_slice(),
            b"<-1>bad",
            b"<192>bad",
            b"<9999999999>bad",
            b"<4x>bad",
            b"<4bad",
        ] {
            assert_eq!(priority_and_body(message), (6, message));
            let start = crate::log::head();
            assert_eq!(submit(message), Ok(message.len()));
            let mut bytes = [0u8; 128];
            let (length, _) = crate::log::read_record_at(start, &mut bytes);
            assert!(bytes[..length].ends_with(&[message, b"\n"].concat()));
            assert_eq!(crate::log::severity_at(start), Some(6));
        }
        let start = crate::log::head();
        assert_eq!(submit(b"<3>valid"), Ok(8));
        assert_eq!(crate::log::severity_at(start), Some(3));
        assert_eq!(
            crate::log::origin_at(start),
            Some(crate::log::Origin::Userspace {
                pid: Some(task.get_thread_group_id())
            })
        );
        let head = crate::log::head();
        assert_eq!(submit(b"<6>"), Err(errno::EINVAL));
        assert_eq!(
            submit(&[b'x'; crate::log::MAX_USER_MESSAGE_BYTES + 1]),
            Err(errno::EINVAL)
        );
        assert_eq!(crate::log::head(), head);
        clear_mock_current_task();
    }

    #[test_case]
    fn proc_and_syslog_share_cursor_and_faults_do_not_consume_bytes() {
        let task = user_task();
        let start = crate::log::head();
        reset_reader(&SYSLOG_READER, start);
        crate::log::submit_userspace_for_test(9, 7, b"shared-reader").unwrap();
        let expected = b"<15>userspace[pid=9] shared-reader\n";
        let mut abi = LinuxAbi::default();
        let fd = super::super::proc_text::open_proc_text_file(
            &mut abi,
            &task,
            "/proc/kmsg",
            O_NONBLOCK | O_CLOEXEC,
        )
        .unwrap();
        assert_eq!(abi.get_fd_flags(fd), Some(FD_CLOEXEC));
        let mut tf = frame(&task);
        let buffer = USER_STACK_END - PAGE_SIZE - 2; // crosses two mapped pages
        tf.set_arg(0, fd);
        tf.set_arg(1, USER_STACK_END - 2); // first page writable, next unmapped
        tf.set_arg(2, expected.len());
        assert_eq!(
            super::super::fs::sys_read(&mut abi, &mut tf),
            errno::to_result(errno::EFAULT)
        );
        assert_eq!(SYSLOG_READER.state.lock().cursor, start);
        assert_eq!(SYSLOG_READER.unread_len(), Ok(expected.len()));
        tf.set_arg(1, buffer);
        tf.set_arg(2, 2);
        assert_eq!(super::super::fs::sys_read(&mut abi, &mut tf), 2);
        let mut got = [0u8; 2];
        copy_from_user(&task, buffer, &mut got).unwrap();
        assert_eq!(&got, &expected[..2]);
        assert_eq!(
            syslog_for_task(&task, &mut tf, 9, 0, 0),
            Ok(expected.len() - 2)
        );
        assert_eq!(syslog_for_task(&task, &mut tf, 2, 0, 3), Err(errno::EFAULT));
        assert_eq!(SYSLOG_READER.unread_len(), Ok(expected.len() - 2));
        assert_eq!(syslog_for_task(&task, &mut tf, 2, buffer, 3), Ok(3));
        let mut got = [0u8; 3];
        copy_from_user(&task, buffer, &mut got).unwrap();
        assert_eq!(&got, &expected[2..5]);
        // A second /proc open continues the same cursor instead of replaying.
        let other = open(&mut abi, &task, "/proc/kmsg", O_NONBLOCK).unwrap();
        tf.set_arg(0, other);
        tf.set_arg(1, buffer);
        tf.set_arg(2, 128);
        assert_eq!(
            super::super::fs::sys_read(&mut abi, &mut tf),
            expected.len() - 5
        );
        assert_eq!(
            super::super::fs::sys_read(&mut abi, &mut tf),
            errno::to_result(errno::EAGAIN)
        );
        assert_eq!(SYSLOG_READER.unread_len(), Ok(0));
        clear_mock_current_task();
    }

    #[test_case]
    fn access_modes_and_unsupported_actions_preserve_native_ring_and_boundary() {
        let task = user_task();
        for mode in [
            AccessMode::ReadOnly,
            AccessMode::WriteOnly,
            AccessMode::ReadWrite,
        ] {
            for read in [false, true] {
                for write in [false, true] {
                    let allowed = (mode == AccessMode::WriteOnly || read)
                        && (mode == AccessMode::ReadOnly || write);
                    assert_eq!(
                        check_access(
                            mode,
                            FilePermission {
                                read,
                                write,
                                execute: false
                            }
                        )
                        .is_ok(),
                        allowed
                    );
                }
            }
        }
        assert_eq!(access_mode(3), Err(errno::EINVAL));
        let mut abi = LinuxAbi::default();
        assert_eq!(
            open(&mut abi, &task, "/proc/kmsg", 1),
            Some(errno::to_result(errno::EACCES))
        );
        assert_eq!(
            open(&mut abi, &task, "/proc/kmsg", O_DIRECTORY),
            Some(errno::to_result(errno::ENOTDIR))
        );
        let mut tf = frame(&task);
        let tail = crate::log::tail();
        for action in 4..=8 {
            assert_eq!(
                syslog_for_task(&task, &mut tf, action, 0, 0),
                Err(errno::EOPNOTSUPP)
            );
        }
        assert_eq!(crate::log::tail(), tail);
        assert_eq!(
            syslog_for_task(&task, &mut tf, 11, 0, 0),
            Err(errno::EINVAL)
        );
        assert_eq!(
            syslog_for_task(&task, &mut tf, 2, 0, -1),
            Err(errno::EINVAL)
        );
        assert_eq!(
            syslog_for_task(&task, &mut tf, 10, 0, 0),
            Ok(crate::log::capacity())
        );
        crate::shield::information::constrain(
            &task,
            crate::shield::information::HIDE_KERNEL_ADDRESSES,
        )
        .unwrap();
        for action in [0, 1, 2, 3, 9, 10] {
            assert_eq!(
                syslog_for_task(&task, &mut tf, action, 0, 0),
                Err(errno::EPERM)
            );
        }
        assert_eq!(
            open(&mut abi, &task, "/proc/kmsg", 0),
            Some(errno::to_result(errno::EPERM))
        );
        // The existing read boundary does not forbid posting diagnostics.
        assert_eq!(submit(b"restricted writer"), Ok(17));
        clear_mock_current_task();
    }

    #[test_case]
    fn native_device_identity_writev_usercopy_and_nonblocking_subscriptions() {
        let task = user_task();
        let vfs = VfsManager::new();
        vfs.create_dir("/dev").unwrap();
        vfs.create_file("/dev/kmsg", FileType::RegularFile).unwrap();
        let replacement = wrap_device(vfs.open("/dev/kmsg", 0).unwrap(), 0).unwrap();
        assert!(!is_log_file(&replacement));
        let manager = crate::device::manager::DeviceManager::get_manager();
        let id = manager.register_device_with_name(
            "linux-log-test-kmsg".into(),
            Arc::new(crate::drivers::special::kmsg::KmsgDevice),
        );
        vfs.mount(DevFS::new(), "/dev", 0).unwrap();
        let native = vfs
            .open("/dev/linux-log-test-kmsg", scarlet_abi::fs::VFS_O_RDWR)
            .unwrap();
        let object = wrap_device(native, 2 | O_NONBLOCK).unwrap();
        let file = from_object(&object).unwrap();
        reset_reader(file.reader(), crate::log::head());
        let waiter = Arc::new(Waker::new_interruptible("linux-log-ready-test"));
        let registrations = file
            .subscribe_readiness(ReadyInterest::read(), &waiter)
            .unwrap();
        let before = registrations[0].generation();
        assert!(!file.current_ready(ReadyInterest::read()).read);
        let address = USER_STACK_END - PAGE_SIZE - 1;
        copy_to_user(&task, address, b"<4>writev-body").unwrap();
        let start = crate::log::head();
        assert_eq!(
            write_from_user(&object, &task, 0, 1),
            Some(errno::to_result(errno::EFAULT))
        );
        assert_eq!(
            writev_from_user(&object, &task, &[(address, 2), (0, 12)], 14),
            Some(errno::to_result(errno::EFAULT))
        );
        assert_eq!(crate::log::head(), start);
        assert_eq!(
            writev_from_user(&object, &task, &[(address, 2), (address + 2, 12)], 14),
            Some(14)
        );
        assert_ne!(registrations[0].generation(), before);
        assert!(file.current_ready(ReadyInterest::read()).read);
        let mut tf = frame(&task);
        assert_eq!(
            file.wait_until_ready(ReadyInterest::read(), &mut tf, Some(0), 0),
            SelectWaitOutcome::Ready
        );
        assert_eq!(crate::log::severity_at(start), Some(4));
        // readv must retain the cursor when a later iovec faults.
        assert_eq!(
            readv_to_user(&object, &task, &mut tf, &[(address, 2), (0, 64)], 66),
            Some(errno::to_result(errno::EFAULT))
        );
        assert_eq!(file.reader().state.lock().cursor, start);
        let mut output = [0u8; 128];
        let length = read_to_user(&object, &task, &mut tf, address, output.len()).unwrap();
        assert!(length <= output.len());
        copy_from_user(&task, address, &mut output[..length]).unwrap();
        assert!(output[..length].starts_with(b"userspace[pid="));
        assert!(output[..length].ends_with(b"] writev-body\n"));
        assert_eq!(
            read_to_user(&object, &task, &mut tf, address, output.len()),
            Some(errno::to_result(errno::EAGAIN))
        );
        let readonly = wrap_device(vfs.open("/dev/linux-log-test-kmsg", 0).unwrap(), 0).unwrap();
        assert_eq!(
            write_from_user(&readonly, &task, address, 1),
            Some(errno::to_result(errno::EBADF))
        );
        let writeonly = wrap_device(
            vfs.open("/dev/linux-log-test-kmsg", scarlet_abi::fs::VFS_O_WRONLY)
                .unwrap(),
            1,
        )
        .unwrap();
        assert_eq!(
            read_to_user(&writeonly, &task, &mut tf, address, 1),
            Some(errno::to_result(errno::EBADF))
        );
        crate::shield::information::constrain(
            &task,
            crate::shield::information::HIDE_KERNEL_ADDRESSES,
        )
        .unwrap();
        assert_eq!(
            read_to_user(&object, &task, &mut tf, address, 1),
            Some(errno::to_result(errno::EPERM))
        );
        drop(registrations);
        drop(object);
        drop(vfs);
        manager.unregister_device(id);
        clear_mock_current_task();
    }

    #[test_case]
    fn waiting_reader_rechecks_published_data_after_registration() {
        use crate::sched::scheduler::{
            add_task, get_task_by_id, register_online_cpu, remove_from_ready_queues, reset,
            set_current_task_for_test,
        };
        use crate::task::TaskState;
        reset();
        let cpu = crate::arch::get_cpu().get_cpuid();
        register_online_cpu(cpu);
        let task_id = add_task(Task::new("linux-log-wait".into(), 1, TaskType::Kernel), cpu);
        let task = get_task_by_id(task_id).unwrap();
        task.set_state(TaskState::Running);
        task.running_cpu.store(cpu, Ordering::SeqCst);
        set_current_task_for_test(cpu, Some(task_id));
        remove_from_ready_queues(task_id);
        let reader = LogReader::new(crate::log::head());
        let waiter = Waker::new_interruptible("linux-log-registration-test");
        let mut checks = 0;
        // Deliberately do not forward the native wake to this waiter: model
        // another observer consuming the broadcast before registration. Only
        // the post-registration level query can prevent sleeping here.
        let result = waiter.wait_with_condition(task_id, &mut Trapframe::new(), None, 0, || {
            checks += 1;
            if checks == 1 {
                assert!(!reader.ready());
                crate::log::submit_userspace_for_test(1, 6, b"between-query-and-wait").unwrap();
                false
            } else {
                assert_eq!(waiter.waiting_count(), 1);
                reader.ready()
            }
        });
        assert_eq!(result, WaitResult::Woken);
        assert_eq!(checks, 2);
        assert_eq!(waiter.waiting_count(), 0);
        assert_eq!(task.get_state(), TaskState::Running);
        set_current_task_for_test(cpu, None);
        task.running_cpu.store(usize::MAX, Ordering::SeqCst);
        drop(task);
        reset();
    }
}
