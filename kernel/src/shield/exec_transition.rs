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
}
pub fn reserve(task: &Task, policy: &Arc<Policy>, target: u32) -> Result<(), &'static str> {
    if !task.can_replace_exec_image() { return Err("exec transition requires exclusive task"); }
    let subject = task.security_subject.read().clone().ok_or("task has no subject")?;
    let credentials = task.credentials.read();
    if !Arc::ptr_eq(subject.policy(), policy) { return Err("foreign exec transition policy"); }
    subject.transition(target, &credentials)?;
    *task.pending_exec_transition.write() = Some(PendingExecTransition { source: subject, target, principal: credentials.principals().active });
    Ok(())
}
pub fn cancel(task: &Task) -> Result<(), &'static str> {
    if !task.can_replace_exec_image() { return Err("exec transition requires exclusive task"); }
    *task.pending_exec_transition.write() = None;
    Ok(())
}
