//! Bounded additions to a stable policy identity. Only the native management
//! capability exposes publication; subjects and current-policy handles cannot.
use super::*;

#[derive(Default)]
pub struct PolicyUpdate {
    pub labels: Vec<(u32, String)>,
    pub rules: Vec<Rule>,
    pub transitions: Vec<TransitionRule>,
    pub principal_ranges: Vec<PrincipalRangeTransitionRule>,
    pub exec_transitions: Vec<ExecTransitionRule>,
    pub create: Vec<file::CreateRule>,
    pub relabel: Vec<file::RelabelRule>,
    pub sets: sets::Update,
}

impl PolicyData {
    pub(super) fn add(&mut self, update: PolicyUpdate) -> Result<(), &'static str> {
        if update.labels.len() > MAX_LABELS
            || update.rules.len() > MAX_RULES
            || update.transitions.len() > MAX_RULES
            || update.principal_ranges.len() > MAX_RULES
            || update.exec_transitions.len() > MAX_RULES
        {
            return Err("policy update bound");
        }
        let mut identifiers = BTreeSet::new();
        let mut names = BTreeSet::new();
        for (id, name) in update.labels {
            if id == 0
                || name.is_empty()
                || name.len() > MAX_LABEL_BYTES
                || name.contains('\0')
                || !identifiers.insert(id)
                || !names.insert(name.clone())
            {
                return Err("invalid or duplicate label");
            }
            if let Some(previous) = self.labels.get(&id) {
                if previous.as_ref() != name {
                    return Err("label identity cannot change");
                }
                continue;
            }
            if self.names.contains_key(&name) {
                return Err("label name already registered");
            }
            self.names.insert(name.clone(), id);
            self.labels.insert(id, Arc::from(name));
        }
        for rule in update.rules {
            if rule.class == 0
                || rule.permissions == 0
                || !self.labels.contains_key(&rule.source)
                || !self.labels.contains_key(&rule.target)
            {
                return Err("invalid grant");
            }
            *self
                .grants
                .entry((rule.source, rule.target, rule.class))
                .or_default() |= rule.permissions;
        }
        let mut transitions = BTreeSet::new();
        for rule in update.transitions {
            let key = (rule.source, rule.target, rule.principal.0);
            if rule.source == rule.target
                || !self.labels.contains_key(&rule.source)
                || !self.labels.contains_key(&rule.target)
                || !transitions.insert(key)
            {
                return Err("invalid or duplicate transition");
            }
            self.transitions.insert(key);
        }
        let mut ranges=BTreeSet::new();
        for rule in update.principal_ranges {
            let key=(rule.source,rule.target,rule.first.0,rule.last.0);
            if rule.source==rule.target || rule.first.0>rule.last.0
                || !self.labels.contains_key(&rule.source) || !self.labels.contains_key(&rule.target)
                || !ranges.insert(key) {
                return Err("invalid or duplicate principal transition range");
            }
            self.principal_ranges.insert(key);
        }
        let mut execs = BTreeSet::new();
        for rule in update.exec_transitions {
            let key = (rule.source, rule.resource, rule.principal.0);
            if !self.labels.contains_key(&rule.source)
                || !self.labels.contains_key(&rule.resource)
                || !self.labels.contains_key(&rule.target)
                || !execs.insert(key)
                || self
                    .exec_transitions
                    .get(&key)
                    .is_some_and(|target| *target != rule.target)
            {
                return Err("invalid or ambiguous exec transition");
            }
            self.exec_transitions.insert(key, rule.target);
        }
        self.files
            .add(&self.labels, update.create, update.relabel)?;
        self.sets.add(&self.labels, self.files.default.is_some(), update.sets)?;
        if self.labels.len() > MAX_LABELS
            || self.grants.len() > MAX_RULES
            || self.transitions.len().saturating_add(self.principal_ranges.len()) > MAX_RULES
            || self.exec_transitions.len() > MAX_RULES
        {
            return Err("complete policy exceeds bound");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    #[test_case]
    fn principal_ranges_are_inclusive_directional_and_require_complete_identity() {
        use identity::{Credentials,Identities,Principal};
        let policy=Policy::new(vec![(1,"parent".into()),(2,"child".into()),(3,"other".into())],vec![]).unwrap();
        let source=policy.subject(1).unwrap();
        assert!(!source.can_transition());
        let range=PrincipalRangeTransitionRule {source:1,target:2,first:Principal(100),last:Principal(199)};
        policy.extend(1,PolicyUpdate {principal_ranges:vec![range],..Default::default()}).unwrap();
        assert!(source.can_transition());
        let mut credentials=Credentials::bootstrap();
        for principal in [0,99,100,150,199,200,u32::MAX] {
            credentials.set_principals(Identities::all(Principal(principal))).unwrap();
            assert_eq!(source.transition(2,&credentials).is_ok(),(100..=199).contains(&principal));
            assert!(source.transition(3,&credentials).is_err());
            assert!(policy.subject(2).unwrap().transition(1,&credentials).is_err());
        }
        credentials.set_principals(Identities {base:Principal(99),active:Principal(150),retained:Principal(150)}).unwrap();
        assert!(source.transition(2,&credentials).is_err());
        credentials.set_principals(Identities {base:Principal(150),active:Principal(150),retained:Principal(200)}).unwrap();
        assert!(source.transition(2,&credentials).is_err());
        let derived=policy.with_file_rules(Some(3),vec![],vec![]).unwrap().with_file_persistence([7;16]).unwrap();
        credentials.set_principals(Identities::all(Principal(199))).unwrap();
        assert_eq!(derived.subject(1).unwrap().transition(2,&credentials).unwrap().label(),2);
    }
    #[test_case]
    fn range_updates_publish_atomically_and_share_the_transition_resource_bound() {
        use identity::Principal;
        let policy=Policy::new(vec![(1,"parent".into()),(2,"child".into())],vec![]).unwrap();
        let rule=PrincipalRangeTransitionRule {source:1,target:2,first:Principal(100),last:Principal(199)};
        for invalid in [PrincipalRangeTransitionRule {first:Principal(200),..rule},
            PrincipalRangeTransitionRule {source:2,..rule},PrincipalRangeTransitionRule {target:99,..rule}] {
            assert!(policy.extend(1,PolicyUpdate {labels:vec![(3,"added".into())],principal_ranges:vec![invalid],..Default::default()}).is_err());
            assert_eq!(policy.generation(),1);assert!(policy.label(3).is_none());
        }
        assert!(policy.extend(1,PolicyUpdate {principal_ranges:vec![rule,rule],..Default::default()}).is_err());
        let single=PrincipalRangeTransitionRule {first:Principal(u32::MAX),last:Principal(u32::MAX),..rule};
        assert_eq!(policy.extend(1,PolicyUpdate {principal_ranges:vec![single],..Default::default()}),Ok(2));
        assert!(policy.extend(1,PolicyUpdate {principal_ranges:vec![rule],..Default::default()}).is_err());
        assert_eq!(policy.snapshot().principal_ranges.len(),1);
        let exact=(0..MAX_RULES as u32).map(|id|TransitionRule {source:1,target:2,principal:Principal(id)}).collect();
        let full=Policy::with_transitions(vec![(1,"parent".into()),(2,"child".into())],vec![],exact).unwrap();
        assert!(full.extend(1,PolicyUpdate {principal_ranges:vec![rule],..Default::default()}).is_err());
        assert_eq!(full.generation(),1);assert!(full.snapshot().principal_ranges.is_empty());
    }
    #[test_case]
    fn updates_publish_a_complete_generation_and_keep_subject_and_inode_identities() {
        let policy = Policy::new(
            vec![
                (1, "manager".into()),
                (2, "image".into()),
                (3, "parent".into()),
            ],
            vec![Rule {
                source: 1,
                target: 2,
                class: 7,
                permissions: 1,
            }],
        )
        .unwrap()
        .with_file_rules(Some(3), vec![], vec![])
        .unwrap();
        let subject = policy.subject(1).unwrap();
        let snapshot = policy.snapshot();
        let fs = crate::fs::VfsManager::new();
        fs.create_file("/image", crate::fs::FileType::RegularFile)
            .unwrap();
        let image = fs.resolve_path("/image").unwrap().0.node();
        policy.bind_resource(image.clone(), 2).unwrap();
        assert_eq!(
            policy.extend(
                1,
                PolicyUpdate {
                    labels: vec![(4, "application".into())],
                    rules: vec![Rule {
                        source: 1,
                        target: 2,
                        class: 7,
                        permissions: 2
                    }],
                    transitions: vec![TransitionRule {
                        source: 1,
                        target: 4,
                        principal: identity::Principal(10042)
                    }],
                    exec_transitions: vec![ExecTransitionRule {
                        source: 1,
                        resource: 2,
                        target: 4,
                        principal: identity::Principal(10042)
                    }],
                    relabel: vec![file::RelabelRule {
                        source: 1,
                        old: 3,
                        target: 4,
                        kind: 1
                    }],
                    ..Default::default()
                }
            ),
            Ok(2)
        );
        assert_eq!(subject.name(), "manager");
        assert!(Arc::ptr_eq(subject.policy(), &policy));
        assert_eq!(policy.resource_label(image.as_ref()), Some(2));
        assert_eq!(policy.allowed(1, 2, 7), 3);
        assert_eq!(snapshot.grants.get(&(1, 2, 7)), Some(&1));
        assert!(!snapshot.labels.contains_key(&4));
        let mut credentials = identity::Credentials::bootstrap();
        assert!(subject.transition(4, &credentials).is_err());
        credentials
            .set_principals(identity::Identities::all(identity::Principal(10042)))
            .unwrap();
        assert_eq!(
            subject.transition(4, &credentials).unwrap().name(),
            "application"
        );
        assert_eq!(subject.resource_exec(2, &credentials).unwrap().label(), 4);
        fs.create_file("/data", crate::fs::FileType::RegularFile)
            .unwrap();
        let node = fs.resolve_path("/data").unwrap().0.node();
        policy.relabel_file(&subject, node.as_ref(), 4).unwrap();
        assert_eq!(policy.resource_label(node.as_ref()), Some(4));
        assert!(policy.relabel_file(&subject, image.as_ref(), 4).is_err());
        assert_eq!(policy.extend(2, PolicyUpdate::default()), Ok(2));
    }
    #[test_case]
    fn stock_boot_and_manager_labels_publish_above_the_original_bound() {
        let boot = (1..=859)
            .map(|id| (id, alloc::format!("boot-{id}")))
            .collect();
        let policy = Policy::new(boot, vec![]).unwrap();
        let manager = (860..=1027)
            .map(|id| (id, alloc::format!("manager-{id}")))
            .collect();
        assert_eq!(
            policy.extend(
                1,
                PolicyUpdate {
                    labels: manager,
                    ..Default::default()
                }
            ),
            Ok(2)
        );
        assert_eq!(policy.label(1027).unwrap().as_ref(), "manager-1027");
    }
    #[test_case]
    fn rejected_stale_ambiguous_and_over_quota_updates_publish_nothing() {
        let policy = Policy::with_exec_transitions(
            vec![(1, "manager".into()), (2, "file".into()), (3, "old".into())],
            vec![],
            vec![],
            vec![ExecTransitionRule {
                source: 1,
                resource: 2,
                target: 3,
                principal: identity::Principal(7),
            }],
        )
        .unwrap();
        let original = policy.snapshot();
        assert!(policy.extend(0, PolicyUpdate::default()).is_err());
        for update in [
            PolicyUpdate {
                labels: vec![(4, "new".into())],
                rules: vec![Rule {
                    source: 1,
                    target: 999,
                    class: 1,
                    permissions: 1,
                }],
                ..Default::default()
            },
            PolicyUpdate {
                labels: vec![(2, "renamed".into())],
                ..Default::default()
            },
            PolicyUpdate {
                labels: vec![(4, "old".into())],
                ..Default::default()
            },
            PolicyUpdate {
                labels: vec![(4, "new".into())],
                exec_transitions: vec![ExecTransitionRule {
                    source: 1,
                    resource: 2,
                    target: 4,
                    principal: identity::Principal(7),
                }],
                ..Default::default()
            },
            PolicyUpdate {
                labels: (4..4 + super::MAX_LABELS as u32)
                    .map(|id| (id, alloc::format!("label-{id}")))
                    .collect(),
                ..Default::default()
            },
        ] {
            assert!(policy.extend(1, update).is_err());
            assert_eq!(policy.generation(), 1);
            assert!(Arc::ptr_eq(&original, &policy.snapshot()));
            assert!(policy.label(4).is_none());
        }
    }
}
