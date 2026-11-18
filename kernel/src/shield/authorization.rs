//! Grant acquisition. Callers supply a resolved target and its independent
//! authority (inode rights, an existing grant, or credential authority).
//! Shield selects the policy class, resolves the target label and applies the
//! caller's domain ceiling. Transferred files deliberately skip pathname DAC.
use super::Subject;
use crate::{fs::vfs_v2::core::VfsNode, task::Task};
use scarlet_abi::shield as abi;

pub(crate) enum Target<'a> {
    Inode(&'a dyn VfsNode),
    NewInode(Option<u32>),
    ReceivedFile(&'a dyn VfsNode),
    ReceivedCapability,
    LocalEndpoint(Option<&'a Subject>),
    Internet,
    System,
}

impl Target<'_> {
    pub(crate) fn grant(self, task: &Task, available: u64) -> u64 {
        let Some(source) = task.security_subject.read().clone() else {
            return available;
        };
        let policy = source.policy();
        // Earlier label blueprints only describe ABI-specific classes. The
        // first native resource rule opts the whole policy into default-deny.
        // System administration always requires an explicit labeled grant.
        if !matches!(self, Self::System) && !policy.snapshot().native_access {
            return available;
        }
        let (class, target) = match self {
            Self::Inode(node) | Self::ReceivedFile(node) => {
                (abi::VFS_ACCESS_CLASS, policy.resource_label(node))
            }
            Self::NewInode(label) => (abi::VFS_ACCESS_CLASS, label),
            Self::LocalEndpoint(peer) => (
                abi::ENDPOINT_ACCESS_CLASS,
                peer.filter(|s| alloc::sync::Arc::ptr_eq(s.policy(), policy))
                    .map(Subject::label),
            ),
            Self::Internet => (abi::ENDPOINT_ACCESS_CLASS, Some(source.label())),
            Self::System => (abi::SYSTEM_MANAGEMENT_CLASS, Some(source.label())),
            Self::ReceivedCapability => (abi::CAPABILITY_RECEIVE_CLASS, Some(source.label())),
        };
        let ceiling = target.map_or(0, |label| policy.allowed(source.label(), label, class));
        available
            & if matches!(self, Self::ReceivedFile(_)) {
                ceiling >> 3
            } else {
                ceiling
            }
    }

    pub(crate) fn permits(self, task: &Task, requested: u64) -> bool {
        self.grant(task, requested) == requested
    }
}

pub(crate) fn is_native_class(class: u32) -> bool {
    matches!(
        class,
        abi::VFS_ACCESS_CLASS
            | abi::ENDPOINT_ACCESS_CLASS
            | abi::CAPABILITY_RECEIVE_CLASS
            | abi::SYSTEM_MANAGEMENT_CLASS
    )
}

#[derive(Clone, Copy)]
pub enum Management {
    Network,
    Modules,
}

pub fn permits_management(task: &Task, action: Management) -> bool {
    use super::identity::{MANAGE_MODULES, MANAGE_TRAFFIC};
    let (authority, permission) = match action {
        Management::Network => (MANAGE_TRAFFIC, abi::ADMIN_NETWORK),
        Management::Modules => (MANAGE_MODULES, abi::ADMIN_MODULES),
    };
    let available = if task.credentials.read().permits(authority) {
        permission
    } else {
        0
    };
    Target::System.grant(task, available) == permission
}

#[cfg(feature = "network")]
pub(crate) fn connect(target: Target<'_>) -> Result<(), crate::network::SocketError> {
    let operation = if matches!(target, Target::Internet) {
        abi::CONNECT_INTERNET
    } else {
        abi::CONNECT_LOCAL
    };
    if crate::task::mytask().is_some_and(|t| !target.permits(&t, operation)) {
        return Err(crate::network::SocketError::PermissionDenied);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        shield::{Policy, Rule},
        task::TaskType,
    };
    use alloc::vec;

    #[test_case]
    fn common_ceiling_uses_policy_identity_and_preserves_independent_authority() {
        let task = Task::new("authorization".into(), 0, TaskType::User);
        let labels = vec![(1, "client".into()), (2, "service".into())];
        let foreign = Policy::new(labels.clone(), vec![]).unwrap();
        let empty = foreign
            .derive_file_policy(None, vec![], vec![], true)
            .unwrap();
        *task.security_subject.write() = Some(empty.subject(1).unwrap());
        assert!(!Target::Internet.permits(&task, abi::CONNECT_INTERNET));
        assert!(!Target::ReceivedCapability.permits(&task, abi::RECEIVE_CAPABILITY));
        let policy = Policy::new(
            labels,
            vec![
                Rule {
                    source: 1,
                    target: 2,
                    class: abi::ENDPOINT_ACCESS_CLASS,
                    permissions: abi::CONNECT_LOCAL,
                },
                Rule {
                    source: 1,
                    target: 1,
                    class: abi::SYSTEM_MANAGEMENT_CLASS,
                    permissions: abi::ADMIN_NETWORK,
                },
            ],
        )
        .unwrap();
        *task.security_subject.write() = Some(policy.subject(1).unwrap());
        assert!(
            Target::LocalEndpoint(Some(&policy.subject(2).unwrap()))
                .permits(&task, abi::CONNECT_LOCAL)
        );
        assert!(
            !Target::LocalEndpoint(Some(&foreign.subject(2).unwrap()))
                .permits(&task, abi::CONNECT_LOCAL)
        );
        assert!(!Target::LocalEndpoint(None).permits(&task, abi::CONNECT_LOCAL));
        assert!(permits_management(&task, Management::Network));
        task.credentials.write().set_authority(0, 0).unwrap();
        assert!(!permits_management(&task, Management::Network));
    }
}
