//! Resource membership authority, independent of lifecycle scopes and namespaces.
use crate::{
    Handle,
    handle::{HandleError, HandleResult},
};
use scarlet_sys::{Syscall, syscall0, syscall1, syscall2, syscall4};
pub struct ResourceGroup(Handle);
#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct Status {
    pub identity: u64,
    pub processes: u64,
    pub subtree_processes: u64,
    pub descendants: u64,
}
fn result(raw: usize) -> HandleResult<()> {
    if raw == 0 {
        Ok(())
    } else {
        Err(HandleError::SystemError(-1))
    }
}
impl ResourceGroup {
    fn returned(raw: usize) -> HandleResult<Self> {
        if raw == usize::MAX {
            return Err(HandleError::SystemError(-1));
        }
        // SAFETY: successful construction returned a new close-on-exec handle.
        unsafe { Handle::from_raw(raw as i32) }.map(Self)
    }
    pub fn create_root() -> HandleResult<Self> {
        // SAFETY: the kernel checks resource-management authority.
        Self::returned(unsafe { syscall2(Syscall::ResourceGroupCreate, usize::MAX, 0) })
    }
    pub fn current() -> HandleResult<Self> {
        // SAFETY: no pointer arguments.
        Self::returned(unsafe { syscall0(Syscall::ResourceGroupCurrent) })
    }
    pub fn create_child(&self, name: &str) -> HandleResult<Self> {
        let name =
            crate::ffi::str_to_cstr_bytes(name).map_err(|_| HandleError::InvalidParameter)?;
        // SAFETY: borrowed handle and name remain valid throughout the synchronous copy.
        Self::returned(unsafe {
            syscall2(
                Syscall::ResourceGroupCreate,
                self.0.as_raw() as usize,
                name.as_ptr() as usize,
            )
        })
    }
    /// Move this process (pid zero), or a visible process. Existing threads move together.
    pub fn enter(&self, pid: usize) -> HandleResult<()> {
        // SAFETY: the kernel checks authority, target visibility and lifecycle state.
        result(unsafe { syscall2(Syscall::ResourceGroupEnter, self.0.as_raw() as usize, pid) })
    }
    pub fn status(&self) -> HandleResult<Status> {
        let mut status = Status::default();
        // SAFETY: fixed 32-byte output is exclusive for the duration of the call.
        result(unsafe {
            syscall2(
                Syscall::ResourceGroupStatus,
                self.0.as_raw() as usize,
                &mut status as *mut Status as usize,
            )
        })?;
        Ok(status)
    }
    pub fn remove(&self) -> HandleResult<()> {
        // SAFETY: the kernel rejects roots and groups with children or live memberships.
        result(unsafe { syscall1(Syscall::ResourceGroupRemove, self.0.as_raw() as usize) })
    }
    /// Set a native CPU mask, or inherit the parent's constraint.
    pub fn set_cpus(&self, mask: Option<usize>) -> HandleResult<()> {
        // SAFETY: no pointers; authority and all live tasks are checked atomically.
        result(unsafe {
            syscall4(
                Syscall::ResourceGroupCpus,
                self.0.as_raw() as usize,
                if mask.is_some() { 1 } else { 2 },
                mask.unwrap_or(0),
                0,
            )
        })
    }
    /// Requested mask (zero means inherited) and effective online mask.
    pub fn cpus(&self) -> HandleResult<(u64, u64)> {
        let mut masks = [0u64; 2];
        // SAFETY: exclusive fixed-size output remains live throughout the copy.
        result(unsafe {
            syscall4(
                Syscall::ResourceGroupCpus,
                self.0.as_raw() as usize,
                0,
                0,
                masks.as_mut_ptr() as usize,
            )
        })?;
        Ok((masks[0], masks[1]))
    }
    /// Leave this independent hierarchy, retaining all other memberships.
    pub fn leave(&self) -> HandleResult<()> {
        // SAFETY: native capability and resource-management authority are checked.
        result(unsafe { syscall1(Syscall::ResourceGroupLeave, self.0.as_raw() as usize) })
    }
    pub fn as_handle(&self) -> &Handle {
        &self.0
    }
}
