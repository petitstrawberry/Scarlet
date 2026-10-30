//! Policy-scoped inode labels and explicit creation/relabel rules.
use super::{Policy, Subject};
use crate::{
    fs::{FileType, vfs_v2::core::VfsNode},
    sync::IrqRwSpinLock,
    task::Task,
};
use alloc::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Weak},
    vec::Vec,
};

#[derive(Clone, Copy)]
pub struct CreateRule {
    pub source: u32,
    pub parent: u32,
    pub target: u32,
    pub kind: u32,
    pub default: bool,
}
#[derive(Clone, Copy)]
pub struct RelabelRule {
    pub source: u32,
    pub old: u32,
    pub target: u32,
    pub kind: u32,
}
#[derive(Default)]
pub(crate) struct FilePolicy {
    pub default: Option<u32>,
    defaults: BTreeMap<(u32, u32, u32), u32>,
    create: BTreeSet<(u32, u32, u32, u32)>,
    relabel: BTreeSet<(u32, u32, u32, u32)>,
}
impl FilePolicy {
    pub(crate) fn build(
        policy: &Policy,
        default: Option<u32>,
        create: Vec<CreateRule>,
        relabel: Vec<RelabelRule>,
    ) -> Result<Self, &'static str> {
        if create.len() > super::MAX_RULES || relabel.len() > super::MAX_RULES {
            return Err("file rule bound");
        }
        if default.is_some_and(|label| policy.label(label).is_none()) {
            return Err("unknown default file label");
        }
        if default.is_none() && (!create.is_empty() || !relabel.is_empty()) {
            return Err("file rules require a default label");
        }
        let mut result = Self {
            default,
            ..Self::default()
        };
        for rule in create {
            if !(1..=7).contains(&rule.kind)
                || [rule.source, rule.parent, rule.target]
                    .iter()
                    .any(|id| policy.label(*id).is_none())
                || !result
                    .create
                    .insert((rule.source, rule.parent, rule.kind, rule.target))
            {
                return Err("invalid create rule");
            }
            if rule.default
                && result
                    .defaults
                    .insert((rule.source, rule.parent, rule.kind), rule.target)
                    .is_some()
            {
                return Err("ambiguous default create rule");
            }
        }
        for rule in relabel {
            if !(1..=7).contains(&rule.kind)
                || [rule.source, rule.old, rule.target]
                    .iter()
                    .any(|id| policy.label(*id).is_none())
                || !result
                    .relabel
                    .insert((rule.source, rule.old, rule.kind, rule.target))
            {
                return Err("invalid relabel rule");
            }
        }
        Ok(result)
    }
}
/// Owned by an inode, shared by hard links and retained open files. Weak policy
/// identities prevent cycles with policies that retain administrator-bound nodes.
pub struct Labels {
    entries: IrqRwSpinLock<BTreeMap<usize, (Weak<Policy>, u32)>>,
}
impl Default for Labels {
    fn default() -> Self {
        Self {
            entries: IrqRwSpinLock::new(BTreeMap::new()),
        }
    }
}
impl core::fmt::Debug for Labels {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FileLabels")
            .field("policies", &self.entries.read().len())
            .finish()
    }
}
impl Labels {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub fn get(&self, policy: &Policy) -> Option<u32> {
        self.entries
            .read()
            .get(&(policy as *const Policy as usize))
            .map(|(_, label)| *label)
    }
    fn assign(&self, policy: &Arc<Policy>, label: u32, replace: bool) -> Result<(), &'static str> {
        let mut entries = self.entries.write();
        entries.retain(|_, (policy, _)| policy.strong_count() != 0);
        let key = Arc::as_ptr(policy) as usize;
        if let Some((_, old)) = entries.get(&key) {
            if !replace && *old != label {
                return Err("file already labeled");
            }
        } else if entries.len() >= 16 {
            return Err("inode policy bound");
        }
        entries.insert(key, (Arc::downgrade(policy), label));
        Ok(())
    }
}
pub fn kind(file_type: &FileType) -> Result<u32, &'static str> {
    Ok(match file_type {
        FileType::RegularFile => 1,
        FileType::Directory => 2,
        FileType::SymbolicLink(_) => 3,
        FileType::CharDevice(_) => 4,
        FileType::BlockDevice(_) => 5,
        FileType::Socket(_) => 6,
        FileType::Pipe => 7,
        _ => return Err("unlabelable file kind"),
    })
}
#[derive(Clone)]
pub struct CreationLabel {
    pub policy: Arc<Policy>,
    pub source: u32,
    pub target: u32,
}
#[derive(Clone)]
pub struct CreateContext {
    policy: Arc<Policy>,
    target: u32,
}
impl CreateContext {
    pub fn prepare(
        task: &Task,
        parent: &dyn VfsNode,
        file_type: &FileType,
    ) -> Result<Option<Self>, &'static str> {
        let Some(subject) = task.security_subject.read().clone() else {
            return Ok(None);
        };
        if subject.policy().files.default.is_none() {
            return Ok(None);
        }
        let parent = subject
            .policy()
            .resource_label(parent)
            .ok_or("parent file label missing")?;
        let kind = kind(file_type)?;
        let policy = subject.policy().clone();
        let selected = task.creation_label.read().clone();
        let target = if let Some(selected) = selected {
            if !Arc::ptr_eq(&selected.policy, &policy)
                || selected.source != subject.label()
                || !policy
                    .files
                    .create
                    .contains(&(subject.label(), parent, kind, selected.target))
            {
                return Err("file create override denied");
            }
            selected.target
        } else {
            *policy
                .files
                .defaults
                .get(&(subject.label(), parent, kind))
                .unwrap_or(&parent)
        };
        Ok(Some(Self { policy, target }))
    }
    pub fn initialize(&self, node: &dyn VfsNode) -> Result<(), &'static str> {
        node.shield_labels()
            .ok_or("filesystem cannot retain file labels")?
            .assign(&self.policy, self.target, false)
    }
}
impl Policy {
    pub fn default_file_label(&self) -> Option<u32> {
        self.files.default
    }
    pub fn relabel_file(
        self: &Arc<Self>,
        subject: &Subject,
        node: &dyn VfsNode,
        target: u32,
    ) -> Result<(), &'static str> {
        if !core::ptr::eq(subject.policy().as_ref(), self.as_ref()) || self.label(target).is_none()
        {
            return Err("foreign file policy or label");
        }
        if let Some(old) = self.bound_resource_label(node) {
            return if old == target {
                Ok(())
            } else {
                Err("administrator-pinned file label")
            };
        }
        let labels = node
            .shield_labels()
            .ok_or("filesystem cannot retain file labels")?;
        let kind = kind(&node.file_type().map_err(|_| "file kind unavailable")?)?;
        let mut entries = labels.entries.write();
        entries.retain(|_, (policy, _)| policy.strong_count() != 0);
        let key = Arc::as_ptr(self) as usize;
        let old = entries
            .get(&key)
            .map(|(_, label)| *label)
            .or(self.files.default)
            .ok_or("file label missing")?;
        if old == target {
            return Ok(());
        }
        if !self
            .files
            .relabel
            .contains(&(subject.label(), old, kind, target))
        {
            return Err("file relabel denied");
        }
        if !entries.contains_key(&key) && entries.len() >= 16 {
            return Err("inode policy bound");
        }
        entries.insert(key, (Arc::downgrade(self), target));
        Ok(())
    }
}
pub fn select_creation_label(task: &Task, target: Option<u32>) -> Result<(), &'static str> {
    if let Some(target) = target {
        let subject = task
            .security_subject
            .read()
            .clone()
            .ok_or("subject missing")?;
        let policy = subject.policy();
        if !policy
            .files
            .create
            .iter()
            .any(|&(source, _, _, label)| source == subject.label() && label == target)
        {
            return Err("file creation label not delegated");
        }
        *task.creation_label.write() = Some(CreationLabel {
            policy: policy.clone(),
            source: subject.label(),
            target,
        });
    } else {
        *task.creation_label.write() = None;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        fs::VfsManager,
        task::{TaskType, clear_mock_current_task, set_mock_current_task},
    };
    use alloc::vec;
    fn policy() -> Arc<Policy> {
        Policy::new(
            vec![
                (1, "actor".into()),
                (2, "base".into()),
                (3, "private".into()),
                (4, "public".into()),
                (5, "foreign".into()),
                (6, "parent".into()),
            ],
            vec![],
        )
        .unwrap()
        .with_file_rules(
            Some(2),
            vec![
                CreateRule {
                    source: 1,
                    parent: 2,
                    target: 3,
                    kind: 1,
                    default: true,
                },
                CreateRule {
                    source: 1,
                    parent: 2,
                    target: 4,
                    kind: 1,
                    default: false,
                },
            ],
            vec![
                RelabelRule {
                    source: 1,
                    old: 3,
                    target: 4,
                    kind: 1,
                },
                RelabelRule {
                    source: 1,
                    old: 2,
                    target: 6,
                    kind: 2,
                },
            ],
        )
        .unwrap()
    }
    #[test_case]
    fn labels_follow_inode_lifetime_and_creation_overrides_recheck_parent() {
        let policy = policy();
        let subject = policy.subject(1).unwrap();
        let fs = VfsManager::new();
        let task = Arc::new(Task::new("file-label".into(), 1, TaskType::User));
        *task.security_subject.write() = Some(subject.clone());
        set_mock_current_task(task.clone());
        fs.create_file("/image", FileType::RegularFile).unwrap();
        let original = fs.resolve_path("/image").unwrap().0.node();
        assert_eq!(policy.resource_label(original.as_ref()), Some(3));
        assert!(policy.relabel_file(&subject, original.as_ref(), 5).is_err());
        policy.relabel_file(&subject, original.as_ref(), 4).unwrap();
        fs.create_hardlink("/image", "/alias").unwrap();
        fs.rename("/image", "/renamed").unwrap();
        assert_eq!(
            policy.resource_label(fs.resolve_path("/alias").unwrap().0.node().as_ref()),
            Some(4)
        );
        fs.remove("/alias").unwrap();
        fs.remove("/renamed").unwrap();
        fs.create_file("/image", FileType::RegularFile).unwrap();
        assert_eq!(policy.resource_label(original.as_ref()), Some(4));
        assert_eq!(
            policy.resource_label(fs.resolve_path("/image").unwrap().0.node().as_ref()),
            Some(3)
        );
        select_creation_label(&task, Some(4)).unwrap();
        fs.create_file("/chosen", FileType::RegularFile).unwrap();
        assert_eq!(
            policy.resource_label(fs.resolve_path("/chosen").unwrap().0.node().as_ref()),
            Some(4)
        );
        select_creation_label(&task, None).unwrap();
        fs.create_dir("/directory").unwrap();
        let parent = fs.resolve_path("/directory").unwrap().0.node();
        policy.relabel_file(&subject, parent.as_ref(), 6).unwrap();
        select_creation_label(&task, Some(4)).unwrap();
        assert!(
            fs.create_file("/directory/denied", FileType::RegularFile)
                .is_err()
        );
        assert!(fs.resolve_path("/directory/denied").is_err());
        select_creation_label(&task, None).unwrap();
        fs.create_file("/directory/inherited", FileType::RegularFile)
            .unwrap();
        assert_eq!(
            policy.resource_label(
                fs.resolve_path("/directory/inherited")
                    .unwrap()
                    .0
                    .node()
                    .as_ref()
            ),
            Some(6)
        );
        assert!(select_creation_label(&task, Some(5)).is_err());
        clear_mock_current_task();
    }
    #[test_case]
    fn relabel_rechecks_current_inode_label_and_preserves_administrator_bindings() {
        let policy = policy();
        let subject = policy.subject(1).unwrap();
        let fs = VfsManager::new();
        fs.create_file("/file", FileType::RegularFile).unwrap();
        let node = fs.resolve_path("/file").unwrap().0.node();
        let context = CreateContext {
            policy: policy.clone(),
            target: 3,
        };
        context.initialize(node.as_ref()).unwrap();
        policy.relabel_file(&subject, node.as_ref(), 4).unwrap();
        assert!(policy.relabel_file(&subject, node.as_ref(), 3).is_err());
        assert_eq!(policy.resource_label(node.as_ref()), Some(4));
        let foreign = policy.with_file_rules(Some(2), vec![], vec![]).unwrap();
        assert_eq!(foreign.resource_label(node.as_ref()), Some(2));
        assert!(
            policy
                .relabel_file(&foreign.subject(1).unwrap(), node.as_ref(), 4)
                .is_err()
        );
        policy.bind_resource(node.clone(), 3).unwrap();
        assert_eq!(policy.resource_label(node.as_ref()), Some(3));
        assert!(policy.relabel_file(&subject, node.as_ref(), 4).is_err());
        assert!(policy.with_file_rules(Some(99), vec![], vec![]).is_err());
        assert!(
            policy
                .with_file_rules(
                    Some(2),
                    vec![
                        CreateRule {
                            source: 1,
                            parent: 2,
                            target: 3,
                            kind: 1,
                            default: true
                        },
                        CreateRule {
                            source: 1,
                            parent: 2,
                            target: 4,
                            kind: 1,
                            default: true
                        }
                    ],
                    vec![]
                )
                .is_err()
        );
    }
}
