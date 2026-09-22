//! Nonblocking advisory whole-file locks shared across VFS views.
//!
//! Lock ownership follows a VFS open description, including dup and fork.
//! The registry contains scalar identities, never a reference that could keep
//! an open description alive. Its only lock protects those scalar records;
//! allocation, destruction, handle lookup and filesystem calls occur outside
//! that lock. Blocking acquisition is deliberately unsupported.

use alloc::vec::Vec;
use scarlet_abi::fs::ERRNO_ENOMEM;
use scarlet_abi::{ERRNO_EAGAIN, ERRNO_EBADF, ERRNO_EINVAL, ERRNO_EIO, ERRNO_EOPNOTSUPP};

use super::core::VfsFileObject;
use crate::arch::Trapframe;
use crate::fs::FileType;
use crate::object::{KernelObject, handle::HandleTable};
use crate::sync::{IrqSpinLock, sequence::IdSequence};
use crate::task::mytask;

const LOCK_SH: usize = 1;
const LOCK_EX: usize = 2;
const LOCK_NB: usize = 4;
const LOCK_UN: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileKey {
    filesystem: u64,
    inode: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Shared,
    Exclusive,
}

#[derive(Clone, Copy)]
struct Record {
    key: FileKey,
    owner: u64,
    mode: Mode,
}

static LOCKS: IrqSpinLock<Vec<Record>> = IrqSpinLock::new(Vec::new());

/// Unique owner whose lifetime is exactly that of its VfsFileObject.
pub(crate) struct LockOwner(u64);

impl LockOwner {
    pub(crate) fn new() -> Self {
        static OWNERS: IdSequence = IdSequence::new();
        Self(
            OWNERS
                .reserve()
                .expect("File lock owner identities exhausted")
                .get(),
        )
    }

    fn unlock(&self) {
        let mut locks = LOCKS.lock();
        if let Some(index) = locks.iter().position(|record| record.owner == self.0) {
            locks.swap_remove(index);
        }
    }

    fn acquire(&self, key: FileKey, mode: Mode) -> Result<(), i32> {
        loop {
            let mut locks = LOCKS.lock();
            // Test contention before changing our existing record: a failed
            // upgrade must keep its shared lock continuously held.
            if locks.iter().any(|record| {
                record.key == key
                    && record.owner != self.0
                    && (record.mode == Mode::Exclusive || mode == Mode::Exclusive)
            }) {
                return Err(ERRNO_EAGAIN);
            }
            if let Some(record) = locks.iter_mut().find(|record| record.owner == self.0) {
                debug_assert!(record.key == key);
                record.mode = mode;
                return Ok(());
            }
            if locks.len() < locks.capacity() {
                locks.push(Record {
                    key,
                    owner: self.0,
                    mode,
                });
                return Ok(());
            }
            // Do not enter the heap allocator while holding the global lock.
            // Another CPU may change the registry during allocation, so check
            // capacity again and retry the complete contention test afterwards.
            let needed = locks.len().checked_add(1).ok_or(ERRNO_ENOMEM)?;
            let capacity = locks.len().checked_mul(2).unwrap_or(needed).max(needed);
            drop(locks);
            let mut grown = Vec::new();
            grown.try_reserve(capacity).map_err(|_| ERRNO_ENOMEM)?;
            let mut locks = LOCKS.lock();
            if grown.capacity() > locks.capacity() && grown.capacity() > locks.len() {
                for record in locks.iter().copied() {
                    grown.push(record); // Capacity was checked above: no allocation.
                }
                core::mem::swap(&mut *locks, &mut grown);
            }
            drop(locks);
            drop(grown); // Free the replaced storage only after releasing the lock.
        }
    }
}

impl Drop for LockOwner {
    fn drop(&mut self) {
        self.unlock();
    }
}

fn operation(value: usize) -> Result<Option<Mode>, i32> {
    if value & !(LOCK_SH | LOCK_EX | LOCK_NB | LOCK_UN) != 0 {
        return Err(ERRNO_EINVAL);
    }
    let mode = match value & !LOCK_NB {
        LOCK_SH => Some(Mode::Shared),
        LOCK_EX => Some(Mode::Exclusive),
        LOCK_UN => None,
        _ => return Err(ERRNO_EINVAL),
    };
    if mode.is_some() && value & LOCK_NB == 0 {
        return Err(ERRNO_EOPNOTSUPP);
    }
    Ok(mode)
}

fn lock_object(object: &KernelObject, value: usize) -> Result<(), i32> {
    let mode = operation(value)?;
    let file: &VfsFileObject = object
        .as_file()
        .and_then(|file| file.as_any().downcast_ref())
        .ok_or(ERRNO_EOPNOTSUPP)?;
    let node = file.get_vfs_entry().node();
    if node.file_type().map_err(|_| ERRNO_EIO)? != FileType::RegularFile {
        return Err(ERRNO_EOPNOTSUPP);
    }
    let filesystem = node
        .filesystem()
        .and_then(|fs| fs.upgrade())
        .ok_or(ERRNO_EIO)?;
    if !filesystem.supports_advisory_locks() {
        return Err(ERRNO_EOPNOTSUPP);
    }
    let Some(mode) = mode else {
        file.lock_owner().unlock();
        return Ok(());
    };
    let key = FileKey {
        filesystem: filesystem.fs_id().get(),
        inode: node.id(),
    };
    file.lock_owner().acquire(key, mode)
}

fn lock_handle(table: &HandleTable, handle: usize, value: usize) -> Result<(), i32> {
    let object = u32::try_from(handle)
        .ok()
        .and_then(|handle| table.get_arc_clone(handle))
        .ok_or(ERRNO_EBADF)?;
    // Advisory locks do not require a writable descriptor.
    lock_object(&object, value)
}

pub fn sys_file_lock(tf: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let (handle, value) = (tf.get_arg(0), tf.get_arg(1));
    tf.increment_pc_next(&task);
    match lock_handle(&task.handle_table, handle, value) {
        Ok(()) => 0,
        Err(errno) => (-(errno as isize)) as usize,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::vfs_v2::VfsManager;
    use crate::object::handle::{AccessMode, HandleMetadata, HandleType};
    use alloc::sync::Arc;

    fn file(vfs: &VfsManager, path: &str) -> KernelObject {
        vfs.open_at(None, path, scarlet_abi::fs::VFS_O_CREAT, 0o600)
            .unwrap()
    }

    #[test_case]
    fn flock_shared_owners_coexist_and_exclusive_contends() {
        let vfs = VfsManager::new();
        let a = file(&vfs, "/file");
        let b = file(&vfs, "/file");
        let c = file(&vfs, "/file");
        assert_eq!(lock_object(&a, LOCK_SH | LOCK_NB), Ok(()));
        assert_eq!(lock_object(&b, LOCK_SH | LOCK_NB), Ok(()));
        assert_eq!(lock_object(&c, LOCK_EX | LOCK_NB), Err(ERRNO_EAGAIN));
        assert_eq!(lock_object(&a, LOCK_UN), Ok(()));
        assert_eq!(lock_object(&c, LOCK_EX | LOCK_NB), Err(ERRNO_EAGAIN));
        assert_eq!(lock_object(&b, LOCK_UN | LOCK_NB), Ok(()));
        assert_eq!(lock_object(&c, LOCK_EX | LOCK_NB), Ok(()));
        assert_eq!(lock_object(&a, LOCK_SH | LOCK_NB), Err(ERRNO_EAGAIN));
        assert_eq!(lock_object(&b, LOCK_EX | LOCK_NB), Err(ERRNO_EAGAIN));
        assert_eq!(lock_object(&c, LOCK_EX | LOCK_NB), Ok(()));
    }

    #[test_case]
    fn flock_failed_upgrade_preserves_lock_and_downgrade_is_atomic() {
        let vfs = VfsManager::new();
        let a = file(&vfs, "/file");
        let b = file(&vfs, "/file");
        lock_object(&a, LOCK_SH | LOCK_NB).unwrap();
        lock_object(&b, LOCK_SH | LOCK_NB).unwrap();
        assert_eq!(lock_object(&a, LOCK_EX | LOCK_NB), Err(ERRNO_EAGAIN));
        lock_object(&b, LOCK_UN).unwrap();
        assert_eq!(lock_object(&b, LOCK_EX | LOCK_NB), Err(ERRNO_EAGAIN));
        lock_object(&a, LOCK_EX | LOCK_NB).unwrap();
        lock_object(&a, LOCK_SH | LOCK_NB).unwrap();
        lock_object(&b, LOCK_SH | LOCK_NB).unwrap();
    }

    #[test_case]
    fn flock_dup_close_and_process_table_drop_release_only_last_owner() {
        let vfs = VfsManager::new();
        let table = HandleTable::new();
        let fd = table
            .insert_with_metadata(
                file(&vfs, "/file"),
                HandleMetadata {
                    handle_type: HandleType::Regular,
                    access_mode: AccessMode::ReadOnly,
                    special_semantics: None,
                },
            )
            .unwrap();
        let other = file(&vfs, "/file");
        lock_handle(&table, fd as usize, LOCK_EX | LOCK_NB).unwrap();
        let (duplicate, metadata) = table.clone_for_dup(fd).unwrap();
        let dupfd = table.insert_with_metadata(duplicate, metadata).unwrap();
        drop(table.remove(fd).unwrap());
        assert_eq!(lock_object(&other, LOCK_EX | LOCK_NB), Err(ERRNO_EAGAIN));
        lock_handle(&table, dupfd as usize, LOCK_UN).unwrap();
        lock_object(&other, LOCK_EX | LOCK_NB).unwrap();
        lock_object(&other, LOCK_UN).unwrap();
        lock_handle(&table, dupfd as usize, LOCK_EX | LOCK_NB).unwrap();
        let forked = table.deep_clone();
        drop(table);
        assert_eq!(lock_object(&other, LOCK_EX | LOCK_NB), Err(ERRNO_EAGAIN));
        drop(forked);
        assert_eq!(lock_object(&other, LOCK_EX | LOCK_NB), Ok(()));
    }

    #[test_case]
    fn flock_hardlinks_symlinks_and_cross_vfs_bind_mounts_share_identity() {
        let source = Arc::new(VfsManager::new());
        source.create_dir("/dir").unwrap();
        let first = file(&source, "/dir/file");
        source
            .create_hardlink("/dir/file", "/dir/hardlink")
            .unwrap();
        source.create_symlink("/symlink", "/dir/file").unwrap();
        let target = VfsManager::new();
        target.create_dir("/mount").unwrap();
        target.bind_mount_from(&source, "/dir", "/mount").unwrap();
        lock_object(&first, LOCK_EX | LOCK_NB).unwrap();
        for other in [
            file(&source, "/dir/hardlink"),
            file(&source, "/symlink"),
            file(&target, "/mount/file"),
        ] {
            assert_eq!(lock_object(&other, LOCK_SH | LOCK_NB), Err(ERRNO_EAGAIN));
            assert_eq!(lock_object(&other, LOCK_EX | LOCK_NB), Err(ERRNO_EAGAIN));
        }
        let distinct_filesystem = file(&target, "/other");
        lock_object(&distinct_filesystem, LOCK_EX | LOCK_NB).unwrap();
    }

    #[test_case]
    fn flock_same_inode_numbers_on_distinct_filesystems_do_not_contend() {
        let source = VfsManager::new();
        let target = VfsManager::new();
        let a = file(&source, "/file");
        let b = file(&target, "/file");
        assert_eq!(
            a.as_file().unwrap().metadata().unwrap().file_id,
            b.as_file().unwrap().metadata().unwrap().file_id
        );
        lock_object(&a, LOCK_EX | LOCK_NB).unwrap();
        lock_object(&b, LOCK_EX | LOCK_NB).unwrap();
    }

    #[test_case]
    fn flock_overlay_path_identities_cannot_bypass_alias_locks() {
        use crate::fs::vfs_v2::drivers::overlayfs::OverlayFS;
        use alloc::{string::ToString, vec};

        let source = VfsManager::new();
        let a = file(&source, "/a");
        source.create_hardlink("/a", "/b").unwrap();
        let (entry, mount) = source.resolve_path("/").unwrap();
        let overlay = OverlayFS::new(None, vec![(mount, entry)], "locks".to_string()).unwrap();
        let vfs = VfsManager::new_with_root(overlay);
        lock_object(&a, LOCK_EX | LOCK_NB).unwrap();
        // Overlay currently assigns IDs by path. Report unsupported rather
        // than claiming that distinct aliases hold independent exclusive locks.
        for path in ["/a", "/b"] {
            let object = vfs.open_at(None, path, 0, 0).unwrap();
            assert_eq!(
                lock_object(&object, LOCK_EX | LOCK_NB),
                Err(ERRNO_EOPNOTSUPP)
            );
            assert_eq!(lock_object(&object, LOCK_UN), Err(ERRNO_EOPNOTSUPP));
        }
    }

    #[test_case]
    fn flock_tmpfs_unlink_recreate_retains_distinct_live_inode_locks() {
        let vfs = VfsManager::new();
        let old = file(&vfs, "/file");
        lock_object(&old, LOCK_EX | LOCK_NB).unwrap();
        vfs.remove("/file").unwrap();
        let new = file(&vfs, "/file");
        assert_ne!(
            old.as_file().unwrap().metadata().unwrap().file_id,
            new.as_file().unwrap().metadata().unwrap().file_id
        );
        lock_object(&new, LOCK_EX | LOCK_NB).unwrap();
        let new_again = file(&vfs, "/file");
        assert_eq!(
            lock_object(&new_again, LOCK_SH | LOCK_NB),
            Err(ERRNO_EAGAIN)
        );
        drop(old);
        assert_eq!(
            lock_object(&new_again, LOCK_SH | LOCK_NB),
            Err(ERRNO_EAGAIN)
        );
        drop(new);
        lock_object(&new_again, LOCK_EX | LOCK_NB).unwrap();
    }

    #[test_case]
    fn flock_rejects_invalid_and_blocking_modes_without_changing_lock() {
        let vfs = VfsManager::new();
        let a = file(&vfs, "/file");
        let b = file(&vfs, "/file");
        lock_object(&a, LOCK_EX | LOCK_NB).unwrap();
        for value in [
            0,
            LOCK_NB,
            LOCK_SH | LOCK_EX | LOCK_NB,
            LOCK_UN | LOCK_SH,
            LOCK_UN | LOCK_EX,
            16,
            usize::MAX,
        ] {
            assert_eq!(lock_object(&a, value), Err(ERRNO_EINVAL));
        }
        for value in [LOCK_SH, LOCK_EX] {
            assert_eq!(lock_object(&a, value), Err(ERRNO_EOPNOTSUPP));
            assert_eq!(lock_object(&b, value), Err(ERRNO_EOPNOTSUPP));
        }
        assert_eq!(lock_object(&b, LOCK_SH | LOCK_NB), Err(ERRNO_EAGAIN));
        lock_object(&a, LOCK_UN).unwrap();
        lock_object(&a, LOCK_UN).unwrap();
        lock_object(&b, LOCK_EX | LOCK_NB).unwrap();
        let dir = vfs
            .open_at(None, "/", scarlet_abi::fs::VFS_O_DIRECTORY, 0)
            .unwrap();
        assert_eq!(lock_object(&dir, LOCK_SH | LOCK_NB), Err(ERRNO_EOPNOTSUPP));
        assert_eq!(lock_object(&dir, LOCK_UN), Err(ERRNO_EOPNOTSUPP));
        assert_eq!(
            lock_handle(&HandleTable::new(), 42, LOCK_UN),
            Err(ERRNO_EBADF)
        );
        assert_eq!(
            lock_handle(&HandleTable::new(), usize::MAX, LOCK_EX | LOCK_NB),
            Err(ERRNO_EBADF)
        );
    }

    #[test_case]
    fn flock_drops_registry_records_and_handles_growth() {
        let vfs = VfsManager::new();
        let first = file(&vfs, "/file");
        let owner = first
            .as_file()
            .unwrap()
            .as_any()
            .downcast_ref::<VfsFileObject>()
            .unwrap()
            .lock_owner()
            .0;
        lock_object(&first, LOCK_SH | LOCK_NB).unwrap();
        let mut files = Vec::new();
        for _ in 0..40 {
            let object = file(&vfs, "/file");
            lock_object(&object, LOCK_SH | LOCK_NB).unwrap();
            files.push(object);
        }
        drop(first);
        assert!(!LOCKS.lock().iter().any(|record| record.owner == owner));
        let other = file(&vfs, "/file");
        assert_eq!(lock_object(&other, LOCK_EX | LOCK_NB), Err(ERRNO_EAGAIN));
        drop(files);
        assert_eq!(lock_object(&other, LOCK_EX | LOCK_NB), Ok(()));
        lock_object(&other, LOCK_UN).unwrap();
        let fsid = vfs
            .resolve_path("/file")
            .unwrap()
            .0
            .node()
            .filesystem()
            .unwrap()
            .upgrade()
            .unwrap()
            .fs_id()
            .get();
        assert!(
            !LOCKS
                .lock()
                .iter()
                .any(|record| record.key.filesystem == fsid)
        );
    }
}
