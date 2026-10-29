//! Hierarchical process membership, independent of lifecycle scopes and namespaces.
//! No CPU or memory controller is advertised by this primitive.
use crate::sync::{IrqRwSpinLock, Mutex, MutexGuard, Once, sequence::IdSequence};
use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    sync::{Arc, Weak},
    vec::Vec,
};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static TRANSACTION: Mutex<()> = Mutex::new(());
static IDS: IdSequence = IdSequence::new();
fn transaction() -> Result<MutexGuard<'static, ()>, &'static str> {
    if let Some(guard) = TRANSACTION.try_lock() {
        Ok(guard)
    } else if crate::sync::preemptible() {
        Ok(TRANSACTION.lock())
    } else {
        Err("resource group transaction busy")
    }
}
struct Node {
    parent: Option<u64>,
    name: String,
    children: BTreeMap<String, u64>,
    members: BTreeMap<u64, Weak<Member>>,
}
struct Hierarchy {
    nodes: IrqRwSpinLock<BTreeMap<u64, Node>>,
}
#[derive(Clone)]
pub struct ResourceGroup {
    hierarchy: Arc<Hierarchy>,
    id: u64,
}
struct Member {
    id: u64,
    group: IrqRwSpinLock<Arc<ResourceGroup>>,
    threads: AtomicUsize,
}
/// One lease per task; threads share a process membership. Fork creates a new
/// membership at its parent's current group. Exec retains the original lease.
pub struct Membership {
    member: Arc<Member>,
    active: AtomicBool,
}

impl ResourceGroup {
    pub fn new_root() -> Arc<Self> {
        let id = IDS.reserve().expect("resource IDs exhausted").get();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            id,
            Node {
                parent: None,
                name: String::new(),
                children: BTreeMap::new(),
                members: BTreeMap::new(),
            },
        );
        Arc::new(Self {
            hierarchy: Arc::new(Hierarchy {
                nodes: IrqRwSpinLock::new(nodes),
            }),
            id,
        })
    }
    pub fn identity(&self) -> u64 {
        self.id
    }
    pub fn same_group(&self, other: &Self) -> bool {
        self.id == other.id && Arc::ptr_eq(&self.hierarchy, &other.hierarchy)
    }
    pub fn child(&self, name: &str) -> Option<Arc<Self>> {
        let id = *self
            .hierarchy
            .nodes
            .read()
            .get(&self.id)?
            .children
            .get(name)?;
        Some(Arc::new(Self {
            hierarchy: self.hierarchy.clone(),
            id,
        }))
    }
    pub fn create(&self, name: &str) -> Result<Arc<Self>, &'static str> {
        if name.is_empty()
            || name.len() > 255
            || matches!(name, "." | "..")
            || name
                .bytes()
                .any(|byte| byte == b'/' || byte == 0 || byte < 32)
        {
            return Err("invalid resource group name");
        }
        let _transaction = transaction()?;
        let mut nodes = self.hierarchy.nodes.write();
        if nodes.len() >= 4096 {
            return Err("resource group bound");
        }
        let parent = nodes.get(&self.id).ok_or("resource group removed")?;
        if parent.children.contains_key(name) {
            return Err("resource group exists");
        }
        let mut depth = 0;
        let mut ancestor = Some(self.id);
        while let Some(id) = ancestor {
            depth += 1;
            if depth >= 64 {
                return Err("resource group depth bound");
            }
            ancestor = nodes.get(&id).and_then(|node| node.parent);
        }
        let id = IDS.reserve().ok_or("resource IDs exhausted")?.get();
        nodes.insert(
            id,
            Node {
                parent: Some(self.id),
                name: name.to_string(),
                children: BTreeMap::new(),
                members: BTreeMap::new(),
            },
        );
        nodes
            .get_mut(&self.id)
            .unwrap()
            .children
            .insert(name.to_string(), id);
        Ok(Arc::new(Self {
            hierarchy: self.hierarchy.clone(),
            id,
        }))
    }
    pub fn parent(&self) -> Option<Arc<Self>> {
        let id = self.hierarchy.nodes.read().get(&self.id)?.parent?;
        Some(Arc::new(Self {
            hierarchy: self.hierarchy.clone(),
            id,
        }))
    }
    pub fn exists(&self) -> bool {
        self.hierarchy.nodes.read().contains_key(&self.id)
    }
    pub fn children(&self) -> Vec<(String, Arc<Self>)> {
        let nodes = self.hierarchy.nodes.read();
        nodes
            .get(&self.id)
            .map(|node| {
                node.children
                    .iter()
                    .map(|(name, &id)| {
                        (
                            name.clone(),
                            Arc::new(Self {
                                hierarchy: self.hierarchy.clone(),
                                id,
                            }),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn remove(&self) -> Result<(), &'static str> {
        let _transaction = transaction()?;
        let mut nodes = self.hierarchy.nodes.write();
        let node = nodes.get(&self.id).ok_or("resource group removed")?;
        let parent = node.parent.ok_or("cannot remove resource root")?;
        if !node.children.is_empty()
            || node
                .members
                .values()
                .filter_map(Weak::upgrade)
                .any(|member| member.threads.load(Ordering::Acquire) != 0)
        {
            return Err("resource group busy");
        }
        let name = node.name.clone();
        nodes.remove(&self.id);
        nodes.get_mut(&parent).unwrap().children.remove(&name);
        Ok(())
    }
    pub fn process_count(&self, recursive: bool) -> usize {
        let nodes = self.hierarchy.nodes.read();
        let mut pending = alloc::vec![self.id];
        let mut count = 0;
        while let Some(id) = pending.pop() {
            if let Some(node) = nodes.get(&id) {
                count += node
                    .members
                    .values()
                    .filter_map(Weak::upgrade)
                    .filter(|member| {
                        let location = member.group.read();
                        member.threads.load(Ordering::Acquire) != 0
                            && location.id == id
                            && Arc::ptr_eq(&location.hierarchy, &self.hierarchy)
                    })
                    .count();
                if recursive {
                    pending.extend(node.children.values().copied());
                }
            }
        }
        count
    }
    pub fn contains(&self, other: &Self) -> bool {
        if !Arc::ptr_eq(&self.hierarchy, &other.hierarchy) {
            return false;
        }
        let nodes = self.hierarchy.nodes.read();
        let mut current = Some(other.id);
        while let Some(id) = current {
            if id == self.id {
                return nodes.contains_key(&id);
            }
            current = nodes.get(&id).and_then(|node| node.parent);
        }
        false
    }
    pub fn descendants(&self) -> usize {
        let nodes = self.hierarchy.nodes.read();
        let mut pending = alloc::vec![self.id];
        let mut count = 0;
        while let Some(id) = pending.pop() {
            if let Some(node) = nodes.get(&id) {
                count += node.children.len();
                pending.extend(node.children.values().copied());
            }
        }
        count
    }
}
impl Membership {
    fn new(group: Arc<ResourceGroup>, active: bool) -> Result<Self, &'static str> {
        let _transaction = transaction()?;
        let id = IDS.reserve().ok_or("resource IDs exhausted")?.get();
        let member = Arc::new(Member {
            id,
            group: IrqRwSpinLock::new(group.clone()),
            threads: AtomicUsize::new(usize::from(active)),
        });
        let mut nodes = group.hierarchy.nodes.write();
        let node = nodes.get_mut(&group.id).ok_or("resource group removed")?;
        node.members.retain(|_, weak| {
            weak.upgrade()
                .is_some_and(|member| member.threads.load(Ordering::Acquire) != 0)
        });
        if active {
            node.members.insert(id, Arc::downgrade(&member));
        }
        Ok(Self {
            member,
            active: AtomicBool::new(active),
        })
    }
    pub(crate) fn initial(active: bool) -> Self {
        static ROOT: Once<Arc<ResourceGroup>> = Once::new();
        let group = ROOT.call_once(ResourceGroup::new_root).clone();
        // The permanent bootstrap root cannot be removed. Task construction may
        // run with IRQs disabled, so it must not wait on a topology transaction.
        let id = IDS.reserve().expect("resource IDs exhausted").get();
        let member = Arc::new(Member {
            id,
            group: IrqRwSpinLock::new(group.clone()),
            threads: AtomicUsize::new(usize::from(active)),
        });
        if active {
            let mut nodes = group.hierarchy.nodes.write();
            let node = nodes.get_mut(&group.id).unwrap();
            node.members.retain(|_, weak| {
                weak.upgrade()
                    .is_some_and(|member| member.threads.load(Ordering::Acquire) != 0)
            });
            node.members.insert(id, Arc::downgrade(&member));
        }
        Self {
            member,
            active: AtomicBool::new(active),
        }
    }
    pub fn group(&self) -> Arc<ResourceGroup> {
        self.member.group.read().clone()
    }
    pub fn is_live(&self) -> bool {
        self.member.threads.load(Ordering::Acquire) != 0
    }
    pub(crate) fn fork(&self) -> Result<Self, &'static str> {
        Self::new(self.group(), self.active.load(Ordering::Acquire))
    }
    pub(crate) fn thread(&self) -> Result<Self, &'static str> {
        let _transaction = transaction()?;
        if !self.active.load(Ordering::Acquire) {
            return Err("resource membership retired");
        }
        if !self
            .group()
            .hierarchy
            .nodes
            .read()
            .contains_key(&self.group().id)
        {
            return Err("resource group removed");
        }
        self.member
            .threads
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |threads| {
                if threads == 0 {
                    None
                } else {
                    threads.checked_add(1)
                }
            })
            .map_err(|_| "resource membership retired or exhausted")?;
        Ok(Self {
            member: self.member.clone(),
            active: AtomicBool::new(true),
        })
    }
    pub fn migrate(&self, group: Arc<ResourceGroup>) -> Result<(), &'static str> {
        let _transaction = transaction()?;
        if self.member.threads.load(Ordering::Acquire) == 0 {
            return Err("resource membership retired");
        }
        let old = self.group();
        if old.same_group(&group) {
            return Ok(());
        }
        if !old.hierarchy.nodes.read().contains_key(&old.id) {
            return Err("source resource group removed");
        }
        let mut nodes = group.hierarchy.nodes.write();
        let target = nodes.get_mut(&group.id).ok_or("resource group removed")?;
        target.members.retain(|_, weak| {
            weak.upgrade()
                .is_some_and(|member| member.threads.load(Ordering::Acquire) != 0)
        });
        target
            .members
            .insert(self.member.id, Arc::downgrade(&self.member));
        drop(nodes);
        old.hierarchy
            .nodes
            .write()
            .get_mut(&old.id)
            .expect("live resource membership pins node")
            .members
            .remove(&self.member.id);
        *self.member.group.write() = group;
        Ok(())
    }
    pub(crate) fn retire(&self) {
        if self.active.swap(false, Ordering::AcqRel) {
            self.member.threads.fetch_sub(1, Ordering::AcqRel);
        }
    }
}
impl Drop for Membership {
    fn drop(&mut self) {
        self.retire();
    }
}

pub mod syscall {
    use super::*;
    use crate::{
        arch::Trapframe,
        object::{KernelObject, handle::SpecialSemantics},
        task::Task,
    };
    pub fn group(task: &Task, handle: usize) -> Result<Arc<ResourceGroup>, &'static str> {
        match task
            .handle_table
            .get(u32::try_from(handle).map_err(|_| "resource handle")?)
        {
            Some(KernelObject::ResourceGroup(group)) => Ok(group),
            _ => Err("resource capability required"),
        }
    }
    fn insert(task: &Task, group: Arc<ResourceGroup>) -> Result<usize, &'static str> {
        let handle = task
            .handle_table
            .insert(KernelObject::ResourceGroup(group))?;
        let mut metadata = task
            .handle_table
            .get_metadata(handle)
            .ok_or("resource handle metadata")?;
        metadata.special_semantics = Some(SpecialSemantics::CloseOnExec);
        task.handle_table.update_metadata(handle, metadata)?;
        Ok(handle as usize)
    }
    pub fn authorized(task: &Task) -> Result<(), &'static str> {
        if task
            .credentials
            .read()
            .permits(crate::shield::identity::MANAGE_RESOURCES)
        {
            Ok(())
        } else {
            Err("resource management authority required")
        }
    }
    pub fn sys_resource_group_create(tf: &mut Trapframe) -> usize {
        let task = crate::task::mytask().unwrap();
        let parent = tf.get_arg(0);
        let address = tf.get_arg(1);
        tf.increment_pc_next(&task);
        (|| {
            authorized(&task)?;
            let group = if parent == usize::MAX {
                ResourceGroup::new_root()
            } else {
                let name =
                    crate::library::std::string::parse_c_string_from_userspace(&task, address, 256)
                        .map_err(|_| "resource name")?;
                group(&task, parent)?.create(&name)?
            };
            insert(&task, group)
        })()
        .unwrap_or(usize::MAX)
    }
    pub fn sys_resource_group_current(tf: &mut Trapframe) -> usize {
        let task = crate::task::mytask().unwrap();
        tf.increment_pc_next(&task);
        insert(&task, task.resource_membership.group()).unwrap_or(usize::MAX)
    }
    pub fn sys_resource_group_enter(tf: &mut Trapframe) -> usize {
        let task = crate::task::mytask().unwrap();
        let handle = tf.get_arg(0);
        let pid = tf.get_arg(1);
        tf.increment_pc_next(&task);
        (|| {
            authorized(&task)?;
            let id = if pid == 0 {
                task.get_id()
            } else {
                task.get_namespace()
                    .resolve_global_id(pid)
                    .ok_or("resource target visibility")?
            };
            let target =
                crate::sched::scheduler::get_task_by_id(id).ok_or("resource target unavailable")?;
            if target.task_type != crate::task::TaskType::User
                || !target.resource_membership.is_live()
            {
                return Err("invalid resource target");
            }
            target.resource_membership.migrate(group(&task, handle)?)
        })()
        .map_or(usize::MAX, |_| 0)
    }
    pub fn sys_resource_group_status(tf: &mut Trapframe) -> usize {
        let task = crate::task::mytask().unwrap();
        let handle = tf.get_arg(0);
        let address = tf.get_arg(1);
        tf.increment_pc_next(&task);
        (|| {
            let group = group(&task, handle)?;
            let values = [
                group.identity(),
                group.process_count(false) as u64,
                group.process_count(true) as u64,
                group.descendants() as u64,
            ];
            let mut bytes = [0; 32];
            for (i, value) in values.iter().enumerate() {
                bytes[i * 8..i * 8 + 8].copy_from_slice(&value.to_le_bytes());
            }
            crate::library::std::usercopy::copy_to_user(&task, address, &bytes)
                .map_err(|_| "resource status output")
        })()
        .map_or(usize::MAX, |_| 0)
    }
    pub fn sys_resource_group_remove(tf: &mut Trapframe) -> usize {
        let task = crate::task::mytask().unwrap();
        tf.increment_pc_next(&task);
        authorized(&task)
            .and_then(|_| group(&task, tf.get_arg(0))?.remove())
            .map_or(usize::MAX, |_| 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test_case]
    fn process_resource_membership_forks_independently_and_moves_existing_threads_together() {
        let root = ResourceGroup::new_root();
        let a = root.create("a").unwrap();
        let b = root.create("b").unwrap();
        let process = Membership::new(a.clone(), true).unwrap();
        let thread = process.thread().unwrap();
        let child = process.fork().unwrap();
        assert_eq!(a.process_count(false), 2);
        assert_eq!(root.process_count(true), 2);
        process.migrate(b.clone()).unwrap();
        assert!(thread.group().same_group(&b));
        assert!(child.group().same_group(&a));
        assert_eq!(a.process_count(false), 1);
        assert_eq!(b.process_count(false), 1);
        assert!(b.remove().is_err());
        process.retire();
        assert_eq!(b.process_count(false), 1); // Other live threads retain the process membership.
        thread.retire();
        assert_eq!(b.process_count(false), 0);
        b.remove().unwrap();
        assert!(process.migrate(a.clone()).is_err());
        child.retire();
        a.remove().unwrap();
        assert_eq!(root.descendants(), 0);
        assert!(root.remove().is_err());
    }
    #[test_case]
    fn pending_resource_membership_prevents_removal_before_task_publication() {
        let root = ResourceGroup::new_root();
        let group = root.create("pending").unwrap();
        let pending = Membership::new(group.clone(), true).unwrap();
        assert!(group.remove().is_err());
        drop(pending);
        group.remove().unwrap();
        assert!(Membership::new(group.clone(), true).is_err());
        let replacement = root.create("pending").unwrap();
        assert_ne!(group.identity(), replacement.identity());
        assert!(root.contains(&replacement));
        assert!(!root.contains(&group));
    }
}
