//! Android ABI = Android-specific translations + a composed Linux ABI.
//! Binder translates to Scarlet's native RPC objects, which remain ABI-neutral.

#[cfg(target_arch = "aarch64")]
pub mod aarch64;
pub mod binder;
#[cfg(target_arch = "aarch64")]
mod detect;
mod security;

use crate::{
    abi::linux::{
        extensions::{FileMapping, LinuxAbiExtensions},
        generic::LinuxAbi,
    },
    arch::Trapframe,
    object::KernelObject,
    task::Task,
};

struct AndroidExtensions;

impl LinuxAbiExtensions for AndroidExtensions {
    fn peer_security_context(&self, task: &Task, peer: &crate::shield::Subject) -> Option<Result<alloc::vec::Vec<u8>, usize>> {
        let own = task.security_subject.read().clone();
        Some(if own.is_some_and(|own| alloc::sync::Arc::ptr_eq(own.policy(), peer.policy())) {
            Ok(alloc::format!("{}\0", peer.name()).into_bytes())
        } else { Err(crate::abi::linux::generic::errno::EACCES) })
    }
    fn get_xattr(&self, task: &Task, node: &dyn crate::fs::vfs_v2::core::VfsNode, name: &[u8])
        -> Option<Result<alloc::vec::Vec<u8>, usize>> {
        if name != b"security.selinux" { return None; }
        security::resource_context(task, node).map(Ok)
    }
    fn change_xattr(&self, task:&Task,node:&dyn crate::fs::vfs_v2::core::VfsNode,name:&[u8],value:Option<&[u8]>,mode:crate::fs::vfs_v2::xattr::SetMode)->Option<Result<(),usize>> {
        if name!=b"security.selinux" {return None;}
        let subject=task.security_subject.read().clone()?;
        if subject.policy().resource_label(node).is_none() {return None;}
        Some((|| {
            if mode==crate::fs::vfs_v2::xattr::SetMode::Create {return Err(crate::abi::linux::generic::errno::EEXIST);}
            let value=value.ok_or(crate::abi::linux::generic::errno::EACCES)?;
            let text=core::str::from_utf8(value).map_err(|_|crate::abi::linux::generic::errno::EINVAL)?;
            let text=text.strip_suffix('\0').unwrap_or(text);
            let target=subject.policy().label_id(text).ok_or(crate::abi::linux::generic::errno::EACCES)?;
            subject.policy().relabel_file(&subject,node,target).map_err(|_|crate::abi::linux::generic::errno::EACCES)
        })())
    }
    fn open(&self, abi: &mut LinuxAbi, task: &Task, path: &str, flags: i32) -> Option<usize> {
        security::open(abi, task, path, flags).or_else(|| binder::open(abi, task, path, flags))
    }

    fn filesystem_type(
        &self,
        filesystem: &dyn crate::fs::vfs_v2::core::FileSystemOperations,
    ) -> Option<i64> {
        security::filesystem_type(filesystem)
    }

    fn flush_on_close(&self, _task: &Task, object: &KernelObject) {
        if let Some(file) = object
            .as_file()
            .and_then(|file| file.as_any().downcast_ref::<binder::BinderFile>())
        {
            file.flush_on_close();
        }
    }

    fn ioctl(
        &self,
        abi: &LinuxAbi,
        task: &Task,
        trapframe: &mut Trapframe,
        object: &KernelObject,
        fd: usize,
        request: u32,
        arg: usize,
    ) -> Option<usize> {
        let file = object
            .as_file()?
            .as_any()
            .downcast_ref::<binder::BinderFile>()?;
        let nonblocking = abi.get_file_status_flags(fd).unwrap_or(0) & 0x800 != 0;
        Some(file.ioctl(task, abi, trapframe, request, arg, nonblocking))
    }

    fn mmap(&self, task: &Task, object: &KernelObject, mapping: FileMapping) -> Option<usize> {
        if let Some(result) = security::mmap(object, &mapping) {
            return Some(result);
        }
        let file = object
            .as_file()?
            .as_any()
            .downcast_ref::<binder::BinderFile>()?;
        Some(file.mmap(
            task,
            mapping.address,
            mapping.length,
            mapping.protection,
            mapping.flags,
            mapping.offset,
        ))
    }
}
