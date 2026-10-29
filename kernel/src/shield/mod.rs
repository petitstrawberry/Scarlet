//! Shield: Scarlet's protection primitives. Immutable, ABI-neutral label policies.
//! Numeric classes and permission bits are interpreted by enforcement points,
//! never by this policy engine.
//! Possession of a policy identity is distinct from authority to assign labels.
pub mod execution;
pub mod exec_transition;
pub mod information;
pub mod resource;
pub mod identity;
pub mod syscall;

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

pub struct Policy {
    labels: BTreeMap<u32, String>,
    names: BTreeMap<String, u32>,
    grants: BTreeMap<(u32, u32, u32), u64>,
    transitions: BTreeSet<(u32, u32, u32)>,
    exec_transitions: BTreeMap<(u32, u32, u32), u32>,
    resources: crate::sync::IrqRwSpinLock<resource::Resources>,
}
impl Policy {
    /// Validate the complete policy before publishing it. No live mutation or
    /// permissive fallback exists; caches can retain decisions for its lifetime.
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
        labels: Vec<(u32, String)>, rules: Vec<Rule>, transitions: Vec<TransitionRule>,
        exec_transitions: Vec<ExecTransitionRule>,
    ) -> Result<Arc<Self>, &'static str> {
        if exec_transitions.len() > MAX_RULES { return Err("exec transition limit"); }
        if transitions.len() > MAX_RULES {
            return Err("transition limit");
        }
        if labels.is_empty() || labels.len() > MAX_LABELS || rules.len() > MAX_RULES {
            return Err("policy size limit");
        }
        let mut result = Self {
            labels: BTreeMap::new(),
            names: BTreeMap::new(),
            grants: BTreeMap::new(),
            transitions: BTreeSet::new(),
            exec_transitions: BTreeMap::new(),
            resources: crate::sync::IrqRwSpinLock::new(resource::Resources::new()),
        };
        for (id, name) in labels {
            if id == 0
                || name.is_empty()
                || name.len() > MAX_LABEL_BYTES
                || name.contains('\0')
                || result.labels.contains_key(&id)
                || result.names.contains_key(&name)
            {
                return Err("invalid or duplicate label");
            }
            result.names.insert(name.clone(), id);
            result.labels.insert(id, name);
        }
        for rule in rules {
            if rule.class == 0
                || rule.permissions == 0
                || !result.labels.contains_key(&rule.source)
                || !result.labels.contains_key(&rule.target)
            {
                return Err("invalid grant");
            }
            *result
                .grants
                .entry((rule.source, rule.target, rule.class))
                .or_default() |= rule.permissions;
        }
        for rule in transitions {
            if rule.source == rule.target
                || !result.labels.contains_key(&rule.source)
                || !result.labels.contains_key(&rule.target)
                || !result
                    .transitions
                    .insert((rule.source, rule.target, rule.principal.0))
            {
                return Err("invalid or duplicate transition");
            }
        }
        for rule in exec_transitions {
            if !result.labels.contains_key(&rule.source) || !result.labels.contains_key(&rule.resource)
                || !result.labels.contains_key(&rule.target)
                || result.exec_transitions.insert((rule.source, rule.resource, rule.principal.0), rule.target).is_some() {
                return Err("invalid or ambiguous exec transition");
            }
        }
        Ok(Arc::new(result))
    }
    pub fn label(&self, id: u32) -> Option<&str> {
        self.labels.get(&id).map(String::as_str)
    }
    pub fn label_id(&self, name: &str) -> Option<u32> {
        self.names.get(name).copied()
    }
    pub fn labels(&self) -> impl Iterator<Item = (u32, &str)> {
        self.labels.iter().map(|(&id, name)| (id, name.as_str()))
    }
    pub fn allowed(&self, source: u32, target: u32, class: u32) -> u64 {
        self.grants
            .get(&(source, target, class))
            .copied()
            .unwrap_or(0)
    }
    pub fn subject(self: &Arc<Self>, label: u32) -> Result<Subject, &'static str> {
        self.labels
            .contains_key(&label)
            .then(|| Subject {
                policy: self.clone(),
                label,
            })
            .ok_or("unknown label")
    }
}

#[derive(Clone)]
pub struct Subject {
    policy: Arc<Policy>,
    label: u32,
}
impl core::fmt::Debug for Subject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Subject").field("label", &self.label).field("name", &self.name()).finish()
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
        self.policy.label(self.label).unwrap()
    }
    pub fn same_identity(&self, other: &Self) -> bool {
        self.label == other.label && Arc::ptr_eq(&self.policy, &other.policy)
    }
    pub fn can_transition(&self) -> bool {
        self.policy
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
