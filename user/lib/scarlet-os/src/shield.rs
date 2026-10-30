//! Versioned label policies and explicit subject assignment, independent of
//! Environment, filesystem views and namespaces. Enforcement points choose
//! their numeric classes and permission masks.
extern crate alloc;
use crate::{
    Handle,
    handle::{HandleError, HandleResult, introspection::KernelObjectType},
};
use alloc::vec::Vec;
use scarlet_abi::shield::{RawLabel, RawRule};
pub use scarlet_abi::shield::{RawRule as Rule, RawTransition as Transition, RawExecTransition as ExecTransition, RawFileCreate as FileCreate, RawFileRelabel as FileRelabel};
use scarlet_sys::{Syscall, syscall0, syscall1, syscall2, syscall4, syscall6};

pub struct Policy(Handle);
impl Policy {
    pub fn generation(&self) -> HandleResult<u64> {
        let mut generation = 0u64;
        // SAFETY: the output lives through the synchronous copy.
        let result = unsafe {
            syscall2(
                Syscall::ShieldPolicyGeneration,
                self.0.as_raw() as usize,
                &mut generation as *mut u64 as usize,
            )
        };
        if result == 0 {
            Ok(generation)
        } else {
            Err(HandleError::InvalidParameter)
        }
    }
    /// Requires the management capability. A subject's current-policy handle
    /// cannot publish updates. All borrowed records live through the copy.
    pub fn extend(
        &self,
        expected_generation: u64,
        labels: &[(u32, &str)],
        rules: &[Rule],
        transitions: &[Transition],
        exec: &[ExecTransition],
        create: &[FileCreate],
        relabel: &[FileRelabel],
    ) -> HandleResult<()> {
        if labels.len() > 512
            || labels
                .iter()
                .any(|(_, name)| name.is_empty() || name.len() > 255)
            || [
                rules.len(),
                transitions.len(),
                exec.len(),
                create.len(),
                relabel.len(),
            ]
            .iter()
            .any(|count| *count > 4096)
        {
            return Err(HandleError::InvalidParameter);
        }
        let labels: Vec<_> = labels
            .iter()
            .map(|(id, name)| RawLabel {
                id: *id,
                name_len: name.len() as u32,
                name: name.as_ptr() as u64,
            })
            .collect();
        let raw = scarlet_abi::shield::RawPolicyUpdate {
            version: scarlet_abi::shield::POLICY_UPDATE_VERSION,
            expected_generation,
            labels: labels.as_ptr() as u64,
            label_count: labels.len() as u64,
            rules: rules.as_ptr() as u64,
            rule_count: rules.len() as u64,
            transitions: transitions.as_ptr() as u64,
            transition_count: transitions.len() as u64,
            exec_transitions: exec.as_ptr() as u64,
            exec_transition_count: exec.len() as u64,
            file_create: create.as_ptr() as u64,
            file_create_count: create.len() as u64,
            file_relabel: relabel.as_ptr() as u64,
            file_relabel_count: relabel.len() as u64,
            ..Default::default()
        };
        // SAFETY: the specification, records and names are borrowed until return.
        let result = unsafe {
            scarlet_sys::syscall3(
                Syscall::ShieldPolicyExtend,
                self.0.as_raw() as usize,
                &raw as *const _ as usize,
                128,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(HandleError::InvalidParameter)
        }
    }

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
    /// Create file-dependent exec rules without delegating direct set-label authority.
    pub fn create_with_exec_transitions(labels: &[(u32, &str)], rules: &[Rule], transitions: &[Transition], exec: &[ExecTransition]) -> HandleResult<Self> {
        if labels.iter().any(|(_, name)| name.len() > 255) { return Err(HandleError::InvalidParameter); }
        let raw: Vec<_> = labels.iter().map(|(id, name)| RawLabel {id:*id, name_len:name.len() as u32, name:name.as_ptr() as u64}).collect();
        let spec = [raw.as_ptr() as u64, raw.len() as u64, rules.as_ptr() as u64, rules.len() as u64,
            transitions.as_ptr() as u64, transitions.len() as u64, exec.as_ptr() as u64, exec.len() as u64];
        // SAFETY: fixed-width specification and all borrowed arrays live through the copy.
        Self::returned(unsafe { syscall2(Syscall::ShieldPolicyCreateExec, spec.as_ptr() as usize, 64) })
    }
    /// Derive a fresh immutable policy before binding administrator-pinned files.
    pub fn with_file_rules(&self,default:u32,create:&[FileCreate],relabel:&[FileRelabel])->HandleResult<Self> {
        let spec=[default as u64,create.as_ptr() as u64,create.len() as u64,relabel.as_ptr() as u64,relabel.len() as u64];
        // SAFETY: fixed-width specification and borrowed arrays live through the synchronous copy.
        Self::returned(unsafe {scarlet_sys::syscall3(Syscall::ShieldFilePolicy,self.0.as_raw() as usize,spec.as_ptr() as usize,40)})
    }
    /// Bind a native inode identity to this policy until the policy is released.
    /// Requires management authority; a different existing binding is rejected.
    pub fn bind_file(&self, file: &Handle, label: u32) -> HandleResult<()> {
        // SAFETY: both live capabilities are validated by the kernel.
        let result = unsafe { scarlet_sys::syscall3(Syscall::ShieldBindFile, self.0.as_raw() as usize, file.as_raw() as usize, label as usize) };
        if result == 0 { Ok(()) } else { Err(HandleError::InvalidParameter) }
    }
    /// Change inode metadata only when the current subject has an explicit rule.
    pub fn relabel_file(&self,file:&Handle,label:u32)->HandleResult<()> {
        // SAFETY: native policy identity, retained file and current subject are checked together.
        if unsafe {scarlet_sys::syscall3(Syscall::ShieldRelabelFile,self.0.as_raw() as usize,file.as_raw() as usize,label as usize)}==0 {Ok(())}else {Err(HandleError::InvalidParameter)}
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
    /// Reserve an explicitly delegated transition for the next successful exec.
    pub fn reserve_exec(&self, label: u32) -> HandleResult<()> {
        // SAFETY: the kernel checks the policy, current subject and native identities.
        if unsafe { syscall2(Syscall::ShieldReserveExec, self.0.as_raw() as usize, label as usize) } == 0 { Ok(()) }
        else { Err(HandleError::InvalidParameter) }
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

/// Cancel this task's reserved exec transition without consuming an exec.
pub fn clear_exec_transition() -> HandleResult<()> {
    // SAFETY: scalar operation restricted to the current task.
    if unsafe { syscall0(Syscall::ShieldClearExec) } == 0 { Ok(()) } else { Err(HandleError::InvalidParameter) }
}
/// Monotonically suppress privileged address introspection. Fork/spawn/exec inherit it.
pub fn hide_kernel_addresses() -> HandleResult<()> {
    // SAFETY: tightening this caller's information constraint requires no pointer.
    if unsafe { syscall1(Syscall::ShieldConstrainInformation, 1) } == 0 { Ok(()) } else { Err(HandleError::InvalidParameter) }
}
pub fn kernel_addresses_hidden() -> bool {
    // SAFETY: scalar query of this caller's inherited restrictions.
    unsafe { syscall0(Syscall::ShieldInformation) & 1 != 0 }
}

/// Select a label authorized by this task's native policy, or zero to clear it.
pub fn file_creation_label(label:u32)->HandleResult<()> {
    // SAFETY: the kernel checks the current subject and immutable create grants.
    if unsafe {syscall1(Syscall::ShieldCreationLabel,label as usize)}==0 {Ok(())}else {Err(HandleError::InvalidParameter)}
}

pub use scarlet_abi::shield::{SOCKET_LABEL_CLASS,SOCKET_LABEL_CREATE};
/// Select a delegated native local-socket endpoint label. Zero clears the selection.
pub fn set_socket_creation_label(target:Option<u32>)->HandleResult<()> {
    // SAFETY: no pointers; Shield checks current-subject authority.
    let result=unsafe {syscall1(Syscall::ShieldSocketCreationLabel,target.unwrap_or(0) as usize)};
    if result==0 {Ok(())}else {Err(HandleError::SystemError(-1))}
}
