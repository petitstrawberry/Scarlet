//! Immutable label policies and explicit subject assignment, independent of
//! Environment, filesystem views and namespaces. Enforcement points choose
//! their numeric classes and permission masks.
extern crate alloc;
use crate::{
    Handle,
    handle::{HandleError, HandleResult, introspection::KernelObjectType},
};
use alloc::vec::Vec;
use scarlet_abi::shield::{RawLabel, RawRule};
pub use scarlet_abi::shield::{RawRule as Rule, RawTransition as Transition};
use scarlet_sys::{Syscall, syscall0, syscall2, syscall4, syscall6};

pub struct Policy(Handle);
impl Policy {
    /// Requires bootstrap authority or an existing policy management handle.
    pub fn create(labels: &[(u32, &str)], rules: &[RawRule]) -> HandleResult<Self> {
        let raw: Vec<_> = labels
            .iter()
            .map(|(id, name)| RawLabel {
                id: *id,
                name_len: name.len() as u32,
                name: name.as_ptr() as u64,
            })
            .collect();
        if labels.iter().any(|(_, name)| name.len() > 255) {
            return Err(HandleError::InvalidParameter);
        }
        // SAFETY: all arrays and borrowed names live through this synchronous copy.
        let result = unsafe {
            syscall4(
                Syscall::ShieldPolicyCreate,
                raw.as_ptr() as usize,
                raw.len(),
                rules.as_ptr() as usize,
                rules.len(),
            )
        };
        Self::returned(result)
    }
    /// Create immutable, explicitly delegated transitions without giving child
    /// processes the management capability. Principal conditions are checked
    /// against all three native identity slots when a transition is requested.
    pub fn create_with_transitions(
        labels: &[(u32, &str)],
        rules: &[RawRule],
        transitions: &[Transition],
    ) -> HandleResult<Self> {
        if labels.iter().any(|(_, name)| name.len() > 255) {
            return Err(HandleError::InvalidParameter);
        }
        let raw: Vec<_> = labels
            .iter()
            .map(|(id, name)| RawLabel {
                id: *id,
                name_len: name.len() as u32,
                name: name.as_ptr() as u64,
            })
            .collect();
        // SAFETY: all borrowed arrays live through this synchronous kernel copy.
        Self::returned(unsafe {
            syscall6(
                Syscall::ShieldPolicyCreateWithTransitions,
                raw.as_ptr() as usize,
                raw.len(),
                rules.as_ptr() as usize,
                rules.len(),
                transitions.as_ptr() as usize,
                transitions.len(),
            )
        })
    }
    /// Apply only a transition explicitly delegated by this immutable policy.
    /// Query authority identifies the policy; it does not grant transitions.
    pub fn transition(&self, label: u32) -> HandleResult<()> {
        // SAFETY: the kernel checks the policy, source subject and identities.
        let result = unsafe {
            syscall2(
                Syscall::ShieldTransition,
                self.0.as_raw() as usize,
                label as usize,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(HandleError::InvalidParameter)
        }
    }
    /// Returns query authority only; arbitrary assignment remains forbidden.
    pub fn current() -> HandleResult<Self> {
        // SAFETY: no pointer arguments.
        Self::returned(unsafe { syscall0(Syscall::ShieldPolicyCurrent) })
    }
    fn returned(result: usize) -> HandleResult<Self> {
        if result == usize::MAX {
            return Err(HandleError::InvalidParameter);
        }
        // SAFETY: successful syscall returned an owned handle.
        unsafe { Handle::from_raw(result as i32) }.map(Self)
    }
    pub fn from_handle(handle: Handle) -> HandleResult<Self> {
        if handle.object_info().object_type != KernelObjectType::ShieldPolicy {
            return Err(HandleError::InvalidParameter);
        }
        Ok(Self(handle))
    }
    pub fn as_handle(&self) -> &Handle {
        &self.0
    }
    /// Assign the current process before launching children. Requires policy
    /// management authority and an exclusive single-threaded process. Clone,
    /// Environment spawn and exec preserve the assigned identity.
    pub fn apply(&self, label: u32) -> HandleResult<()> {
        // SAFETY: the kernel checks the live capability and label identifier.
        let result = unsafe {
            syscall2(
                Syscall::ShieldApply,
                self.0.as_raw() as usize,
                label as usize,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(HandleError::InvalidParameter)
        }
    }
}
