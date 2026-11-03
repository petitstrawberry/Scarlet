//! Administrator-defined label sets. Membership and shared rules are published
//! in the same immutable policy generation as ordinary labels and grants.
//! Sets contain labels only (never other sets), and do not alter label identity.
use alloc::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    vec::Vec,
};

pub use scarlet_abi::shield::{MAX_LABEL_SETS as MAX_SETS, MAX_SET_MEMBERS as MAX_MEMBERS};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Selector {
    Label(u32),
    Set(u32),
}
#[derive(Clone, Copy)]
pub struct Grant {
    pub source: Selector,
    pub target: Selector,
    pub class: u32,
    pub permissions: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileAction {
    Create,
    Relabel,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileRule {
    pub source: Selector,
    pub origin: Selector,
    pub target: Selector,
    pub kind: u32,
    pub action: FileAction,
    pub different: bool,
}
#[derive(Default)]
pub struct Update {
    pub sets: Vec<u32>,
    pub members: Vec<(u32, u32)>,
    pub grants: Vec<Grant>,
    pub files: Vec<FileRule>,
}
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct SetPolicy {
    sets: BTreeSet<u32>,
    /// Reverse index makes access checks proportional to the queried labels'
    /// memberships, without expanding a set rule into all label pairs.
    memberships: BTreeMap<u32, BTreeSet<u32>>,
    grants: BTreeMap<(Selector, Selector, u32), u64>,
    files: BTreeSet<FileRule>,
}
impl SetPolicy {
    pub(super) fn add(
        &mut self,
        labels: &BTreeMap<u32, Arc<str>>,
        has_file_default: bool,
        update: Update,
    ) -> Result<(), &'static str> {
        if update.sets.len() > MAX_SETS
            || update.members.len() > MAX_MEMBERS
            || update.grants.len() > super::MAX_RULES
            || update.files.len() > super::MAX_RULES
        {
            return Err("label set update bound");
        }
        let mut declarations = BTreeSet::new();
        for id in update.sets {
            if id == 0 || !declarations.insert(id) {
                return Err("invalid or duplicate label set");
            }
            self.sets.insert(id);
        }
        for (set, label) in update.members {
            if !self.sets.contains(&set) || !labels.contains_key(&label) {
                return Err("unknown set member");
            }
            self.memberships.entry(label).or_default().insert(set);
        }
        for grant in update.grants {
            if !self.valid(grant.source, labels)
                || !self.valid(grant.target, labels)
                || grant.class == 0
                || grant.permissions == 0
            {
                return Err("invalid set grant");
            }
            *self
                .grants
                .entry((grant.source, grant.target, grant.class))
                .or_default() |= grant.permissions;
        }
        if !has_file_default && !update.files.is_empty() {
            return Err("set file rules require file policy");
        }
        for rule in update.files {
            if !(1..=7).contains(&rule.kind)
                || !self.valid(rule.source, labels)
                || !self.valid(rule.origin, labels)
                || !self.valid(rule.target, labels)
                || rule.action == FileAction::Create && rule.different
            {
                return Err("invalid set file rule");
            }
            self.files.insert(rule);
        }
        if self.sets.len() > MAX_SETS
            || self.memberships.values().map(BTreeSet::len).sum::<usize>() > MAX_MEMBERS
            || self.grants.len() > super::MAX_RULES
            || self.files.len() > super::MAX_RULES
        {
            return Err("complete label set policy bound");
        }
        Ok(())
    }
    fn valid(&self, selector: Selector, labels: &BTreeMap<u32, Arc<str>>) -> bool {
        match selector {
            Selector::Label(id) => labels.contains_key(&id),
            Selector::Set(id) => self.sets.contains(&id),
        }
    }
    fn selectors(&self, label: u32) -> impl Iterator<Item = Selector> + '_ {
        core::iter::once(Selector::Label(label)).chain(
            self.memberships
                .get(&label)
                .into_iter()
                .flat_map(|sets| sets.iter().copied())
                .map(Selector::Set),
        )
    }
    pub(super) fn allowed(&self, source: u32, target: u32, class: u32) -> u64 {
        let mut mask = 0;
        for source in self.selectors(source) {
            for target in self.selectors(target) {
                mask |= self
                    .grants
                    .get(&(source, target, class))
                    .copied()
                    .unwrap_or(0);
            }
        }
        mask
    }
    pub(super) fn file_allowed(
        &self,
        source: u32,
        origin: u32,
        target: u32,
        kind: u32,
        action: FileAction,
    ) -> bool {
        self.files.iter().any(|rule| {
            rule.kind == kind
                && rule.action == action
                && (!rule.different || origin != target)
                && self.matches(rule.source, source)
                && self.matches(rule.origin, origin)
                && self.matches(rule.target, target)
        })
    }
    fn matches(&self, selector: Selector, label: u32) -> bool {
        match selector {
            Selector::Label(id) => id == label,
            Selector::Set(id) => self
                .memberships
                .get(&label)
                .is_some_and(|sets| sets.contains(&id)),
        }
    }
    pub(super) fn clear_file_rules(&mut self) {
        self.files.clear();
    }
    pub(super) fn creation_delegated(&self, source: u32, target: u32) -> bool {
        self.files.iter().any(|rule| {
            rule.action == FileAction::Create
                && self.matches(rule.source, source)
                && self.matches(rule.target, target)
                && match rule.origin {
                    Selector::Label(_) => true,
                    Selector::Set(id) => self.memberships.values().any(|sets| sets.contains(&id)),
                }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shield::{Policy, update::PolicyUpdate};
    use alloc::{format, vec};
    #[test_case]
    fn set_grants_combine_with_exact_grants_and_updates_preserve_identity() {
        let policy = Policy::new(
            vec![
                (1, "one".into()),
                (2, "two".into()),
                (3, "service".into()),
                (4, "other".into()),
            ],
            vec![super::super::Rule {
                source: 1,
                target: 3,
                class: 1,
                permissions: 1,
            }],
        )
        .unwrap();
        let subject = policy.subject(1).unwrap();
        let generation = policy
            .extend(
                1,
                PolicyUpdate {
                    sets: Update {
                        sets: vec![9],
                        members: vec![(9, 1)],
                        grants: vec![Grant {
                            source: Selector::Set(9),
                            target: Selector::Label(3),
                            class: 1,
                            permissions: 2,
                        }],
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(policy.decision(1, 3, 1), (3, generation));
        assert_eq!(policy.allowed(2, 3, 1), 0);
        policy
            .extend(
                generation,
                PolicyUpdate {
                    sets: Update {
                        members: vec![(9, 2)],
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(policy.allowed(2, 3, 1), 2);
        assert_eq!(policy.allowed(4, 3, 1), 0);
        assert_eq!(subject.label(), 1);
        assert_eq!(policy.label(1).unwrap().as_ref(), "one");
        let before = policy.generation();
        assert!(
            policy
                .extend(
                    before,
                    PolicyUpdate {
                        sets: Update {
                            members: vec![(9, 4), (10, 2)],
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                )
                .is_err()
        );
        assert_eq!(policy.generation(), before);
        assert_eq!(policy.allowed(4, 3, 1), 0);
    }
    #[test_case]
    fn file_set_rules_match_expanded_decisions_without_cartesian_storage() {
        let labels = (1..=151).map(|id| (id, format!("label-{id}"))).collect();
        let policy = Policy::new(labels, vec![])
            .unwrap()
            .with_file_rules(Some(1), vec![], vec![])
            .unwrap();
        let mut members = Vec::new();
        for label in 2..=151 {
            members.push((1, label));
            if label <= 101 {
                members.push((2, label));
            }
        }
        let rule = FileRule {
            source: Selector::Label(1),
            origin: Selector::Set(1),
            target: Selector::Set(2),
            kind: 2,
            action: FileAction::Relabel,
            different: true,
        };
        policy
            .extend(
                1,
                PolicyUpdate {
                    sets: Update {
                        sets: vec![1, 2],
                        members,
                        files: vec![rule],
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .unwrap();
        let data = policy.snapshot();
        assert_eq!(data.sets.files.len(), 1);
        for old in 1..=151 {
            for target in 1..=151 {
                assert_eq!(
                    data.sets
                        .file_allowed(1, old, target, 2, FileAction::Relabel),
                    old >= 2 && (2..=101).contains(&target) && old != target
                );
            }
        }
        assert!(!data.sets.file_allowed(2, 5, 6, 2, FileAction::Relabel));
        assert!(!data.sets.file_allowed(1, 5, 6, 1, FileAction::Relabel));
        assert!(!data.sets.file_allowed(1, 5, 6, 2, FileAction::Create));
    }
}
