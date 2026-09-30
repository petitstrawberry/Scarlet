//! Process-owned POSIX byte-range locks for APT/dpkg and Wine.
//! Sleeping acquisition on contention is explicitly unsupported. These
//! locks are independent of BSD flock's open-description-owned registry.

use super::{LinuxAbi, errno};
use crate::{
    object::KernelObject,
    sync::{IrqSpinLock, sequence::IdSequence},
};
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct FileKey(u64, u64);

#[derive(Clone, Copy)]
struct Record {
    key: FileKey,
    owner: u64,
    pid: i32,
    kind: i16,
    range: Range,
}

/// Half-open file offsets. TOP includes the last representable off_t byte;
/// zero length in struct flock means all remaining bytes, including future data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Range {
    start: u64,
    end: u64,
}
impl Range {
    const TOP: u64 = 1 << 63;

    fn new(base: i64, start: i64, len: i64) -> Result<Self, usize> {
        let start = base.checked_add(start).ok_or(errno::EOVERFLOW)?;
        let (start, end) = if len < 0 {
            (
                start.checked_add(len).ok_or(errno::EOVERFLOW)?,
                start as u64,
            )
        } else if len == 0 {
            (start, Self::TOP)
        } else {
            let end = (start as i128) + (len as i128);
            if end > Self::TOP as i128 {
                return Err(errno::EOVERFLOW);
            }
            (start, end as u64)
        };
        if start < 0 || end > Self::TOP || start as u64 >= end {
            return Err(errno::EINVAL);
        }
        Ok(Self {
            start: start as u64,
            end,
        })
    }

    fn overlaps(self, other: Self) -> bool {
        self.start < other.end && other.start < self.end
    }
}

static LOCKS: IrqSpinLock<Vec<Record>> = IrqSpinLock::new(Vec::new());

pub(super) fn file_key(object: &KernelObject) -> Option<FileKey> {
    let file = object
        .as_file()?
        .as_any()
        .downcast_ref::<crate::fs::vfs_v2::core::VfsFileObject>()?;
    let node = file.get_vfs_entry().node();
    if node.file_type().ok()? != crate::fs::FileType::RegularFile {
        return None;
    }
    let filesystem = node.filesystem()?.upgrade()?;
    // Path-derived overlay inode IDs cannot guarantee alias exclusion.
    if !filesystem.supports_advisory_locks() {
        return None;
    }
    Some(FileKey(filesystem.fs_id().get(), node.id()))
}

pub(super) struct ProcessLocks(u64);

impl ProcessLocks {
    pub(super) fn new() -> Self {
        static OWNERS: IdSequence = IdSequence::new();
        Self(
            OWNERS
                .reserve()
                .expect("POSIX lock owner identities exhausted")
                .get(),
        )
    }

    pub(super) fn unlock_file(&self, key: FileKey) {
        LOCKS.lock().retain(|r| r.owner != self.0 || r.key != key);
    }

    pub(super) fn unlock_all(&self) {
        LOCKS.lock().retain(|r| r.owner != self.0);
    }

    fn conflict(
        locks: &[Record],
        key: FileKey,
        owner: u64,
        kind: i16,
        range: Range,
    ) -> Option<Record> {
        locks
            .iter()
            .find(|r| {
                r.key == key
                    && r.owner != owner
                    && (kind == 1 || r.kind == 1)
                    && r.range.overlaps(range)
            })
            .copied()
    }

    fn update(
        &self,
        key: FileKey,
        pid: i32,
        kind: i16,
        range: Range,
        wait: bool,
    ) -> Result<(), usize> {
        loop {
            let mut locks = LOCKS.lock();
            if kind != 2 && Self::conflict(&locks, key, self.0, kind, range).is_some() {
                return Err(if wait {
                    errno::EOPNOTSUPP
                } else {
                    errno::EAGAIN
                });
            }
            // Replacing a subrange can split one existing interval and add a
            // new one. Owned intervals are disjoint, so two spare slots suffice.
            let needed = locks.len().checked_add(2).ok_or(errno::ENOMEM)?;
            if needed <= locks.capacity() {
                replace(&mut locks, key, self.0, pid, kind, range);
                return Ok(());
            }
            let capacity = locks.len().checked_mul(2).unwrap_or(needed).max(needed);
            drop(locks);
            // Never allocate or free while holding the global registry lock.
            let mut grown = Vec::new();
            grown.try_reserve(capacity).map_err(|_| errno::ENOMEM)?;
            let mut locks = LOCKS.lock();
            if grown.capacity() > locks.capacity() && grown.capacity() > locks.len() {
                grown.extend(locks.iter().copied());
                core::mem::swap(&mut *locks, &mut grown);
            }
            drop(locks);
            drop(grown);
        }
    }
}

/// Called only with enough reserved storage. No allocation under LOCKS.
fn replace(locks: &mut Vec<Record>, key: FileKey, owner: u64, pid: i32, kind: i16, range: Range) {
    let mut i = 0;
    while i < locks.len() {
        let record = locks[i];
        if record.key != key || record.owner != owner || !record.range.overlaps(range) {
            i += 1;
            continue;
        }
        let left = record.range.start < range.start;
        let right = range.end < record.range.end;
        match (left, right) {
            (true, true) => {
                locks[i].range.end = range.start;
                locks.push(Record {
                    range: Range {
                        start: range.end,
                        end: record.range.end,
                    },
                    ..record
                });
            }
            (true, false) => locks[i].range.end = range.start,
            (false, true) => locks[i].range.start = range.end,
            (false, false) => {
                locks.swap_remove(i);
                continue;
            }
        }
        i += 1;
    }
    if kind == 2 {
        return;
    }
    let mut added = Record {
        key,
        owner,
        pid,
        kind,
        range,
    };
    let mut i = 0;
    while i < locks.len() {
        let record = locks[i];
        if record.key == key
            && record.owner == owner
            && record.kind == kind
            && record.range.start <= added.range.end
            && added.range.start <= record.range.end
        {
            added.range.start = added.range.start.min(record.range.start);
            added.range.end = added.range.end.max(record.range.end);
            locks.swap_remove(i);
            // The expanded interval may now touch an earlier interval.
            i = 0;
        } else {
            i += 1;
        }
    }
    locks.push(added);
}

impl Drop for ProcessLocks {
    fn drop(&mut self) {
        self.unlock_all();
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxFlock {
    kind: i16,
    whence: i16,
    pad: u32,
    start: i64,
    len: i64,
    pid: i32,
    pad2: u32,
}

pub(super) fn fcntl(
    abi: &LinuxAbi,
    task: &crate::task::Task,
    fd: usize,
    cmd: u32,
    ptr: usize,
) -> usize {
    let result = (|| {
        let handle = abi.get_handle(fd).ok_or(errno::EBADF)?;
        let object = task.handle_table.get(handle).ok_or(errno::EBADF)?;
        let key = file_key(&object).ok_or(errno::EOPNOTSUPP)?;
        let mut bytes = [0; core::mem::size_of::<LinuxFlock>()];
        crate::library::std::usercopy::copy_from_user(task, ptr, &mut bytes)
            .map_err(|_| errno::EFAULT)?;
        let mut lock = unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast::<LinuxFlock>()) };
        if !matches!(lock.kind, 0..=2) {
            return Err(errno::EINVAL);
        }
        let file = object.as_file().ok_or(errno::EBADF)?;
        let base = match lock.whence {
            0 => 0,
            1 => i64::try_from(
                file.seek(crate::fs::SeekFrom::Current(0))
                    .map_err(|_| errno::EINVAL)?,
            )
            .map_err(|_| errno::EOVERFLOW)?,
            2 => i64::try_from(file.metadata().map_err(|_| errno::EIO)?.size)
                .map_err(|_| errno::EOVERFLOW)?,
            _ => return Err(errno::EINVAL),
        };
        let range = Range::new(base, lock.start, lock.len)?;
        if cmd == 5 {
            if lock.kind == 2 {
                return Err(errno::EINVAL);
            }
            let other =
                ProcessLocks::conflict(&LOCKS.lock(), key, abi.record_locks.0, lock.kind, range);
            if let Some(other) = other {
                lock.kind = other.kind;
                lock.pid = other.pid;
                lock.whence = 0;
                lock.start = other.range.start as i64;
                lock.len = if other.range.end == Range::TOP {
                    0
                } else {
                    (other.range.end - other.range.start) as i64
                };
            } else {
                lock.kind = 2; // F_UNLCK, remaining fields unchanged.
            }
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    (&lock as *const LinuxFlock).cast(),
                    core::mem::size_of::<LinuxFlock>(),
                )
            };
            crate::library::std::usercopy::copy_to_user(task, ptr, bytes)
                .map_err(|_| errno::EFAULT)?;
        } else {
            let access = abi.get_file_status_flags(fd).unwrap_or(0) & 3;
            if (lock.kind == 0 && access == 1) || (lock.kind == 1 && access == 0) {
                return Err(errno::EBADF);
            }
            abi.record_locks.update(
                key,
                abi.visible_thread_group_id(task) as i32,
                lock.kind,
                range,
                cmd == 7,
            )?;
        }
        Ok(())
    })();
    result.map(|_| 0).unwrap_or_else(errno::to_result)
}
