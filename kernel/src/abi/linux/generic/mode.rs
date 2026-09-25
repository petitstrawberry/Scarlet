//! Unix mode bits for files created or chmodded through the Linux ABI.
//!
//! Scarlet's VFS permissions are capability booleans, so retain the precise
//! Unix mode beside the VFS node for Linux stat results.

use alloc::{collections::BTreeMap, sync::Arc};

use crate::{
    fs::vfs_v2::core::VfsNode,
    sync::{IrqRwSpinLock, Once},
};

type NodeKey = (u64, u64);
static MODES: Once<IrqRwSpinLock<BTreeMap<NodeKey, u32>>> = Once::new();

fn modes() -> &'static IrqRwSpinLock<BTreeMap<NodeKey, u32>> {
    MODES.call_once(|| IrqRwSpinLock::new(BTreeMap::new()))
}

fn key(node: &Arc<dyn VfsNode>) -> Option<NodeKey> {
    let filesystem = node.filesystem()?.upgrade()?;
    Some((filesystem.fs_id().get(), node.id()))
}

pub(super) fn set(node: &Arc<dyn VfsNode>, mode: u32) {
    if let Some(key) = key(node) {
        modes().write().insert(key, mode & 0o7777);
    }
}

pub(super) fn get(node: &Arc<dyn VfsNode>) -> Option<u32> {
    modes().read().get(&key(node)?).copied()
}

pub(super) fn clear(node: &Arc<dyn VfsNode>) {
    if let Some(key) = key(node) {
        modes().write().remove(&key);
    }
}
