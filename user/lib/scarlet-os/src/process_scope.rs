//! Explicit process lifecycle control, independent of Environment and namespaces.
use crate::{
    Handle,
    handle::{HandleError, HandleResult},
};
use scarlet_sys::{Syscall, syscall0, syscall1, syscall2};
#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct Status {
    /// Zero while admitting processes, one after irreversible termination starts.
    pub closing: u32,
    pub live: u32,
    /// Includes exited members awaiting task reclamation.
    pub members: u32,
    pub reserved: u32,
}
pub struct ProcessScope(Handle);
fn result(raw: usize) -> HandleResult<()> {
    if raw == 0 {
        Ok(())
    } else {
        Err(HandleError::SystemError(-1))
    }
}
impl ProcessScope {
    pub fn create() -> HandleResult<Self> {
        // SAFETY: scalar query; the result is a new close-on-exec capability.
        let raw = unsafe { syscall0(Syscall::ProcessScopeCreate) };
        if raw == usize::MAX {
            return Err(HandleError::SystemError(-1));
        }
        // SAFETY: successful construction returned an owned native handle.
        unsafe { Handle::from_raw(raw as i32) }.map(Self)
    }
    pub fn from_handle(handle: Handle) -> HandleResult<Self> {
        if handle.object_info().object_type
            != crate::handle::introspection::KernelObjectType::ProcessScope
        {
            return Err(HandleError::InvalidParameter);
        }
        Ok(Self(handle))
    }
    pub fn as_handle(&self) -> &Handle {
        &self.0
    }
    /// Enroll this exclusive process once; membership is retained after this handle is closed.
    pub fn enter(&self) -> HandleResult<()> {
        // SAFETY: a borrowed capability; the kernel validates current-process enrollment.
        result(unsafe { syscall1(Syscall::ProcessScopeEnter, self.0.as_raw() as usize) })
    }
    /// Select a live member, or use zero to explicitly nominate this caller.
    pub fn set_reaper(&self, pid: u32) -> HandleResult<()> {
        // SAFETY: the kernel resolves the namespace-local PID and checks membership/consent.
        result(unsafe {
            syscall2(
                Syscall::ProcessScopeSetReaper,
                self.0.as_raw() as usize,
                pid as usize,
            )
        })
    }
    /// Stop admission and request fatal termination of all members. Errors may be retried.
    pub fn terminate(&self) -> HandleResult<()> {
        // SAFETY: control authority is carried by this capability.
        result(unsafe { syscall1(Syscall::ProcessScopeTerminate, self.0.as_raw() as usize) })
    }
    pub fn status(&self) -> HandleResult<Status> {
        let mut status = Status::default();
        // SAFETY: the fixed 16-byte output remains exclusive for the synchronous copy.
        result(unsafe {
            syscall2(
                Syscall::ProcessScopeStatus,
                self.0.as_raw() as usize,
                &mut status as *mut Status as usize,
            )
        })?;
        Ok(status)
    }
}
