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
