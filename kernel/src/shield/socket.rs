//! Delegated creation labels for native local-socket endpoints.
use super::Subject;
use crate::task::Task;
pub use scarlet_abi::shield::{SOCKET_LABEL_CLASS, SOCKET_LABEL_CREATE};

/// A resource label must not form a cycle with administrator-bound VFS nodes.
#[derive(Clone)]
pub struct Label {
    policy: alloc::sync::Weak<super::Policy>,
    id: u32,
}
impl Label {
    pub fn from_subject(subject: Subject) -> Self {
        Self {
            policy: alloc::sync::Arc::downgrade(subject.policy()),
            id: subject.label(),
        }
    }
    pub fn subject(&self) -> Option<Subject> {
        self.policy.upgrade()?.subject(self.id).ok()
    }
}

#[derive(Clone)]
pub struct CreationLabel {
    source: Subject,
    target: u32,
}
impl CreationLabel {
    fn resolve(&self, current: &Subject) -> Result<Subject, &'static str> {
        if !self.source.same_identity(current)
            || current
                .policy()
                .allowed(current.label(), self.target, SOCKET_LABEL_CLASS)
                & SOCKET_LABEL_CREATE
                == 0
        {
            return Err("socket label delegation changed");
        }
        current.policy().subject(self.target)
    }
    pub fn target(&self) -> Subject {
        self.source
            .policy()
            .subject(self.target)
            .expect("immutable socket label")
    }
}
pub fn select(task: &Task, target: Option<u32>) -> Result<(), &'static str> {
    let selected = match target {
        None => None,
        Some(target) => {
            let source = task
                .security_subject
                .read()
                .clone()
                .ok_or("socket creator has no label")?;
            let selected = CreationLabel {
                source: source.clone(),
                target,
            };
            selected.resolve(&source)?;
            Some(selected)
        }
    };
    *task.socket_creation_label.write() = selected;
    Ok(())
}
pub fn label_for(task: &Task) -> Result<Option<Subject>, &'static str> {
    let current = task.security_subject.read().clone();
    match task.socket_creation_label.read().as_ref() {
        Some(selected) => Ok(Some(
            selected.resolve(current.as_ref().ok_or("socket creator label lost")?)?,
        )),
        None => Ok(current),
    }
}
pub fn current_label() -> Result<Option<Subject>, &'static str> {
    match crate::task::mytask() {
        Some(task) => label_for(&task),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        network::{SocketProtocol, SocketType, local::LocalSocket},
        shield::{Policy, Rule},
        task::{Task, TaskType, clear_mock_current_task, set_mock_current_task},
    };
    use alloc::{sync::Arc, vec};
    fn make_policy() -> Arc<Policy> {
        Policy::new(
            vec![
                (1, "creator".into()),
                (2, "endpoint".into()),
                (3, "other".into()),
            ],
            vec![Rule {
                source: 1,
                target: 2,
                class: SOCKET_LABEL_CLASS,
                permissions: SOCKET_LABEL_CREATE,
            }],
        )
        .unwrap()
    }
    #[test_case]
    fn endpoint_labels_are_immutable_and_do_not_replace_process_credentials() {
        let policy = make_policy();
        let task = Arc::new(Task::new("socket-label".into(), 1, TaskType::User));
        *task.security_subject.write() = Some(policy.subject(1).unwrap());
        set_mock_current_task(task.clone());
        select(&task, Some(2)).unwrap();
        let socket = LocalSocket::try_new(SocketType::Stream, SocketProtocol::Default).unwrap();
        let (first, second) = LocalSocket::try_create_connected_pair_with_type(
            SocketType::Stream,
            "one".into(),
            "two".into(),
        )
        .unwrap();
        select(&task, None).unwrap();
        assert_eq!(socket.security_label().unwrap().label(), 2);
        assert_eq!(first.peer_security_label().unwrap().label(), 2);
        assert_eq!(
            second.peer_credentials().unwrap().subject.unwrap().label(),
            1
        );
        assert_eq!(
            LocalSocket::try_new(SocketType::Stream, SocketProtocol::Default)
                .unwrap()
                .security_label()
                .unwrap()
                .label(),
            1
        );
        assert!(select(&task, Some(3)).is_err());
        assert!(select(&task, Some(99)).is_err());
        clear_mock_current_task();
    }
    #[test_case]
    fn socket_creation_rechecks_delegation_after_subject_or_policy_changes() {
        let policy = make_policy();
        let task = Task::new("socket-recheck".into(), 1, TaskType::User);
        *task.security_subject.write() = Some(policy.subject(1).unwrap());
        select(&task, Some(2)).unwrap();
        *task.security_subject.write() = Some(policy.subject(3).unwrap());
        assert!(label_for(&task).is_err());
        let foreign = make_policy();
        *task.security_subject.write() = Some(foreign.subject(1).unwrap());
        assert!(label_for(&task).is_err());
        *task.security_subject.write() = None;
        assert!(label_for(&task).is_err());
        select(&task, None).unwrap();
        assert!(label_for(&task).unwrap().is_none());
    }
}
