//! Bounded additions to a stable policy identity. Only the native management
//! capability exposes publication; subjects and current-policy handles cannot.
use super::*;

#[derive(Default)]
pub struct PolicyUpdate {
    pub labels: Vec<(u32, String)>,
    pub rules: Vec<Rule>,
    pub transitions: Vec<TransitionRule>,
    pub exec_transitions: Vec<ExecTransitionRule>,
    pub create: Vec<file::CreateRule>,
    pub relabel: Vec<file::RelabelRule>,
}

impl PolicyData {
    pub(super) fn add(&mut self, update: PolicyUpdate) -> Result<(), &'static str> {
        if update.labels.len() > MAX_LABELS
            || update.rules.len() > MAX_RULES
            || update.transitions.len() > MAX_RULES
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
        if self.labels.len() > MAX_LABELS
            || self.grants.len() > MAX_RULES
            || self.transitions.len() > MAX_RULES
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
