//! Per-policy resource labels, registered through retained native file handles.
//! A retained inode and filesystem prevent identity reuse after unlink. Labels
//! are independent of pathnames, ABI xattrs, Environment and mount namespace.
use super::{Policy, Subject, identity::{Credentials, Identities}};
use crate::fs::vfs_v2::core::{FileSystemOperations, VfsNode};
use alloc::{collections::BTreeMap, sync::Arc};

struct Resource {
    label: u32,
    _node: Arc<dyn VfsNode>,
    _filesystem: Arc<dyn FileSystemOperations>,
}
pub(super) struct Resources(BTreeMap<(u64, u64), Resource>);
impl Resources { pub(super) const fn new() -> Self { Self(BTreeMap::new()) } }

impl Policy {
    pub fn bind_resource(&self, node: Arc<dyn VfsNode>, label: u32) -> Result<(), &'static str> {
        if self.label(label).is_none() { return Err("unknown resource label"); }
        let filesystem = node.filesystem().and_then(|fs| fs.upgrade()).ok_or("resource filesystem lost")?;
        if !filesystem.supports_retained_node_identity() { return Err("resource identity cannot be retained"); }
        let key = (filesystem.fs_id().get(), node.id());
        let mut resources = self.resources.write();
        if let Some(previous) = resources.0.get(&key) {
            return if previous.label == label { Ok(()) } else { Err("resource label already bound") };
        }
        if resources.0.len() >= 4096 { return Err("resource label bound"); }
        resources.0.insert(key, Resource {label, _node: node, _filesystem: filesystem});
        Ok(())
    }
    pub fn resource_label(&self, node: &dyn VfsNode) -> Option<u32> {
        self.bound_resource_label(node).or_else(||node.shield_labels().and_then(|labels|labels.get(self))).or(self.default_file_label())
    }
    pub(crate) fn bound_resource_label(&self, node: &dyn VfsNode) -> Option<u32> {
        let filesystem = node.filesystem()?.upgrade()?;
        self.resources.read().0.get(&(filesystem.fs_id().get(), node.id())).map(|resource| resource.label)
    }
    /// A query can select a label only if every registered principal condition
    /// agrees. Actual execution must additionally match the native credentials.
    pub fn has_exec_resource(&self, source: u32, resource: u32) -> bool {
        self.snapshot().exec_transitions.keys().any(|&(origin, file, _)| origin == source && file == resource)
    }
    pub fn preview_exec(&self, source: u32, resource: u32) -> Result<u32, &'static str> {
        let mut target = None;
        for (&(origin, file, _), &next) in &self.snapshot().exec_transitions {
            if origin == source && file == resource {
                if target.is_some_and(|value| value != next) { return Err("ambiguous exec preview"); }
                target = Some(next);
            }
        }
        target.ok_or("no resource exec transition")
    }
}
impl Subject {
    pub fn reserve_exec_target(&self, target: u32, credentials: &Credentials) -> Result<(), &'static str> {
        if self.transition(target, credentials).is_ok() { return Ok(()); }
        let ids = credentials.principals();
        if ids == Identities::all(ids.active) && self.policy.snapshot().exec_transitions.iter().any(
            |(&(source, _, principal), &next)| source == self.label && principal == ids.active.0 && next == target) {
            Ok(())
        } else { Err("exec target not delegated") }
    }
    pub fn resource_exec(&self, resource: u32, credentials: &Credentials) -> Result<Subject, &'static str> {
        let ids = credentials.principals();
        if ids != Identities::all(ids.active) { return Err("mixed exec identities"); }
        let snapshot=self.policy.snapshot();
        let target = snapshot.exec_transitions.get(&(self.label, resource, ids.active.0)).ok_or("resource exec not delegated")?;
        self.policy.subject(*target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::{VfsManager, FileType};
    use alloc::{vec,string::ToString};
    #[test_case]
    fn resource_labels_follow_inode_identity_and_do_not_transfer_to_replacement_paths() {
        let fs = VfsManager::new();
        fs.create_file("/image",FileType::RegularFile).unwrap();
        let policy = Policy::new(vec![(1,"image".to_string()),(2,"other".to_string())],vec![]).unwrap();
        let original = fs.resolve_path("/image").unwrap().0.node();
        policy.bind_resource(original.clone(),1).unwrap();
        assert!(policy.bind_resource(original.clone(),2).is_err());
        fs.create_hardlink("/image","/alias").unwrap();
        assert_eq!(policy.resource_label(fs.resolve_path("/alias").unwrap().0.node().as_ref()),Some(1));
        fs.rename("/image","/renamed").unwrap();
        assert_eq!(policy.resource_label(fs.resolve_path("/renamed").unwrap().0.node().as_ref()),Some(1));
        fs.remove("/renamed").unwrap();
        fs.create_file("/image",FileType::RegularFile).unwrap();
        assert_eq!(policy.resource_label(fs.resolve_path("/image").unwrap().0.node().as_ref()),None);
        assert_eq!(policy.resource_label(original.as_ref()),Some(1));
        let foreign = Policy::new(vec![(1,"image".to_string())],vec![]).unwrap();
        assert_eq!(foreign.resource_label(original.as_ref()),None);
    }
}
