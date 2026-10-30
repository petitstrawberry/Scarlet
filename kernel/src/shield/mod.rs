//! Shield: Scarlet's protection primitives. Versioned, ABI-neutral label policies.
//! Numeric classes and permission bits are interpreted by enforcement points,
//! never by this policy engine.
//! Possession of a policy identity is distinct from authority to assign labels.
pub mod exec_transition;
pub mod execution;
pub mod file;
pub mod identity;
pub mod information;
pub mod resource;
pub mod socket;
pub mod syscall;
pub mod update;

use alloc::{
    collections::{BTreeMap, BTreeSet},
    string::String,
    sync::Arc,
    vec::Vec,
};

pub const MAX_LABELS: usize = 512;
pub const MAX_RULES: usize = 4096;
pub const MAX_LABEL_BYTES: usize = 255;

#[derive(Clone, Copy)]
pub struct Rule {
    pub source: u32,
    pub target: u32,
    pub class: u32,
    pub permissions: u64,
}

/// An explicit transition after all principal identity slots have become the
/// delegated principal. Policy construction, not numeric identity, grants it.
#[derive(Clone, Copy)]
pub struct TransitionRule {
    pub source: u32,
    pub target: u32,
    pub principal: identity::Principal,
}

#[derive(Clone, Copy)]
pub struct ExecTransitionRule {
    pub source: u32,
    pub resource: u32,
    pub target: u32,
    pub principal: identity::Principal,
}

/// Immutable policy contents published together under one stable capability identity.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PolicyData {
    generation: u64,
    pub(crate) files: file::FilePolicy,
    labels: BTreeMap<u32, Arc<str>>,
    names: BTreeMap<String, u32>,
    grants: BTreeMap<(u32, u32, u32), u64>,
    transitions: BTreeSet<(u32, u32, u32)>,
    exec_transitions: BTreeMap<(u32, u32, u32), u32>,
}

/// ABI adapters may retain a weakly registered observer for cache invalidation.
/// Notifications can coalesce or arrive out of order; observers retain the
/// highest generation they have seen. No observer runs under the policy lock.
pub trait PolicyObserver: Send + Sync {
    fn policy_changed(&self, generation: u64);
}

pub struct Policy {
    data: crate::sync::IrqRwSpinLock<Arc<PolicyData>>,
    observers: crate::sync::IrqSpinLock<Vec<alloc::sync::Weak<dyn PolicyObserver>>>,
    resources: crate::sync::IrqRwSpinLock<resource::Resources>,
}
impl Policy {
    fn from_data(data: PolicyData) -> Arc<Self> {
        Arc::new(Self {
            data: crate::sync::IrqRwSpinLock::new(Arc::new(data)),
            observers: crate::sync::IrqSpinLock::new(Vec::new()),
            resources: crate::sync::IrqRwSpinLock::new(resource::Resources::new()),
        })
    }
    pub(crate) fn snapshot(&self) -> Arc<PolicyData> {
        self.data.read().clone()
    }
    pub fn generation(&self) -> u64 {
        self.data.read().generation
    }
    pub fn new(labels: Vec<(u32, String)>, rules: Vec<Rule>) -> Result<Arc<Self>, &'static str> {
        Self::with_transitions(labels, rules, Vec::new())
    }
    pub fn with_transitions(
        labels: Vec<(u32, String)>,
        rules: Vec<Rule>,
        transitions: Vec<TransitionRule>,
    ) -> Result<Arc<Self>, &'static str> {
        Self::with_exec_transitions(labels, rules, transitions, Vec::new())
    }
    pub fn with_exec_transitions(
        labels: Vec<(u32, String)>,
        rules: Vec<Rule>,
        transitions: Vec<TransitionRule>,
        exec_transitions: Vec<ExecTransitionRule>,
    ) -> Result<Arc<Self>, &'static str> {
        if labels.is_empty() {
            return Err("policy requires labels");
        }
        let mut data = PolicyData {
            generation: 1,
            files: file::FilePolicy::default(),
            labels: BTreeMap::new(),
            names: BTreeMap::new(),
            grants: BTreeMap::new(),
            transitions: BTreeSet::new(),
            exec_transitions: BTreeMap::new(),
        };
        data.add(update::PolicyUpdate {
            labels,
            rules,
            transitions,
            exec_transitions,
            ..Default::default()
        })?;
        Ok(Self::from_data(data))
    }
    /// Derivation keeps the explicit contents, creates a fresh policy identity,
    /// and leaves inode bindings on the original identity.
    pub fn with_file_rules(
        &self,
        default: Option<u32>,
        create: Vec<file::CreateRule>,
        relabel: Vec<file::RelabelRule>,
    ) -> Result<Arc<Self>, &'static str> {
        let mut data = (*self.snapshot()).clone();
        data.files = file::FilePolicy::build(&data.labels, default, create, relabel)?;
        data.generation = 1;
        Ok(Self::from_data(data))
    }
    /// Additions never replace a label, remove a grant, or change an existing
    /// exec/default selection. Validate a private snapshot before publishing it.
    pub(crate) fn extend(
        &self,
        expected: u64,
        update: update::PolicyUpdate,
    ) -> Result<u64, &'static str> {
        let previous = self.snapshot();
        if previous.generation != expected {
            return Err("policy generation changed");
        }
        let mut next = (*previous).clone();
        next.add(update)?;
        if next == *previous {
            return Ok(expected);
        }
        next.generation = expected
            .checked_add(1)
            .ok_or("policy generation exhausted")?;
        let generation = next.generation;
        {
            let mut data = self.data.write();
            if data.generation != expected {
                return Err("policy generation changed");
            }
            *data = Arc::new(next);
        }
        let listeners = {
            let mut observers = self.observers.lock();
            observers.retain(|observer| observer.strong_count() != 0);
            observers
                .iter()
                .filter_map(alloc::sync::Weak::upgrade)
                .collect::<Vec<_>>()
        };
        for observer in listeners {
            observer.policy_changed(generation);
        }
        Ok(generation)
    }
    pub fn observe(&self, observer: &Arc<dyn PolicyObserver>) -> Result<(), &'static str> {
        let mut observers = self.observers.lock();
        observers.retain(|observer| observer.strong_count() != 0);
        if observers.len() >= 64 {
            return Err("policy observer bound");
        }
        // Register and initialize against publication notifications under the
        // same observer lock, so a concurrent update cannot be missed.
        observer.policy_changed(self.generation());
        observers.push(Arc::downgrade(observer));
        Ok(())
    }
    pub fn label(&self, id: u32) -> Option<Arc<str>> {
        self.data.read().labels.get(&id).cloned()
    }
    pub fn label_id(&self, name: &str) -> Option<u32> {
        self.data.read().names.get(name).copied()
    }
    pub fn labels(&self) -> impl Iterator<Item = (u32, Arc<str>)> {
        self.data
            .read()
            .labels
            .iter()
            .map(|(&id, name)| (id, name.clone()))
            .collect::<Vec<_>>()
            .into_iter()
    }
    pub fn allowed(&self, source: u32, target: u32, class: u32) -> u64 {
        self.data
            .read()
            .grants
            .get(&(source, target, class))
            .copied()
            .unwrap_or(0)
    }
    pub fn decision(&self, source: u32, target: u32, class: u32) -> (u64, u64) {
        let data = self.data.read();
        (
            data.grants
                .get(&(source, target, class))
                .copied()
                .unwrap_or(0),
            data.generation,
        )
    }
    pub fn subject(self: &Arc<Self>, label: u32) -> Result<Subject, &'static str> {
        Ok(Subject {
            policy: self.clone(),
            label,
            name: self.label(label).ok_or("unknown label")?,
        })
    }
}

#[derive(Clone)]
pub struct Subject {
    policy: Arc<Policy>,
    label: u32,
    name: Arc<str>,
}
impl core::fmt::Debug for Subject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Subject")
            .field("label", &self.label)
            .field("name", &self.name())
            .finish()
    }
}
impl Subject {
    pub fn policy(&self) -> &Arc<Policy> {
        &self.policy
    }
    pub fn label(&self) -> u32 {
        self.label
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn same_identity(&self, other: &Self) -> bool {
        self.label == other.label && Arc::ptr_eq(&self.policy, &other.policy)
    }
    pub fn can_transition(&self) -> bool {
        self.policy
            .snapshot()
            .transitions
            .iter()
            .any(|&(source, _, _)| source == self.label)
    }
    pub fn transition(
        &self,
        target: u32,
        credentials: &identity::Credentials,
    ) -> Result<Self, &'static str> {
        let ids = credentials.principals();
        if ids != identity::Identities::all(ids.active)
            || !self
                .policy
                .snapshot()
                .transitions
                .contains(&(self.label, target, ids.active.0))
        {
            return Err("label transition not delegated for these identities");
        }
        self.policy.subject(target)
    }
    pub fn permits(&self, target: &Self, class: u32, requested: u64) -> bool {
        requested != 0
            && Arc::ptr_eq(&self.policy, &target.policy)
            && self.policy.allowed(self.label, target.label, class) & requested == requested
    }
}

#[derive(Clone)]
pub struct PolicyHandle {
    pub policy: Arc<Policy>,
    pub administer: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    fn policy() -> Arc<Policy> {
        Policy::new(
            vec![
                (1, "caller".into()),
                (2, "service".into()),
                (3, "denied".into()),
            ],
            vec![Rule {
                source: 1,
                target: 2,
                class: 7,
                permissions: 3,
            }],
        )
        .unwrap()
    }
    #[test_case]
    fn security_policy_denies_unknown_labels_classes_bits_and_cross_policy_identity() {
        let p = policy();
        let caller = p.subject(1).unwrap();
        let target = p.subject(2).unwrap();
        assert!(caller.permits(&target, 7, 3));
        assert!(!caller.permits(&target, 7, 4));
        assert!(!caller.permits(&target, 7, 0));
        assert!(!caller.permits(&target, 8, 1));
        assert!(!p.subject(3).unwrap().permits(&target, 7, 1));
        assert!(!caller.permits(&policy().subject(2).unwrap(), 7, 1));
        assert!(p.subject(999).is_err());
        assert_eq!(p.allowed(999, 2, 7), 0);
        assert_eq!(p.label_id("service"), Some(2));
    }
    #[test_case]
    fn security_policy_rejects_ambiguous_names_invalid_grants_and_excessive_input() {
        assert!(Policy::new(vec![(1, "one".into()), (1, "two".into())], vec![]).is_err());
        assert!(Policy::new(vec![(1, "same".into()), (2, "same".into())], vec![]).is_err());
        assert!(Policy::new(vec![(0, "zero".into())], vec![]).is_err());
        assert!(Policy::new(vec![(1, "a\0b".into())], vec![]).is_err());
        assert!(Policy::new(vec![(1, "x".repeat(MAX_LABEL_BYTES + 1))], vec![]).is_err());
        assert!(
            Policy::new(
                vec![(1, "one".into())],
                vec![Rule {
                    source: 1,
                    target: 2,
                    class: 1,
                    permissions: 1
                }]
            )
            .is_err()
        );
        assert!(Policy::new(vec![(1, "one".into()); MAX_LABELS + 1], vec![]).is_err());
    }
    #[test_case]
    fn transitions_require_a_complete_identity_and_do_not_grant_reverse_moves() {
        use identity::{Credentials, Identities, Principal};
        let policy = Policy::with_transitions(
            vec![(1, "launcher".into()), (2, "application".into())],
            vec![],
            vec![TransitionRule {
                source: 1,
                target: 2,
                principal: Principal(10001),
            }],
        )
        .unwrap();
        let source = policy.subject(1).unwrap();
        let mut credentials = Credentials::bootstrap();
        assert!(source.transition(2, &credentials).is_err());
        credentials
            .set_principals(Identities {
                base: Principal(0),
                active: Principal(10001),
                retained: Principal(0),
            })
            .unwrap();
        assert!(source.transition(2, &credentials).is_err());
        credentials
            .set_principals(Identities::all(Principal(10001)))
            .unwrap();
        let target = source.transition(2, &credentials).unwrap();
        assert_eq!(source.label(), 1);
        assert_eq!(target.label(), 2);
        assert!(!target.can_transition());
        assert!(target.transition(1, &credentials).is_err());
        credentials
            .set_principals(Identities::all(Principal(10002)))
            .unwrap();
        assert!(source.transition(2, &credentials).is_err());
    }
    #[test_case]
    fn transitions_reject_unknown_duplicate_and_unbounded_rules() {
        let rule = TransitionRule {
            source: 1,
            target: 2,
            principal: identity::Principal(10001),
        };
        let labels = vec![(1, "launcher".into()), (2, "application".into())];
        assert!(Policy::with_transitions(labels.clone(), vec![], vec![rule, rule]).is_err());
        assert!(
            Policy::with_transitions(
                labels.clone(),
                vec![],
                vec![TransitionRule { target: 3, ..rule }]
            )
            .is_err()
        );
        assert!(
            Policy::with_transitions(
                labels.clone(),
                vec![],
                vec![TransitionRule { target: 1, ..rule }]
            )
            .is_err()
        );
        assert!(Policy::with_transitions(labels, vec![], vec![rule; MAX_RULES + 1]).is_err());
    }
}
