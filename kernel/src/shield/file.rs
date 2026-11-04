//! Policy-scoped inode labels and explicit creation/relabel rules.
use super::{Policy, Subject};
use crate::{
    fs::{FileSystemError, FileSystemErrorKind, FileType, vfs_v2::core::VfsNode},
    sync::{IrqRwSpinLock, Mutex},
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
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct FilePolicy {
    pub default: Option<u32>,
    defaults: BTreeMap<(u32, u32, u32), u32>,
    create: BTreeSet<(u32, u32, u32, u32)>,
    relabel: BTreeSet<(u32, u32, u32, u32)>,
}
impl FilePolicy {
    pub(crate) fn build(
        labels: &BTreeMap<u32, Arc<str>>,
        default: Option<u32>,
        create: Vec<CreateRule>,
        relabel: Vec<RelabelRule>,
    ) -> Result<Self, &'static str> {
        if default.is_some_and(|label| !labels.contains_key(&label)) {
            return Err("unknown default file label");
        }
        let mut result = Self {
            default,
            ..Self::default()
        };
        result.add(labels, create, relabel)?;
        Ok(result)
    }
    pub(super) fn add(
        &mut self,
        labels: &BTreeMap<u32, Arc<str>>,
        create: Vec<CreateRule>,
        relabel: Vec<RelabelRule>,
    ) -> Result<(), &'static str> {
        if create.len() > super::MAX_RULES || relabel.len() > super::MAX_RULES {
            return Err("file rule bound");
        }
        if self.default.is_none() && (!create.is_empty() || !relabel.is_empty()) {
            return Err("file rules require a default label");
        }
        let mut seen_create = BTreeSet::new();
        let mut seen_relabel = BTreeSet::new();
        for rule in create {
            let key = (rule.source, rule.parent, rule.kind, rule.target);
            if !(1..=7).contains(&rule.kind)
                || [rule.source, rule.parent, rule.target]
                    .iter()
                    .any(|id| !labels.contains_key(id))
                || !seen_create.insert(key)
            {
                return Err("invalid create rule");
            }
            self.create.insert(key);
            if rule.default {
                let key = (rule.source, rule.parent, rule.kind);
                if self
                    .defaults
                    .get(&key)
                    .is_some_and(|old| *old != rule.target)
                {
                    return Err("ambiguous default create rule");
                }
                self.defaults.insert(key, rule.target);
            }
        }
        for rule in relabel {
            let key = (rule.source, rule.old, rule.kind, rule.target);
            if !(1..=7).contains(&rule.kind)
                || [rule.source, rule.old, rule.target]
                    .iter()
                    .any(|id| !labels.contains_key(id))
                || !seen_relabel.insert(key)
            {
                return Err("invalid relabel rule");
            }
            self.relabel.insert(key);
        }
        if self.create.len() > super::MAX_RULES || self.relabel.len() > super::MAX_RULES {
            return Err("complete file rule bound");
        }
        Ok(())
    }
}
/// Maximum number of stable metadata domains retained by one inode's RAM
/// fallback. Persistent backends enforce the same bound through the policy
/// configuration and their xattr storage.
pub const MAX_LABEL_DOMAINS: usize = 16;
pub const MAX_LABEL_METADATA_BYTES: usize =
    scarlet_abi::shield::FILE_LABEL_METADATA_MAX_BYTES;
const MAX_POLICY_BINDINGS: usize = 16;

#[derive(Default)]
struct LabelState {
    policies: BTreeMap<usize, (Weak<Policy>, u32)>,
    metadata: BTreeMap<[u8; 16], Vec<u8>>,
}

/// Owned by an inode, shared by hard links and retained open files. Weak policy
/// identities prevent cycles with policies that retain administrator-bound nodes.
pub struct Labels {
    state: IrqRwSpinLock<LabelState>,
    mutation: Mutex<()>,
}
impl Default for Labels {
    fn default() -> Self {
        Self {
            state: IrqRwSpinLock::new(LabelState::default()),
            mutation: Mutex::new(()),
        }
    }
}
impl core::fmt::Debug for Labels {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let state = self.state.read();
        f.debug_struct("FileLabels")
            .field("policies", &state.policies.len())
            .field("domains", &state.metadata.len())
            .finish()
    }
}
impl Labels {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub fn get(&self, policy: &Policy) -> Option<u32> {
        self.state
            .read()
            .policies
            .get(&(policy as *const Policy as usize))
            .map(|(_, label)| *label)
    }
    fn assign(&self, policy: &Arc<Policy>, label: u32, replace: bool) -> Result<(), &'static str> {
        let mut state = self.state.write();
        state.policies.retain(|_, (policy, _)| policy.strong_count() != 0);
        let key = Arc::as_ptr(policy) as usize;
        if let Some((_, old)) = state.policies.get(&key) {
            if !replace && *old != label {
                return Err("file already labeled");
            }
        } else if state.policies.len() >= MAX_POLICY_BINDINGS {
            return Err("inode policy bound");
        }
        state.policies.insert(key, (Arc::downgrade(policy), label));
        Ok(())
    }

    pub fn read_label_metadata(
        &self,
        domain: &[u8; 16],
    ) -> Result<Option<Vec<u8>>, FileSystemError> {
        Ok(self.state.read().metadata.get(domain).cloned())
    }

    pub fn write_label_metadata(
        &self,
        domain: &[u8; 16],
        value: &[u8],
    ) -> Result<(), FileSystemError> {
        if value.len() > MAX_LABEL_METADATA_BYTES {
            return Err(FileSystemError::new(
                FileSystemErrorKind::ValueOverflow,
                "Persistent file label is too large",
            ));
        }
        let mut state = self.state.write();
        if !state.metadata.contains_key(domain) && state.metadata.len() >= MAX_LABEL_DOMAINS {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NoSpace,
                "Inode persistent label domain bound",
            ));
        }
        state.metadata.insert(*domain, value.to_vec());
        Ok(())
    }

    /// Serialize a complete read/check/write transaction with a sleepable
    /// per-inode lock. Disk-backed metadata implementations must keep I/O in
    /// this closure and out of `state`'s IRQ spin lock.
    pub(crate) fn transaction<T, E>(
        &self,
        operation: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        let _guard = self.mutation.lock();
        operation()
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
        if subject.policy().default_file_label().is_none() {
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
                || !policy.snapshot().creation_allowed(subject.label(), parent, kind, selected.target)
            {
                return Err("file create override denied");
            }
            selected.target
        } else {
            *policy
                .snapshot()
                .files
                .defaults
                .get(&(subject.label(), parent, kind))
                .unwrap_or(&parent)
        };
        Ok(Some(Self { policy, target }))
    }
    pub fn initialize(&self, node: &dyn VfsNode) -> Result<(), &'static str> {
        let labels = node.shield_labels()
            .ok_or("filesystem cannot retain file labels")?;
        if let Some(domain) = self.policy.file_persistence_domain() {
            let name = self.policy.label(self.target).ok_or("unknown file label")?;
            let value = encode_persistent_label(name.as_ref())?;
            return node
                .write_label_metadata(&domain, &value)
                .map_err(|_| "persistent file label write failed");
        }
        labels.assign(&self.policy, self.target, false)
    }
}

fn encode_persistent_label(name: &str) -> Result<Vec<u8>, &'static str> {
    let mut value = [0u8; MAX_LABEL_METADATA_BYTES];
    let length = scarlet_abi::shield::encode_file_label(name, &mut value)
        .ok_or("invalid persistent file label")?;
    Ok(value[..length].to_vec())
}

impl Policy {
    pub fn file_persistence_domain(&self) -> Option<[u8; 16]> {
        self.snapshot().file_persistence
    }

    /// Derive a fresh policy identity that stores file labels as symbolic names
    /// in the shared stable domain. The management capability is checked by the
    /// native syscall; derivation itself does not grant authority.
    pub fn with_file_persistence(
        &self,
        domain: [u8; 16],
    ) -> Result<Arc<Self>, &'static str> {
        if domain == [0; 16] {
            return Err("file persistence domain must be nonzero");
        }
        let mut data = (*self.snapshot()).clone();
        data.file_persistence = Some(domain);
        data.generation = 1;
        Ok(Self::from_data(data))
    }

    pub(crate) fn persisted_label_name(
        &self,
        node: &dyn VfsNode,
        domain: &[u8; 16],
    ) -> Result<Option<Arc<str>>, &'static str> {
        let Some(value) = node
            .read_label_metadata(domain)
            .map_err(|_| "persistent file label unavailable")?
        else {
            return Ok(None);
        };
        let name = scarlet_abi::shield::decode_file_label(&value)
            .ok_or("malformed persistent file label")?;
        Ok(Some(Arc::from(name)))
    }

    pub fn default_file_label(&self) -> Option<u32> {
        self.snapshot().files.default
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
        if let Some(domain) = self.file_persistence_domain() {
            return labels.transaction(|| {
                if let Some(old) = self.bound_resource_label(node) {
                    return if old == target {
                        Ok(())
                    } else {
                        Err("administrator-pinned file label")
                    };
                }
                let persisted = self.persisted_label_name(node, &domain)?;
                let old = match persisted.as_ref() {
                    Some(name) => self
                        .label_id(name)
                        .ok_or("unknown persistent file label")?,
                    None => self
                        .snapshot()
                        .files
                        .default
                        .ok_or("file label missing")?,
                };
                if old != target
                    && !self
                        .snapshot()
                        .relabel_allowed(subject.label(), old, kind, target)
                {
                    return Err("file relabel denied");
                }
                let target_name = self.label(target).ok_or("unknown file label")?;
                if persisted.as_deref() == Some(target_name.as_ref()) {
                    return Ok(());
                }
                let value = encode_persistent_label(target_name.as_ref())?;
                node.write_label_metadata(&domain, &value)
                    .map_err(|_| "persistent file label write failed")
            });
        }
        let mut state = labels.state.write();
        state
            .policies
            .retain(|_, (policy, _)| policy.strong_count() != 0);
        let key = Arc::as_ptr(self) as usize;
        let old = state
            .policies
            .get(&key)
            .map(|(_, label)| *label)
            .or(self.snapshot().files.default)
            .ok_or("file label missing")?;
        if old == target {
            return Ok(());
        }
        if !self.snapshot().relabel_allowed(subject.label(), old, kind, target)
        {
            return Err("file relabel denied");
        }
        if !state.policies.contains_key(&key) && state.policies.len() >= MAX_POLICY_BINDINGS {
            return Err("inode policy bound");
        }
        state.policies.insert(key, (Arc::downgrade(self), target));
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
        if !policy.snapshot().creation_delegated(subject.label(), target)
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

impl super::PolicyData {
    fn creation_allowed(&self, source:u32, parent:u32, kind:u32, target:u32)->bool {
        self.files.create.contains(&(source,parent,kind,target))
            || self.sets.file_allowed(source,parent,target,kind,super::sets::FileAction::Create)
    }
    fn relabel_allowed(&self, source:u32, old:u32, kind:u32, target:u32)->bool {
        self.files.relabel.contains(&(source,old,kind,target))
            || self.sets.file_allowed(source,old,target,kind,super::sets::FileAction::Relabel)
    }
    fn creation_delegated(&self, source:u32, target:u32)->bool {
        self.files.create.iter().any(|&(s,_,_,t)|s==source && t==target)
            || self.sets.creation_delegated(source,target)
    }
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
    fn shared_file_rules_reach_vfs_creation_and_relabel_checks_without_changing_defaults() {
        use super::super::{sets::{Update,Selector,FileRule,FileAction},update::PolicyUpdate};
        let policy=policy();let subject=policy.subject(1).unwrap();
        let task=Arc::new(Task::new("set-file-label".into(),1,TaskType::User));
        *task.security_subject.write()=Some(subject.clone());
        assert!(select_creation_label(&task,Some(5)).is_err());
        policy.extend(1,PolicyUpdate {sets:Update {sets:vec![9],members:vec![(9,5)],files:vec![
            FileRule {source:Selector::Label(1),origin:Selector::Label(2),target:Selector::Set(9),kind:1,action:FileAction::Create,different:false},
            FileRule {source:Selector::Label(1),origin:Selector::Set(9),target:Selector::Label(4),kind:1,action:FileAction::Relabel,different:true},
        ],..Default::default()},..Default::default()}).unwrap();
        let fs=VfsManager::new();set_mock_current_task(task.clone());
        select_creation_label(&task,Some(5)).unwrap();fs.create_file("/selected",FileType::RegularFile).unwrap();
        let node=fs.resolve_path("/selected").unwrap().0.node();assert_eq!(policy.resource_label(node.as_ref()),Some(5));
        policy.relabel_file(&subject,node.as_ref(),4).unwrap();assert_eq!(policy.resource_label(node.as_ref()),Some(4));
        assert!(policy.relabel_file(&subject,node.as_ref(),5).is_err());
        select_creation_label(&task,None).unwrap();fs.create_file("/default",FileType::RegularFile).unwrap();
        assert_eq!(policy.resource_label(fs.resolve_path("/default").unwrap().0.node().as_ref()),Some(3));
        let derived=policy.with_file_rules(Some(2),vec![],vec![]).unwrap();
        *task.security_subject.write()=Some(derived.subject(1).unwrap());
        assert!(select_creation_label(&task,Some(5)).is_err());clear_mock_current_task();
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

    #[test_case]
    fn persistent_file_labels_survive_policy_id_remapping_and_are_domain_scoped() {
        let domain = [0x42; 16];
        let other_domain = [0x24; 16];
        let original = Policy::new(
            vec![(1, "actor".into()), (2, "base".into()), (3, "private".into())],
            vec![],
        )
        .unwrap()
        .with_file_rules(Some(2), vec![], vec![])
        .unwrap()
        .with_file_persistence(domain)
        .unwrap();
        let fs = VfsManager::new();
        fs.create_file("/persistent", FileType::RegularFile).unwrap();
        let node = fs.resolve_path("/persistent").unwrap().0.node();
        CreateContext {
            policy: original.clone(),
            target: 3,
        }
        .initialize(node.as_ref())
        .unwrap();
        assert_eq!(
            node.read_label_metadata(&domain).unwrap(),
            Some(b"SLB1private".to_vec())
        );
        assert_eq!(node.read_label_metadata(&other_domain).unwrap(), None);
        assert_eq!(original.resource_label(node.as_ref()), Some(3));

        let recreated = Policy::new(
            vec![(1, "actor".into()), (2, "base".into()), (9, "private".into())],
            vec![],
        )
        .unwrap()
        .with_file_rules(Some(2), vec![], vec![])
        .unwrap()
        .with_file_persistence(domain)
        .unwrap();
        assert_eq!(recreated.resource_label(node.as_ref()), Some(9));
        assert_eq!(recreated.resource_label_name(node.as_ref()).as_deref(), Some("private"));
        let isolated = recreated.with_file_persistence(other_domain).unwrap();
        assert_eq!(isolated.resource_label(node.as_ref()), Some(2));
        assert_eq!(recreated.resource_label(node.as_ref()), Some(9));
    }

    #[test_case]
    fn persistent_policy_creation_and_relabel_commit_symbolic_names() {
        let domain = [0x5a; 16];
        let policy = policy().with_file_persistence(domain).unwrap();
        let subject = policy.subject(1).unwrap();
        let task = Arc::new(Task::new("persistent-label".into(), 1, TaskType::User));
        *task.security_subject.write() = Some(subject.clone());
        set_mock_current_task(task);
        let fs = VfsManager::new();
        fs.create_file("/created", FileType::RegularFile).unwrap();
        let node = fs.resolve_path("/created").unwrap().0.node();
        assert_eq!(
            node.read_label_metadata(&domain).unwrap(),
            Some(b"SLB1private".to_vec())
        );
        policy.relabel_file(&subject, node.as_ref(), 4).unwrap();
        assert_eq!(
            node.read_label_metadata(&domain).unwrap(),
            Some(b"SLB1public".to_vec())
        );
        assert_eq!(policy.resource_label(node.as_ref()), Some(4));
        fs.create_file("/pinned", FileType::RegularFile).unwrap();
        let pinned = fs.resolve_path("/pinned").unwrap().0.node();
        policy.bind_resource(pinned.clone(), 3).unwrap();
        assert!(policy.relabel_file(&subject, pinned.as_ref(), 4).is_err());
        assert_eq!(
            pinned.read_label_metadata(&domain).unwrap(),
            Some(b"SLB1private".to_vec())
        );
        clear_mock_current_task();
    }

    #[test_case]
    fn malformed_or_unknown_persistent_names_fail_closed_and_public_xattrs_cannot_spoof_them() {
        use crate::fs::vfs_v2::xattr::{self, SetMode};
        let domain = [0x73; 16];
        let policy = Policy::new(
            vec![(1, "actor".into()), (2, "base".into()), (3, "private".into())],
            vec![],
        )
        .unwrap()
        .with_file_rules(Some(2), vec![], vec![])
        .unwrap()
        .with_file_persistence(domain)
        .unwrap();
        let fs = VfsManager::new();
        fs.create_file("/file", FileType::RegularFile).unwrap();
        let node = fs.resolve_path("/file").unwrap().0.node();
        let key = xattr::label_metadata_key(&domain);
        assert_eq!(
            node.set_xattr(&key, b"SLB1private", SetMode::Upsert),
            Err(xattr::Error::PermissionDenied)
        );
        assert_eq!(node.list_xattrs().unwrap(), Vec::<Vec<u8>>::new());

        node.write_label_metadata(&domain, b"SLB1unregistered").unwrap();
        assert_eq!(policy.resource_label(node.as_ref()), None);
        assert_eq!(
            policy.resource_label_name(node.as_ref()).as_deref(),
            Some("unregistered")
        );

        node.write_label_metadata(&domain, b"3").unwrap();
        assert_eq!(policy.resource_label(node.as_ref()), None);
        assert_eq!(policy.resource_label_name(node.as_ref()), None);
    }

    #[test_case]
    fn file_persistence_domain_is_carried_by_derived_and_updated_policy_snapshots() {
        use super::super::update::PolicyUpdate;
        let domain = [0xa5; 16];
        let policy = policy().with_file_persistence(domain).unwrap();
        assert_eq!(policy.snapshot().file_persistence, Some(domain));
        let derived = policy.with_file_rules(Some(2), vec![], vec![]).unwrap();
        assert_eq!(derived.snapshot().file_persistence, Some(domain));
        derived
            .extend(
                1,
                PolicyUpdate {
                    labels: vec![(7, "additional".into())],
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(derived.snapshot().file_persistence, Some(domain));
        assert!(policy.with_file_persistence([0; 16]).is_err());
    }

    #[test_case]
    fn ram_label_metadata_is_bounded_and_replaces_existing_domains() {
        let labels = Labels::new();
        for domain in 0..MAX_LABEL_DOMAINS as u8 {
            let mut key = [0u8; 16];
            key[0] = domain + 1;
            labels
                .write_label_metadata(&key, b"SLB1label")
                .unwrap();
        }
        let mut existing = [0u8; 16];
        existing[0] = 1;
        labels
            .write_label_metadata(&existing, b"SLB1replacement")
            .unwrap();
        assert_eq!(
            labels.read_label_metadata(&existing).unwrap(),
            Some(b"SLB1replacement".to_vec())
        );
        let extra = [0xff; 16];
        assert_eq!(
            labels.write_label_metadata(&extra, b"SLB1extra").unwrap_err().kind,
            FileSystemErrorKind::NoSpace
        );
        assert_eq!(
            labels
                .write_label_metadata(&existing, &[0; 513])
                .unwrap_err()
                .kind,
            FileSystemErrorKind::ValueOverflow
        );
    }
}
