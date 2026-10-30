//! Optional device translations supplied by an ABI that composes Linux.
//! Common path, descriptor and mmap validation remains in the Linux syscall
//! handlers. A plain Linux instance has no extensions; this module knows
//! nothing about any particular derived ABI or its devices.

use super::generic::LinuxAbi;
use crate::{arch::Trapframe, object::KernelObject, task::Task};

pub struct FileMapping {
    pub address: usize,
    pub length: usize,
    pub protection: usize,
    pub flags: usize,
    pub offset: usize,
}

pub trait LinuxAbiExtensions: Send + Sync {
    /// An outer ABI may translate its filesystem's native identity.
    fn filesystem_type(
        &self,
        _filesystem: &dyn crate::fs::vfs_v2::core::FileSystemOperations,
    ) -> Option<i64> {
        None
    }
    fn peer_security_context(&self, _task: &Task, _peer: &crate::shield::Subject)
        -> Option<Result<alloc::vec::Vec<u8>, usize>> { None }
    /// Translate a derived ABI's protected metadata after common path/fd checks.
    fn get_xattr(&self, _task: &Task, _node: &dyn crate::fs::vfs_v2::core::VfsNode, _name: &[u8])
        -> Option<Result<alloc::vec::Vec<u8>, usize>> { None }
    fn change_xattr(&self, _task: &Task, _node: &dyn crate::fs::vfs_v2::core::VfsNode, _name: &[u8], _value: Option<&[u8]>, _mode: crate::fs::vfs_v2::xattr::SetMode)
        -> Option<Result<(), usize>> { None }

    fn open(&self, _abi: &mut LinuxAbi, _task: &Task, _path: &str, _flags: i32) -> Option<usize> {
        None
    }

    /// Linux flush semantics run when a descriptor is closed or replaced,
    /// even while an in-flight operation or duplicate retains the file object.
    /// This notification does not revoke the underlying object.
    fn flush_on_close(&self, _task: &Task, _object: &KernelObject) {}

    fn ioctl(
        &self,
        _abi: &LinuxAbi,
        _task: &Task,
        _trapframe: &mut Trapframe,
        _object: &KernelObject,
        _fd: usize,
        _request: u32,
        _arg: usize,
    ) -> Option<usize> {
        None
    }

    fn mmap(&self, _task: &Task, _object: &KernelObject, _mapping: FileMapping) -> Option<usize> {
        None
    }
}

pub(super) struct NoExtensions;
impl LinuxAbiExtensions for NoExtensions {}
