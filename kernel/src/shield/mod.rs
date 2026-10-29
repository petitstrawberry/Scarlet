//! Shield: Scarlet's protection primitives. Immutable, ABI-neutral label policies.
//! Numeric classes and permission bits are interpreted by enforcement points,
//! never by this policy engine.
//! Possession of a policy identity is distinct from authority to assign labels.
pub mod syscall;

use alloc::{collections::BTreeMap, string::String, sync::Arc, vec::Vec};

pub const MAX_LABELS: usize = 256;
pub const MAX_RULES: usize = 4096;
pub const MAX_LABEL_BYTES: usize = 255;

#[derive(Clone, Copy)]
pub struct Rule {
    pub source: u32,
    pub target: u32,
    pub class: u32,
    pub permissions: u64,
}

pub struct Policy {
    labels: BTreeMap<u32, String>,
    names: BTreeMap<String, u32>,
    grants: BTreeMap<(u32, u32, u32), u64>,
}
impl Policy {
    /// Validate the complete policy before publishing it. No live mutation or
    /// permissive fallback exists; caches can retain decisions for its lifetime.
    pub fn new(labels: Vec<(u32, String)>, rules: Vec<Rule>) -> Result<Arc<Self>, &'static str> {
        if labels.is_empty() || labels.len() > MAX_LABELS || rules.len() > MAX_RULES {
            return Err("policy size limit");
        }
        let mut result = Self {
            labels: BTreeMap::new(),
            names: BTreeMap::new(),
            grants: BTreeMap::new(),
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
}
