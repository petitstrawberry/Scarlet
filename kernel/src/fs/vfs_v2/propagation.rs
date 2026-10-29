//! Mount-event topology, independent of Environment and process identity.
//!
//! One sleepable transaction serializes topology changes, namespace copies and
//! event publication. Registry references are weak: neither peer relationships
//! nor upstream channels keep a discarded View/mount alive. Group channels can
//! outlive their last peer while downstream mounts still refer to them.
use alloc::{collections::{BTreeMap, BTreeSet}, sync::{Arc, Weak}, vec::Vec};
use super::{manager::VfsView, mount_tree::{MountId, MountPoint, VfsEntryRef, VfsResult}};
use crate::{fs::{FileSystemError, FileSystemErrorKind}, sync::{Mutex, MutexGuard, sequence::IdSequence}};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Propagation { Private, Shared, Downstream }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PropagationInfo {
    pub peer_group: Option<u64>,
    pub upstream_group: Option<u64>,
}
struct Record {
    mount: Weak<MountPoint>,
    view: Weak<VfsView>,
    info: PropagationInfo,
    event: MountId,
}
pub(super) struct Topology {
    records: BTreeMap<MountId, Record>,
    /// Parent channels preserve downstream flow when an intermediate View dies.
    channels: BTreeMap<u64, Option<u64>>,
}
static TOPOLOGY: Mutex<Topology> = Mutex::new(Topology {
    records: BTreeMap::new(), channels: BTreeMap::new(),
});
static GROUP_IDS: IdSequence = IdSequence::new();
fn error(kind: FileSystemErrorKind, message: &str) -> FileSystemError {
    FileSystemError::new(kind, message)
}
pub(super) fn transaction() -> VfsResult<MutexGuard<'static, Topology>> {
    let mut guard = if let Some(guard) = TOPOLOGY.try_lock() { guard }
        else if crate::sync::preemptible() { TOPOLOGY.lock() }
        else { return Err(error(FileSystemErrorKind::Busy, "Mount topology transaction busy")); };
    guard.prune();
    Ok(guard)
}
pub fn information(mount: &MountPoint) -> VfsResult<PropagationInfo> {
    Ok(transaction()?.info(mount))
}
impl Topology {
    fn attached(mount: &Arc<MountPoint>, view: &Arc<VfsView>) -> bool {
        let root = view.mount_tree.root_mount.read().clone();
        let mut current = mount.clone();
        loop {
            if Arc::ptr_eq(&current, &root) { return true; }
            let Some(parent) = current.get_parent() else { return false };
            let Some(entry) = current.parent_entry.read().clone() else { return false };
            if !parent.get_child(&entry).is_some_and(|child| Arc::ptr_eq(&child, &current)) { return false; }
            current = parent;
        }
    }
    fn prune(&mut self) {
        self.records.retain(|_, record| match (record.mount.upgrade(), record.view.upgrade()) {
            (Some(mount), Some(view)) => Self::attached(&mount, &view),
            _ => false,
        });
        let peers: BTreeSet<_> = self.records.values().filter_map(|r| r.info.peer_group).collect();
        let nearest = |mut group: Option<u64>| {
            while let Some(id) = group {
                if peers.contains(&id) { return Some(id); }
                group = self.channels.get(&id).copied().flatten();
            }
            None
        };
        let updates: Vec<_> = self.records.iter().map(|(id, r)| (*id, nearest(r.info.upstream_group))).collect();
        for (id, upstream) in updates { self.records.get_mut(&id).unwrap().info.upstream_group = upstream; }
        let updates: Vec<_> = peers.iter().map(|id| (*id, nearest(self.channels.get(id).copied().flatten()))).collect();
        for (id, upstream) in updates { self.channels.insert(id, upstream); }
        self.channels.retain(|id, _| peers.contains(id));
    }
    fn group(&mut self, upstream: Option<u64>) -> u64 {
        let id = GROUP_IDS.reserve().expect("mount group IDs exhausted").get();
        self.channels.insert(id, upstream);
        id
    }
    pub(super) fn info(&self, mount: &MountPoint) -> PropagationInfo {
        self.records.get(&mount.id).map_or(PropagationInfo::default(), |r| r.info)
    }
    fn event(&self, mount: &MountPoint) -> MountId {
        self.records.get(&mount.id).map_or(mount.id, |r| r.event)
    }
    fn remember(&mut self, mount: &Arc<MountPoint>, view: &Arc<VfsView>, info: PropagationInfo, event: MountId) {
        self.records.insert(mount.id, Record {
            mount: Arc::downgrade(mount), view: Arc::downgrade(view), info, event,
        });
    }
    pub(super) fn inherit(&mut self, source: &MountPoint, copy: &Arc<MountPoint>, view: &Arc<VfsView>) {
        self.remember(copy, view, self.info(source), self.event(source));
    }
    pub(super) fn change(&mut self, mount: &Arc<MountPoint>, view: &Arc<VfsView>, mode: Propagation, recursive: bool) {
        let mut mounts = alloc::vec![mount.clone()];
        while let Some(current) = mounts.pop() {
            if recursive { mounts.extend(current.children.read().values().cloned()); }
            let mut info = self.info(&current);
            match mode {
                Propagation::Private => info = PropagationInfo::default(),
                Propagation::Shared => {
                    if info.peer_group.is_none() { info.peer_group = Some(self.group(info.upstream_group)); }
                }
                Propagation::Downstream => {
                    if let Some(group) = info.peer_group {
                        let has_peer = self.records.iter().any(|(id, r)| *id != current.id && r.info.peer_group == Some(group));
                        info.upstream_group = if has_peer { Some(group) } else { info.upstream_group };
                        info.peer_group = None;
                    }
                }
            }
            self.remember(&current, view, info, self.event(&current));
        }
    }
    /// Copy mount nodes only; dentry/inode and backing filesystem identities stay
    /// shared. The new parent links always belong to the destination tree.
    pub(super) fn copy_tree(&mut self, old: &Arc<MountPoint>, view: &Arc<VfsView>, recursive: bool, depth: usize) -> VfsResult<Arc<MountPoint>> {
        if depth > 256 { return Err(error(FileSystemErrorKind::InvalidPath, "Mount tree too deep")); }
        let copy = Arc::new(MountPoint {
            id: MountId::new(), restrictions: old.restrictions, mount_type: old.mount_type.clone(),
            path: crate::sync::IrqRwSpinLock::new(old.path.read().clone()), root: old.root.clone(),
            filesystem: old.filesystem.clone().or_else(|| old.root.node().filesystem().and_then(|f| f.upgrade())),
            parent: crate::sync::IrqRwSpinLock::new(None), parent_entry: crate::sync::IrqRwSpinLock::new(None),
            children: Arc::new(crate::sync::IrqRwSpinLock::new(BTreeMap::new())),
        });
        self.inherit(old, &copy, view);
        if recursive {
            let children: Vec<_> = old.children.read().values().cloned().collect();
            for child in children {
                let entry = child.parent_entry.read().clone().ok_or_else(|| error(FileSystemErrorKind::InvalidPath, "Orphan mount"))?;
                let new = self.copy_tree(&child, view, true, depth + 1)?;
                copy.add_child(&entry, new)?;
            }
        }
        Ok(copy)
    }
    /// A peer bound to a subtree only receives events inside that subtree.
    fn contains(mount: &MountPoint, target: &VfsEntryRef) -> bool {
        Self::contains_entry(&mount.root, target)
    }
    pub(super) fn contains_entry(root: &VfsEntryRef, target: &VfsEntryRef) -> bool {
        let mut entry = Some(target.clone());
        while let Some(current) = entry {
            if Arc::ptr_eq(&current, root) { return true; }
            entry = current.parent();
        }
        false
    }
    fn receivers(&self, origin: &Arc<MountPoint>, view: &Arc<VfsView>, target: &VfsEntryRef)
        -> Vec<(Arc<MountPoint>, Arc<VfsView>, PropagationInfo)> {
        let mut result = alloc::vec![(origin.clone(), view.clone(), self.info(origin))];
        let Some(group) = self.info(origin).peer_group else { return result };
        let mut groups = BTreeSet::new();
        let mut pending = alloc::vec![group];
        while let Some(id) = pending.pop() {
            if !groups.insert(id) { continue; }
            for (&child, &upstream) in &self.channels { if upstream == Some(id) { pending.push(child); } }
        }
        for record in self.records.values() {
            if !record.info.peer_group.is_some_and(|id| groups.contains(&id))
                && !record.info.upstream_group.is_some_and(|id| groups.contains(&id)) { continue; }
            let (Some(mount), Some(owner)) = (record.mount.upgrade(), record.view.upgrade()) else { continue };
            if mount.id != origin.id && Self::attached(&mount, &owner) && Self::contains(&mount, target) { result.push((mount, owner, record.info)); }
        }
        result
    }
    fn reaches(source: &Arc<VfsView>, target: &Arc<VfsView>) -> bool {
        let mut visited = BTreeSet::new();
        let mut pending = alloc::vec![source.clone()];
        while let Some(view) = pending.pop() {
            if Arc::ptr_eq(&view, target) { return true; }
            if visited.insert(Arc::as_ptr(&view) as usize) {
                pending.extend(view.dependencies.read().iter().filter_map(Weak::upgrade));
            }
        }
        false
    }
    pub(super) fn publish(&mut self, parent: &Arc<MountPoint>, target: &VfsEntryRef,
        new: &Arc<MountPoint>, view: &Arc<VfsView>) -> VfsResult<()> {
        let receivers = self.receivers(parent, view, target);
        for (receiver, _, _) in &receivers {
            if receiver.children.read().contains_key(&target.node().id()) {
                return Err(error(FileSystemErrorKind::Busy, "Propagation target already mounted"));
            }
        }
        let dependencies: Vec<_> = view.dependencies.read().iter().filter_map(Weak::upgrade).collect();
        for (_, owner, _) in &receivers {
            if !Arc::ptr_eq(owner, view) && dependencies.iter().any(|source| Self::reaches(source, owner)) {
                return Err(error(FileSystemErrorKind::InvalidOperation, "Cyclic propagated backing views"));
            }
        }
        let mut source_info = self.info(new);
        let mut branches = BTreeMap::new();
        if let Some(group) = self.info(parent).peer_group {
            let child_group = source_info.peer_group.unwrap_or_else(|| self.group(source_info.upstream_group));
            source_info.peer_group = Some(child_group);
            branches.insert(group, child_group);
        }
        let mut projected = BTreeMap::new();
        let mut planned = Vec::new();
        for (receiver, owner, parent_info) in receivers {
            let child = if receiver.id == parent.id { new.clone() } else { self.copy_tree(new, &owner, true, 0)? };
            let info = if let Some(origin_group) = self.info(parent).peer_group {
                self.child_info(parent_info, origin_group, source_info, &mut branches)
            } else { source_info };
            if self.info(parent).peer_group.is_some_and(|group| parent_info.peer_group != Some(group)) {
                let origin = self.info(parent).peer_group.unwrap();
                self.project_descendants(new, &child, &owner, parent_info, origin, &mut projected);
            }
            planned.push((receiver, owner, child, info));
        }
        // No path/driver callbacks or fallible validation remain during publication.
        for (receiver, owner, child, info) in planned {
            if !Arc::ptr_eq(&owner, view) {
                let mut edges = owner.dependencies.write();
                for dependency in &dependencies {
                    if !edges.iter().any(|edge| edge.ptr_eq(&Arc::downgrade(dependency))) {
                        edges.push(Arc::downgrade(dependency));
                    }
                }
            }
            self.remember(&child, &owner, info, new.id);
            receiver.add_child(target, child)?;
        }
        Ok(())
    }
    fn projected_group(&mut self, source: u64, destination: u64, origin: u64,
        projected: &mut BTreeMap<(u64,u64),u64>) -> u64 {
        if destination == origin { return source; }
        if let Some(id) = projected.get(&(source, destination)) { return *id; }
        let upstream = self.channels[&destination].expect("downstream channel has upstream");
        let parent = self.projected_group(source, upstream, origin, projected);
        let group = self.group(Some(parent));
        projected.insert((source, destination), group);
        group
    }
    fn project_descendants(&mut self, source: &Arc<MountPoint>, copy: &Arc<MountPoint>, owner: &Arc<VfsView>, destination: PropagationInfo,
        origin: u64, projected: &mut BTreeMap<(u64,u64),u64>) {
        let children: Vec<_> = source.children.read().iter().map(|(id, child)| (*id, child.clone())).collect();
        for (id, child) in children {
            let new = copy.children.read()[&id].clone();
            let old_info = self.info(&child);
            let info = if let Some(peer) = old_info.peer_group {
                if let Some(channel) = destination.peer_group {
                    let group = self.projected_group(peer, channel, origin, projected);
                    PropagationInfo { peer_group: Some(group), upstream_group: self.channels[&group] }
                } else {
                    let upstream = self.projected_group(peer, destination.upstream_group.unwrap(), origin, projected);
                    PropagationInfo { peer_group: None, upstream_group: Some(upstream) }
                }
            } else { old_info };
            self.remember(&new, owner, info, self.event(&child));
            self.project_descendants(&child, &new, owner, destination, origin, projected);
        }
    }
    fn child_group(&mut self, parent: u64, branches: &mut BTreeMap<u64, u64>) -> u64 {
        if let Some(id) = branches.get(&parent) { return *id; }
        let upstream_parent = self.channels.get(&parent).copied().flatten().expect("downstream channel has upstream");
        let upstream = self.child_group(upstream_parent, branches);
        let id = self.group(Some(upstream));
        branches.insert(parent, id);
        id
    }
    fn child_info(&mut self, parent: PropagationInfo, origin: u64, source: PropagationInfo, branches: &mut BTreeMap<u64,u64>) -> PropagationInfo {
        if parent.peer_group == Some(origin) { return source; }
        if let Some(peer) = parent.peer_group {
            let group = self.child_group(peer, branches);
            return PropagationInfo { peer_group: Some(group), upstream_group: self.channels[&group] };
        }
        let upstream = self.child_group(parent.upstream_group.unwrap(), branches);
        PropagationInfo { peer_group: None, upstream_group: Some(upstream) }
    }
    pub(super) fn forget_tree(&mut self, root: &Arc<MountPoint>) {
        let mut pending = alloc::vec![root.clone()];
        while let Some(mount) = pending.pop() {
            pending.extend(mount.children.read().values().cloned());
            self.records.remove(&mount.id);
        }
    }
    pub(super) fn detach(&mut self, parent: &Arc<MountPoint>, target: &VfsEntryRef, view: &Arc<VfsView>) -> VfsResult<Arc<MountPoint>> {
        let origin = parent.get_child(target).ok_or_else(|| error(FileSystemErrorKind::NotFound, "Mount not found"))?;
        let event = self.event(&origin);
        let receivers = self.receivers(parent, view, target);
        for (receiver, _, _) in receivers {
            if let Some(child) = receiver.get_child(target) {
                if self.event(&child) == event {
                    receiver.remove_child(target);
                    self.forget_tree(&child);
                }
            }
        }
        Ok(origin)
    }
}
