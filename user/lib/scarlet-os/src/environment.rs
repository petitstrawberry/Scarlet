//! Execution Environments and capability-scoped filesystem views.
//!
//! The kernel does not select a rootfs layout. An administrator builds views,
//! registers one per ABI, seals the map, then explicitly spawns or execs an
//! already-open executable. Queries return use-only handles; they do not grant
//! mount authority. Management handles are close-on-exec by default.
//!
//! Environment isolation covers filesystem views and explicit handle transfer,
//! not PID, network, IPC, users, or a complete security sandbox.

extern crate alloc;
use crate::{
    Handle,
    ffi::str_to_cstr_bytes,
    handle::{HandleError, HandleResult},
};
use alloc::vec::Vec;
use core::convert::Infallible;
use scarlet_abi::{RawEnvironmentExec, RawEnvironmentHandleMapping};
use scarlet_sys::{Syscall, syscall0, syscall1, syscall2, syscall3, syscall4};

fn cstring(value: &str) -> HandleResult<Vec<u8>> {
    str_to_cstr_bytes(value).map_err(|_| HandleError::InvalidParameter)
}
fn result(value: usize) -> HandleResult<usize> {
    if value == usize::MAX {
        Err(HandleError::SystemError(-1))
    } else {
        Ok(value)
    }
}
fn handle(value: usize) -> HandleResult<Handle> {
    let raw = result(value)? as i32;
    // SAFETY: the successful syscall returned a new owned descriptor.
    unsafe { Handle::from_raw(raw) }
}

/// A filesystem root and mount topology, without a working directory.
#[derive(Debug)]
pub struct VfsView(Handle);
impl VfsView {
    /// Adopt a transferred view handle without changing its authority.
    pub fn from_handle(handle: Handle) -> HandleResult<Self> {
        if handle.object_info().object_type
            != crate::handle::introspection::KernelObjectType::VfsView
        {
            return Err(HandleError::InvalidParameter);
        }
        Ok(Self(handle))
    }
    /// Create a filesystem-backed view. Requires existing construction authority.
    pub fn create(fstype: &str, options: &str) -> HandleResult<Self> {
        let fstype = cstring(fstype)?;
        let options = cstring(options)?;
        // SAFETY: both C strings remain valid for the synchronous syscall.
        handle(unsafe {
            syscall2(
                Syscall::VfsViewCreate,
                fstype.as_ptr() as usize,
                options.as_ptr() as usize,
            )
        })
        .map(Self)
    }
    /// Query the active view without acquiring mount authority.
    pub fn current() -> HandleResult<Self> {
        // SAFETY: no pointer arguments.
        handle(unsafe { syscall1(Syscall::VfsViewCurrent, 0) }).map(Self)
    }
    /// Obtain the bootstrap grant, or duplicate existing current-view authority.
    pub fn current_admin() -> HandleResult<Self> {
        // SAFETY: the kernel checks authority; no pointer arguments.
        handle(unsafe { syscall1(Syscall::VfsViewCurrent, 1) }).map(Self)
    }
    pub fn as_handle(&self) -> &Handle {
        &self.0
    }
    fn raw(&self) -> usize {
        self.0.as_raw() as usize
    }
    /// Copy mount topology, sharing filesystem data; requires source administration.
    pub fn clone_mounts(&self) -> HandleResult<Self> {
        // SAFETY: the live handle is checked by the kernel.
        handle(unsafe { syscall1(Syscall::VfsViewClone, self.raw()) }).map(Self)
    }
    /// Make a new view rooted at a source directory, without recursive bind mounts.
    pub fn rooted_at(&self, path: &str) -> HandleResult<Self> {
        let path = cstring(path)?;
        // SAFETY: borrowed handle and C string are valid through the call.
        handle(unsafe { syscall2(Syscall::VfsViewRoot, self.raw(), path.as_ptr() as usize) })
            .map(Self)
    }
    /// Create a two-layer overlay root. Compose these to add more lower layers.
    pub fn overlay(
        lower: &Self,
        lower_path: &str,
        upper: Option<(&Self, &str)>,
    ) -> HandleResult<Self> {
        let lower_path = cstring(lower_path)?;
        let upper_path = cstring(upper.map_or("", |(_, path)| path))?;
        // SAFETY: source handles and strings remain live; absent upper uses -1.
        handle(unsafe {
            syscall4(
                Syscall::VfsViewOverlay,
                lower.raw(),
                lower_path.as_ptr() as usize,
                upper.map_or(usize::MAX, |(view, _)| view.raw()),
                upper_path.as_ptr() as usize,
            )
        })
        .map(Self)
    }
    /// Open with native VFS flags, marking the returned handle close-on-exec.
    /// Like VfsOpen, creation requires O_CREAT | O_EXCL; O_CREAT alone only opens.
    pub fn open(&self, path: &str, flags: u32) -> HandleResult<Handle> {
        let path = cstring(path)?;
        // SAFETY: the kernel validates open flags and the scoped path.
        handle(unsafe {
            syscall3(
                Syscall::VfsViewOpen,
                self.raw(),
                path.as_ptr() as usize,
                flags as usize,
            )
        })
    }
    pub fn create_directory(&self, path: &str) -> HandleResult<()> {
        let path = cstring(path)?;
        // SAFETY: the scoped path remains live for the call.
        result(unsafe {
            syscall2(
                Syscall::VfsViewCreateDirectory,
                self.raw(),
                path.as_ptr() as usize,
            )
        })
        .map(|_| ())
    }
    /// Mount below this view's root; replacing a root requires a new view.
    pub fn mount(&self, target: &str, fstype: &str, options: &str) -> HandleResult<()> {
        let target = cstring(target)?;
        let fstype = cstring(fstype)?;
        let options = cstring(options)?;
        // SAFETY: all strings and the capability are borrowed for the entire call.
        result(unsafe {
            syscall4(
                Syscall::VfsViewMount,
                self.raw(),
                target.as_ptr() as usize,
                fstype.as_ptr() as usize,
                options.as_ptr() as usize,
            )
        })
        .map(|_| ())
    }
    /// Non-recursive bind. Shared submounts must be bound explicitly.
    pub fn bind(&self, target: &str, source: &Self, source_path: &str) -> HandleResult<()> {
        let target = cstring(target)?;
        let source_path = cstring(source_path)?;
        // SAFETY: source/destination handles and C strings remain live.
        result(unsafe {
            syscall4(
                Syscall::VfsViewBind,
                self.raw(),
                target.as_ptr() as usize,
                source.raw(),
                source_path.as_ptr() as usize,
            )
        })
        .map(|_| ())
    }
    pub fn unmount(&self, path: &str) -> HandleResult<()> {
        let path = cstring(path)?;
        // SAFETY: the path remains live; the kernel checks management authority.
        result(unsafe { syscall2(Syscall::VfsViewUnmount, self.raw(), path.as_ptr() as usize) })
            .map(|_| ())
    }
}

/// An ABI-to-view map. Sealing prevents subsequent slot edits, not file writes.
#[derive(Debug)]
pub struct Environment(Handle);
/// A borrowed handle explicitly transferred into the new image.
pub struct HandleMapping<'a> {
    pub source: &'a Handle,
    pub target: u32,
}
impl Environment {
    /// Adopt a transferred Environment handle without changing its authority.
    pub fn from_handle(handle: Handle) -> HandleResult<Self> {
        if handle.object_info().object_type
            != crate::handle::introspection::KernelObjectType::Environment
        {
            return Err(HandleError::InvalidParameter);
        }
        Ok(Self(handle))
    }
    pub fn create() -> HandleResult<Self> {
        // SAFETY: no pointer arguments.
        handle(unsafe { syscall0(Syscall::EnvironmentCreate) }).map(Self)
    }
    pub fn current() -> HandleResult<Self> {
        // SAFETY: no pointer arguments; returns use-only authority.
        handle(unsafe { syscall0(Syscall::EnvironmentCurrent) }).map(Self)
    }
    pub fn as_handle(&self) -> &Handle {
        &self.0
    }
    fn raw(&self) -> usize {
        self.0.as_raw() as usize
    }
    pub fn set_root(&self, abi: &str, view: &VfsView) -> HandleResult<()> {
        let abi = cstring(abi)?;
        // SAFETY: the ABI string and both handles are live through the call.
        result(unsafe {
            syscall3(
                Syscall::EnvironmentSetRoot,
                self.raw(),
                abi.as_ptr() as usize,
                view.raw(),
            )
        })
        .map(|_| ())
    }
    pub fn remove_root(&self, abi: &str) -> HandleResult<()> {
        let abi = cstring(abi)?;
        // SAFETY: the ABI string is live through the call.
        result(unsafe {
            syscall2(
                Syscall::EnvironmentRemoveRoot,
                self.raw(),
                abi.as_ptr() as usize,
            )
        })
        .map(|_| ())
    }
    pub fn seal(&self) -> HandleResult<()> {
        // SAFETY: no pointer arguments.
        result(unsafe { syscall1(Syscall::EnvironmentSeal, self.raw()) }).map(|_| ())
    }
    pub fn root(&self, abi: &str) -> HandleResult<VfsView> {
        let abi = cstring(abi)?;
        // SAFETY: the ABI string is live; returned view has use-only authority.
        handle(unsafe {
            syscall2(
                Syscall::EnvironmentGetRoot,
                self.raw(),
                abi.as_ptr() as usize,
            )
        })
        .map(VfsView)
    }
    /// Spawn without inheriting ambient handles, including standard streams.
    pub fn spawn(
        &self,
        executable: &Handle,
        argv: &[&str],
        envp: &[&str],
        cwd: &str,
        handles: &[HandleMapping<'_>],
    ) -> HandleResult<u32> {
        self.execute(
            Syscall::EnvironmentSpawn,
            executable,
            argv,
            envp,
            cwd,
            handles,
            None,
        )
        .map(|pid| pid as u32)
    }
    /// Replace this single-threaded process. On error its old image remains live.
    /// Only explicitly mapped handles survive; cwd is absolute in the target view.
    pub fn exec(
        &self,
        executable: &Handle,
        argv: &[&str],
        envp: &[&str],
        cwd: &str,
        handles: &[HandleMapping<'_>],
    ) -> HandleResult<Infallible> {
        self.execute(
            Syscall::EnvironmentExec,
            executable,
            argv,
            envp,
            cwd,
            handles,
            None,
        )?;
        Err(HandleError::SystemError(-1)) // a successful exec never resumes here
    }
    /// Explicit ABI selection for images without an unambiguous ABI marker.
    /// The selected ABI must still have a slot in this sealed Environment.
    pub fn exec_with_abi(
        &self,
        abi: &str,
        executable: &Handle,
        argv: &[&str],
        envp: &[&str],
        cwd: &str,
        handles: &[HandleMapping<'_>],
    ) -> HandleResult<Infallible> {
        self.execute(
            Syscall::EnvironmentExec,
            executable,
            argv,
            envp,
            cwd,
            handles,
            Some(abi),
        )?;
        Err(HandleError::SystemError(-1))
    }
    /// Spawn with an explicit ABI and only the mapped handles.
    pub fn spawn_with_abi(
        &self,
        abi: &str,
        executable: &Handle,
        argv: &[&str],
        envp: &[&str],
        cwd: &str,
        handles: &[HandleMapping<'_>],
    ) -> HandleResult<u32> {
        self.execute(
            Syscall::EnvironmentSpawn,
            executable,
            argv,
            envp,
            cwd,
            handles,
            Some(abi),
        )
        .map(|pid| pid as u32)
    }
    fn execute(
        &self,
        syscall: Syscall,
        executable: &Handle,
        argv: &[&str],
        envp: &[&str],
        cwd: &str,
        handles: &[HandleMapping<'_>],
        abi: Option<&str>,
    ) -> HandleResult<usize> {
        if argv.len() > 256 || envp.len() > 256 || handles.len() > 1024 || !cwd.starts_with('/') {
            return Err(HandleError::InvalidParameter);
        }
        let argv: Vec<_> = argv
            .iter()
            .map(|s| cstring(s))
            .collect::<HandleResult<_>>()?;
        let envp: Vec<_> = envp
            .iter()
            .map(|s| cstring(s))
            .collect::<HandleResult<_>>()?;
        let mut argv_ptrs: Vec<_> = argv.iter().map(|s| s.as_ptr() as usize).collect();
        let mut envp_ptrs: Vec<_> = envp.iter().map(|s| s.as_ptr() as usize).collect();
        argv_ptrs.push(0);
        envp_ptrs.push(0);
        let cwd = cstring(cwd)?;
        let abi = abi.map(cstring).transpose()?;
        let handles: Vec<_> = handles
            .iter()
            .map(|h| RawEnvironmentHandleMapping {
                source: h.source.as_raw() as u32,
                target: h.target,
            })
            .collect();
        let raw = RawEnvironmentExec {
            size: core::mem::size_of::<RawEnvironmentExec>() as u32,
            flags: 0,
            argv: argv_ptrs.as_ptr() as usize,
            envp: envp_ptrs.as_ptr() as usize,
            cwd: cwd.as_ptr() as usize,
            handles: handles.as_ptr() as usize,
            handle_count: handles.len(),
        };
        // SAFETY: all records, arrays, C strings, and borrowed handles outlive
        // this synchronous call. The kernel copies inputs before replacing memory.
        result(unsafe {
            syscall4(
                syscall,
                self.raw(),
                executable.as_raw() as usize,
                &raw as *const _ as usize,
                abi.as_ref().map_or(0, |abi| abi.as_ptr() as usize),
            )
        })
    }
}
