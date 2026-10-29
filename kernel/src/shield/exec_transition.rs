//! A delegated label transition reserved for the next successful exec.
use super::{Subject, Policy, identity::{Credentials, Principal, Identities}};
use alloc::sync::Arc;
use crate::task::Task;

#[derive(Clone)]
pub struct PendingExecTransition { source: Subject, target: u32, principal: Principal }
impl PendingExecTransition {
    pub fn target(&self) -> Subject { self.source.policy().subject(self.target).expect("immutable label") }
    pub fn resolve(&self, current: &Subject, credentials: &Credentials) -> Result<Subject, &'static str> {
        if !self.source.same_identity(current) || credentials.principals() != Identities::all(self.principal) {
            return Err("exec transition source or principal changed");
        }
        current.transition(self.target, credentials)
    }
    pub fn resolve_for_resource(&self, current: &Subject, credentials: &Credentials, resource: Option<u32>, allow_file_grant: bool)
        -> Result<Subject, &'static str> {
        if let Ok(target) = self.resolve(current, credentials) { return Ok(target); }
        if !self.source.same_identity(current) || credentials.principals() != Identities::all(self.principal) || !allow_file_grant {
            return Err("exec resource transition source, principal or mount constraint");
        }
        let target = current.resource_exec(resource.ok_or("exec file is not labeled")?, credentials)?;
        if target.label() != self.target { return Err("exec resource does not match reservation"); }
        Ok(target)
    }
}
pub fn reserve(task: &Task, policy: &Arc<Policy>, target: u32) -> Result<(), &'static str> {
    if !task.can_replace_exec_image() { return Err("exec transition requires exclusive task"); }
    let subject = task.security_subject.read().clone().ok_or("task has no subject")?;
    let credentials = task.credentials.read();
    if !Arc::ptr_eq(subject.policy(), policy) { return Err("foreign exec transition policy"); }
    subject.reserve_exec_target(target, &credentials)?;
    *task.pending_exec_transition.write() = Some(PendingExecTransition { source: subject, target, principal: credentials.principals().active });
    Ok(())
}
pub fn cancel(task: &Task) -> Result<(), &'static str> {
    if !task.can_replace_exec_image() { return Err("exec transition requires exclusive task"); }
    *task.pending_exec_transition.write() = None;
    Ok(())
}
