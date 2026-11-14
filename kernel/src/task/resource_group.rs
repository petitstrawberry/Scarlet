//! Hierarchical process membership, independent of lifecycle scopes and namespaces.
//! CPU placement constraints are enforced independently of lifecycle scopes.
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
pub(crate) fn transaction() -> Result<MutexGuard<'static, ()>, &'static str> {
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
    cpus: Option<usize>,
    name: String,
    children: BTreeMap<String, u64>,
    members: BTreeMap<u64, Weak<Member>>,
}
struct Hierarchy {
    id: u64,
    nodes: IrqRwSpinLock<BTreeMap<u64, Node>>,
    bootstrap: bool,
}
#[derive(Clone)]
pub struct ResourceGroup {
    hierarchy: Arc<Hierarchy>,
    id: u64,
}
struct Member {
    id: u64,
    groups: IrqRwSpinLock<MembershipGroups>,
    threads: AtomicUsize,
    allowed_cpus: AtomicUsize,
    slots: IrqRwSpinLock<Vec<Weak<CpuSlot>>>,
}
#[derive(Clone)]
struct MembershipGroups {
    primary: u64,
    entries: BTreeMap<u64, Arc<ResourceGroup>>,
}
impl MembershipGroups {
    fn single(group: Arc<ResourceGroup>) -> Self {
        Self {
            primary: group.hierarchy.id,
            entries: BTreeMap::from([(group.hierarchy.id, group)]),
        }
    }
    fn intersection(&self, replacement: Option<(u64, usize)>) -> usize {
        let mut mask = usize::MAX;
        for (&id, group) in &self.entries {
            mask &= if replacement.is_some_and(|(changed, _)| changed == id) {
                replacement.unwrap().1
            } else {
                group.effective_cpus()
            };
        }
        if let Some((_, changed)) = replacement {
            mask &= changed;
        }
        mask
    }
}
/// Non-owning group identity that survives replacement of Arc wrappers.
pub struct WeakResourceGroup {
    hierarchy: Weak<Hierarchy>,
    id: u64,
}
impl WeakResourceGroup {
    pub fn upgrade(&self) -> Option<Arc<ResourceGroup>> {
        let hierarchy = self.hierarchy.upgrade()?;
        if !hierarchy.nodes.read().contains_key(&self.id) {
            return None;
        }
        Some(Arc::new(ResourceGroup {
            hierarchy,
            id: self.id,
        }))
    }
}
/// One lease per task; threads share a process membership. Fork creates a new
/// membership at its parent's current group. Exec retains the original lease.
pub struct Membership {
    member: Arc<Member>,
    slot: Arc<CpuSlot>,
}

struct CpuSlot {
    requested: AtomicUsize,
    effective: AtomicUsize,
    deadline_cpu: AtomicUsize,
    live: AtomicBool,
}
impl CpuSlot {
    fn new(mask: usize, live: bool) -> Arc<Self> {
        Arc::new(Self {
            requested: AtomicUsize::new(mask),
            effective: AtomicUsize::new(mask),
            deadline_cpu: AtomicUsize::new(usize::MAX),
            live: AtomicBool::new(live),
        })
    }
    fn accepts(&self, mask: usize) -> bool {
        if !self.live.load(Ordering::Acquire) {
            return true;
        }
        let cpu = self.deadline_cpu.load(Ordering::Acquire);
        self.requested.load(Ordering::Acquire) & mask != 0
            && (cpu == usize::MAX
                || 1usize
                    .checked_shl(cpu as u32)
                    .is_some_and(|bit| bit & mask != 0))
    }
}
fn effective(
    nodes: &BTreeMap<u64, Node>,
    id: u64,
    replacement: Option<(u64, Option<usize>)>,
) -> Result<usize, &'static str> {
    let mut mask = usize::MAX;
    let mut current = Some(id);
    while let Some(id) = current {
        let node = nodes.get(&id).ok_or("resource group removed")?;
        mask &= if replacement.is_some_and(|(changed, _)| changed == id) {
            replacement.unwrap().1
        } else {
            node.cpus
        }
        .unwrap_or(usize::MAX);
        current = node.parent;
    }
    Ok(mask)
}
fn online() -> usize {
    crate::sched::scheduler::online_cpu_mask() as usize
}
impl Member {
    fn publish_cpus(&self, mask: usize) {
        self.allowed_cpus.store(mask, Ordering::Release);
        for slot in self.slots.read().iter().filter_map(Weak::upgrade) {
            slot.effective.store(
                slot.requested.load(Ordering::Acquire) & mask,
                Ordering::Release,
            );
        }
    }
    fn accepts(&self, mask: usize) -> bool {
        self.slots
            .read()
            .iter()
            .filter_map(Weak::upgrade)
            .all(|slot| slot.accepts(mask))
    }
}
fn reconcile_members(ids: &[u64]) {
    for id in crate::sched::scheduler::get_task_pool().task_ids_snapshot() {
        if let Some(task) = crate::sched::scheduler::get_task_by_id(id) {
            if ids.contains(&task.resource_membership.member.id) {
                crate::sched::scheduler::reconcile_task_affinity(&task);
            }
        }
    }
}

impl ResourceGroup {
    pub fn new_root() -> Arc<Self> {
        Self::root(false)
    }
    fn root(bootstrap: bool) -> Arc<Self> {
        let id = IDS.reserve().expect("resource IDs exhausted").get();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            id,
            Node {
                parent: None,
                cpus: None,
                name: String::new(),
                children: BTreeMap::new(),
                members: BTreeMap::new(),
            },
        );
        Arc::new(Self {
            hierarchy: Arc::new(Hierarchy {
                id,
                nodes: IrqRwSpinLock::new(nodes),
                bootstrap,
            }),
            id,
        })
    }
    pub fn configured_cpus(&self) -> Option<usize> {
        self.hierarchy
            .nodes
            .read()
            .get(&self.id)
            .and_then(|node| node.cpus)
    }
    pub fn effective_cpus(&self) -> usize {
        effective(&self.hierarchy.nodes.read(), self.id, None).unwrap_or(0)
    }
    pub fn set_cpus(&self, cpus: Option<usize>) -> Result<(), &'static str> {
        let _transaction = transaction()?;
        let online = online();
        if cpus.is_some_and(|mask| mask == 0 || mask & online == 0 || mask & !online != 0) {
            return Err("CPU set must use online CPUs");
        }
        let mut nodes = self.hierarchy.nodes.write();
        let node = nodes.get(&self.id).ok_or("resource group removed")?;
        if self.hierarchy.bootstrap && node.parent.is_none() {
            return Err("bootstrap CPU placement is unmanaged");
        }
        let replacement = Some((self.id, cpus));
        let mut updates = Vec::new();
        let mut ids = Vec::new();
        for (&id, node) in nodes.iter() {
            let mask = effective(&nodes, id, replacement)?;
            let configured = if id == self.id { cpus } else { node.cpus };
            if let Some(parent) = node.parent {
                if configured.is_some_and(|requested| {
                    requested & !effective(&nodes, parent, replacement).unwrap_or(0) != 0
                }) {
                    return Err("CPU set exceeds parent");
                }
            }
            for member in node.members.values().filter_map(Weak::upgrade) {
                if member.threads.load(Ordering::Acquire) != 0 {
                    let mask = member
                        .groups
                        .read()
                        .intersection(Some((self.hierarchy.id, mask)));
                    if !member.accepts(mask & online) {
                        return Err("CPU set strands a task or deadline reservation");
                    }
                    ids.push(member.id);
                    updates.push((member, mask));
                }
            }
        }
        nodes.get_mut(&self.id).unwrap().cpus = cpus;
        for (member, mask) in updates {
            member.publish_cpus(mask);
        }
        drop(nodes);
        reconcile_members(&ids);
        Ok(())
    }
    pub fn downgrade(&self) -> WeakResourceGroup {
        WeakResourceGroup {
            hierarchy: Arc::downgrade(&self.hierarchy),
            id: self.id,
        }
    }
    pub fn hierarchy_identity(&self) -> u64 {
        self.hierarchy.id
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
                cpus: None,
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
                        member.threads.load(Ordering::Acquire) != 0
                            && member
                                .groups
                                .read()
                                .entries
                                .get(&self.hierarchy.id)
                                .is_some_and(|location| location.id == id)
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
    #[cfg(test)]
    fn new_with_mask(
        group: Arc<ResourceGroup>,
        active: bool,
        mask: usize,
    ) -> Result<Self, &'static str> {
        let _transaction = transaction()?;
        Self::new_with_groups(MembershipGroups::single(group), active, mask)
    }
    // Caller holds the admission transaction, including across the parent's snapshot.
    fn new_with_groups(
        groups: MembershipGroups,
        active: bool,
        mask: usize,
    ) -> Result<Self, &'static str> {
        let allowed = groups.intersection(None);
        if allowed == 0 || active && mask & allowed & online() == 0 && online() != 0 {
            return Err("resource group excludes task affinity");
        }
        let id = IDS.reserve().ok_or("resource IDs exhausted")?.get();
        let slot = CpuSlot::new(mask, active);
        slot.effective.store(mask & allowed, Ordering::Release);
        let locations = groups.entries.values().cloned().collect::<Vec<_>>();
        let member = Arc::new(Member {
            id,
            groups: IrqRwSpinLock::new(groups),
            threads: AtomicUsize::new(usize::from(active)),
            allowed_cpus: AtomicUsize::new(allowed),
            slots: IrqRwSpinLock::new(alloc::vec![Arc::downgrade(&slot)]),
        });
        for group in locations {
            let mut nodes = group.hierarchy.nodes.write();
            let node = nodes
                .get_mut(&group.id)
                .expect("admission transaction pins live group");
            node.members.retain(|_, weak| {
                weak.upgrade()
                    .is_some_and(|member| member.threads.load(Ordering::Acquire) != 0)
            });
            if active {
                node.members.insert(id, Arc::downgrade(&member));
            }
        }
        Ok(Self { member, slot })
    }
    #[cfg(test)]
    fn new(group: Arc<ResourceGroup>, active: bool) -> Result<Self, &'static str> {
        Self::new_with_mask(group, active, usize::MAX)
    }
    pub(crate) fn initial(active: bool) -> Self {
        static ROOT: Once<Arc<ResourceGroup>> = Once::new();
        let group = ROOT.call_once(|| ResourceGroup::root(true)).clone();
        let id = IDS.reserve().expect("resource IDs exhausted").get();
        let slot = CpuSlot::new(usize::MAX, active);
        let member = Arc::new(Member {
            id,
            groups: IrqRwSpinLock::new(MembershipGroups::single(group.clone())),
            threads: AtomicUsize::new(usize::from(active)),
            allowed_cpus: AtomicUsize::new(usize::MAX),
            slots: IrqRwSpinLock::new(alloc::vec![Arc::downgrade(&slot)]),
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
        Self { member, slot }
    }
    pub fn group(&self) -> Arc<ResourceGroup> {
        let groups = self.member.groups.read();
        groups.entries.get(&groups.primary).unwrap().clone()
    }
    pub fn group_in(&self, hierarchy: &ResourceGroup) -> Option<Arc<ResourceGroup>> {
        self.member
            .groups
            .read()
            .entries
            .get(&hierarchy.hierarchy.id)
            .cloned()
    }
    pub fn thread_count(&self) -> usize {
        self.member.threads.load(Ordering::Acquire)
    }
    pub fn is_live(&self) -> bool {
        self.member.threads.load(Ordering::Acquire) != 0
    }
    pub fn allowed_cpus(&self) -> usize {
        if self.slot.live.load(Ordering::Acquire) {
            self.member.allowed_cpus.load(Ordering::Acquire)
        } else {
            usize::MAX
        }
    }
    pub(crate) fn record_affinity(&self, mask: usize) {
        self.slot.requested.store(mask, Ordering::Release);
        self.slot
            .effective
            .store(mask & self.allowed_cpus(), Ordering::Release);
    }
    pub(crate) fn effective_affinity(&self) -> usize {
        self.slot.effective.load(Ordering::Acquire)
    }
    pub(crate) fn record_deadline_cpu(&self, cpu: Option<usize>) {
        self.slot
            .deadline_cpu
            .store(cpu.unwrap_or(usize::MAX), Ordering::Release);
    }
    #[cfg(test)]
    pub(crate) fn fork(&self) -> Result<Self, &'static str> {
        self.fork_with_affinity(self.slot.requested.load(Ordering::Acquire))
    }
    pub(crate) fn fork_with_affinity(&self, mask: usize) -> Result<Self, &'static str> {
        let _transaction = transaction()?;
        let groups = self.member.groups.read().clone();
        Self::new_with_groups(groups, self.slot.live.load(Ordering::Acquire), mask)
    }
    #[cfg(test)]
    pub(crate) fn thread(&self) -> Result<Self, &'static str> {
        self.thread_with_affinity(self.slot.requested.load(Ordering::Acquire))
    }
    pub(crate) fn thread_with_affinity(&self, mask: usize) -> Result<Self, &'static str> {
        let _transaction = transaction()?;
        if !self.slot.live.load(Ordering::Acquire)
            || mask & self.allowed_cpus() & online() == 0 && online() != 0
        {
            return Err("resource membership retired or excludes thread affinity");
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
        let slot = CpuSlot::new(mask, true);
        slot.effective
            .store(mask & self.allowed_cpus(), Ordering::Release);
        let mut slots = self.member.slots.write();
        slots.retain(|weak| {
            weak.upgrade()
                .is_some_and(|slot| slot.live.load(Ordering::Acquire))
        });
        slots.push(Arc::downgrade(&slot));
        Ok(Self {
            member: self.member.clone(),
            slot,
        })
    }
    /// Select a native primary group, retaining all independent hierarchies.
    pub fn migrate(&self, group: Arc<ResourceGroup>) -> Result<(), &'static str> {
        self.migrate_checked(group, false, true)
    }
    /// Join a hierarchy without changing the native primary group.
    pub fn join(&self, group: Arc<ResourceGroup>) -> Result<(), &'static str> {
        self.migrate_checked(group, false, false)
    }
    pub fn migrate_single_thread(&self, group: Arc<ResourceGroup>) -> Result<(), &'static str> {
        self.migrate_checked(group, true, false)
    }
    fn migrate_checked(
        &self,
        group: Arc<ResourceGroup>,
        single_thread: bool,
        primary: bool,
    ) -> Result<(), &'static str> {
        let _transaction = transaction()?;
        if single_thread && self.thread_count() != 1 {
            return Err("thread migration requires per-thread group support");
        }
        if !self.is_live() {
            return Err("resource membership retired");
        }
        let domain = group.hierarchy.id;
        let (old, allowed) = {
            let groups = self.member.groups.read();
            let old = groups.entries.get(&domain).cloned();
            if old.is_none() && groups.entries.len() >= 16 {
                return Err("resource hierarchy bound");
            }
            (
                old,
                groups.intersection(Some((domain, group.effective_cpus()))),
            )
        };
        if allowed == 0 || online() != 0 && !self.member.accepts(allowed & online()) {
            return Err("resource groups exclude process affinity or deadline reservation");
        }
        if !old.as_ref().is_some_and(|old| old.same_group(&group)) {
            let mut nodes = group.hierarchy.nodes.write();
            let target = nodes.get_mut(&group.id).ok_or("resource group removed")?;
            target.members.retain(|_, weak| {
                weak.upgrade()
                    .is_some_and(|member| member.threads.load(Ordering::Acquire) != 0)
            });
            target
                .members
                .insert(self.member.id, Arc::downgrade(&self.member));
            if let Some(old) = &old {
                nodes
                    .get_mut(&old.id)
                    .expect("live membership pins node")
                    .members
                    .remove(&self.member.id);
            }
        }
        {
            let mut groups = self.member.groups.write();
            groups.entries.insert(domain, group);
            if primary {
                groups.primary = domain;
            }
        }
        self.member.publish_cpus(allowed);
        reconcile_members(&[self.member.id]);
        Ok(())
    }
    pub fn leave(&self, hierarchy: &ResourceGroup) -> Result<(), &'static str> {
        let _transaction = transaction()?;
        if !self.is_live() {
            return Err("resource membership retired");
        }
        let old = {
            let mut groups = self.member.groups.write();
            if groups.entries.len() <= 1 {
                return Err("cannot leave final resource hierarchy");
            }
            let old = groups
                .entries
                .remove(&hierarchy.hierarchy.id)
                .ok_or("not a member of hierarchy")?;
            if groups.primary == hierarchy.hierarchy.id {
                groups.primary = *groups.entries.keys().next().unwrap();
            }
            old
        };
        old.hierarchy
            .nodes
            .write()
            .get_mut(&old.id)
            .expect("live membership pins node")
            .members
            .remove(&self.member.id);
        let allowed = self.member.groups.read().intersection(None);
        self.member.publish_cpus(allowed);
        reconcile_members(&[self.member.id]);
        Ok(())
    }
    pub(crate) fn retire(&self) {
        if self.slot.live.swap(false, Ordering::AcqRel) {
            self.member.threads.fetch_sub(1, Ordering::AcqRel);
        }
    }
}
/// Affinity changes and group constraints share admission serialization.
pub fn set_task_affinity(task: &crate::task::Task, mask: usize) -> Result<(), &'static str> {
    let _transaction = transaction()?;
    if mask & task.resource_membership.allowed_cpus() & online() == 0 {
        return Err("empty effective CPU affinity");
    }
    if task.deadline_enabled() {
        return Err("deadline affinity is reserved");
    }
    task.set_cpu_affinity_mask(mask);
    crate::sched::scheduler::reconcile_task_affinity(task);
    Ok(())
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
    /// arg1: 0=query, 1=set mask, 2=inherit; arg2: mask, arg3: 16-byte query output.
    pub fn sys_resource_group_cpus(tf: &mut Trapframe) -> usize {
        let task = crate::task::mytask().unwrap();
        let (handle, op, mask, address) =
            (tf.get_arg(0), tf.get_arg(1), tf.get_arg(2), tf.get_arg(3));
        tf.increment_pc_next(&task);
        (|| {
            let group = group(&task, handle)?;
            match op {
                0 => {
                    let mut bytes = [0; 16];
                    bytes[..8].copy_from_slice(
                        &(group.configured_cpus().unwrap_or(0) as u64).to_le_bytes(),
                    );
                    bytes[8..].copy_from_slice(
                        &((group.effective_cpus() & online()) as u64).to_le_bytes(),
                    );
                    crate::library::std::usercopy::copy_to_user(&task, address, &bytes)
                        .map_err(|_| "CPU set output")
                }
                1 | 2 => {
                    authorized(&task)?;
                    group.set_cpus(if op == 2 { None } else { Some(mask) })
                }
                _ => Err("CPU set operation"),
            }
        })()
        .map_or(usize::MAX, |_| 0)
    }
    pub fn sys_resource_group_leave(tf: &mut Trapframe) -> usize {
        let task = crate::task::mytask().unwrap();
        let handle = tf.get_arg(0);
        tf.increment_pc_next(&task);
        authorized(&task)
            .and_then(|_| {
                task.resource_membership
                    .leave(group(&task, handle)?.as_ref())
            })
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
    struct TestCpus;
    impl TestCpus {
        fn new() -> Self {
            crate::sched::scheduler::reset();
            crate::sched::scheduler::register_online_cpu(0);
            crate::sched::scheduler::register_online_cpu(1);
            Self
        }
    }
    impl Drop for TestCpus {
        fn drop(&mut self) {
            crate::sched::scheduler::reset();
        }
    }

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
    #[test_case]
    fn cpu_constraints_validate_unpublished_fork_and_thread_leases() {
        let _cpus = TestCpus::new();
        let mask = online();
        let first = mask & mask.wrapping_neg();
        let root = ResourceGroup::new_root();
        root.set_cpus(Some(mask)).unwrap();
        let group = root.create("cpu").unwrap();
        group.set_cpus(Some(first)).unwrap();
        let process = Membership::new_with_mask(group.clone(), true, first).unwrap();
        let thread = process.thread_with_affinity(first).unwrap();
        let child = process.fork_with_affinity(first).unwrap();
        assert_eq!(child.allowed_cpus() & mask, first);
        assert!(group.set_cpus(Some(0)).is_err());
        assert!(root.set_cpus(Some(0)).is_err());
        assert_eq!(group.effective_cpus(), first);
        let other = mask & !first;
        if other != 0 {
            let second = other & other.wrapping_neg();
            assert!(group.set_cpus(Some(second)).is_err());
            assert_eq!(group.configured_cpus(), Some(first));
            let elsewhere = root.create("other").unwrap();
            elsewhere.set_cpus(Some(second)).unwrap();
            assert!(thread.migrate(elsewhere).is_err());
        }
        assert!(process.migrate_single_thread(root.clone()).is_err());
        thread.retire();
        process.migrate_single_thread(root.clone()).unwrap();
        assert_eq!(process.allowed_cpus() & mask, mask);
        assert_eq!(child.allowed_cpus() & mask, first);
    }
    #[test_case]
    fn cpu_constraints_preserve_deadline_pins_and_parent_subset() {
        let _cpus = TestCpus::new();
        let mask = online();
        let first = mask & mask.wrapping_neg();
        let root = ResourceGroup::new_root();
        let child = root.create("reserved").unwrap();
        child.set_cpus(Some(first)).unwrap();
        let member = Membership::new(child.clone(), true).unwrap();
        member.record_deadline_cpu(Some(first.trailing_zeros() as usize));
        assert!(root.set_cpus(Some(mask & !first)).is_err());
        assert_eq!(root.configured_cpus(), None);
        assert_eq!(child.configured_cpus(), Some(first));
        if mask.count_ones() > 1 {
            child.set_cpus(None).unwrap();
            assert!(root.set_cpus(Some(mask & !first)).is_err());
            member.record_deadline_cpu(None);
            root.set_cpus(Some(mask & !first)).unwrap();
            assert_eq!(member.allowed_cpus() & mask, mask & !first);
        }
    }
    #[test_case]
    fn independent_resource_hierarchies_retain_membership_and_intersect_constraints() {
        let _cpus = TestCpus::new();
        let mask = online();
        let first = mask & mask.wrapping_neg();
        let lifetime = ResourceGroup::new_root();
        let process_group = lifetime.create("process").unwrap();
        let placement = ResourceGroup::new_root();
        let profile = placement.create("profile").unwrap();
        profile.set_cpus(Some(first)).unwrap();
        let member = Membership::new(process_group.clone(), true).unwrap();
        member.join(profile.clone()).unwrap();
        assert!(member.group().same_group(&process_group));
        assert_eq!(lifetime.process_count(true), 1);
        assert_eq!(placement.process_count(true), 1);
        let fork = member.fork().unwrap();
        let thread = member.thread().unwrap();
        assert!(fork.group_in(&placement).unwrap().same_group(&profile));
        assert!(
            thread
                .group_in(&lifetime)
                .unwrap()
                .same_group(&process_group)
        );
        assert_eq!(member.allowed_cpus() & mask, first);
        assert_eq!(placement.process_count(true), 2);
        assert!(profile.remove().is_err());
        if mask.count_ones() > 1 {
            assert!(lifetime.set_cpus(Some(mask & !first)).is_err());
            assert_eq!(lifetime.configured_cpus(), None);
        }
        member.leave(&placement).unwrap();
        assert!(member.group_in(&placement).is_none());
        assert!(thread.group_in(&placement).is_none());
        assert!(fork.group_in(&placement).is_some());
        assert_eq!(lifetime.process_count(true), 2);
        assert_eq!(placement.process_count(true), 1);
        assert_eq!(member.allowed_cpus() & mask, mask);
        assert!(member.leave(&lifetime).is_err());
    }
}
