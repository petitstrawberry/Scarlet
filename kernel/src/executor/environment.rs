//! ABI-to-filesystem mappings, independent of distribution layout.

use alloc::{collections::BTreeMap, string::String, sync::Arc};

use crate::{
    fs::vfs_v2::manager::{VfsManager, VfsView},
    sync::IrqRwSpinLock,
};

struct EnvironmentState {
    sealed: bool,
    roots: BTreeMap<String, Arc<VfsView>>,
}

/// Sealing freezes the ABI mapping, not file contents or authorized mounts.
/// Views never refer back to their environment or to tasks.
pub struct Environment {
    state: IrqRwSpinLock<EnvironmentState>,
}

impl Environment {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: IrqRwSpinLock::new(EnvironmentState {
                sealed: false,
                roots: BTreeMap::new(),
            }),
        })
    }

    pub fn set_root(&self, abi: String, view: Arc<VfsView>) -> Result<(), &'static str> {
        if crate::abi::AbiRegistry::instantiate(&abi).is_none() {
            return Err("unknown ABI");
        }
        let mut state = self.state.write();
        if state.sealed {
            return Err("environment is sealed");
        }
        if state.roots.contains_key(&abi) {
            return Err("ABI root already registered");
        }
        state.roots.insert(abi, view);
        Ok(())
    }

    pub fn remove_root(&self, abi: &str) -> Result<(), &'static str> {
        let mut state = self.state.write();
        if state.sealed {
            return Err("environment is sealed");
        }
        state.roots.remove(abi).ok_or("ABI root not registered")?;
        Ok(())
    }

    pub fn seal(&self) -> Result<(), &'static str> {
        let mut state = self.state.write();
        if state.roots.is_empty() {
            return Err("environment has no ABI roots");
        }
        state.sealed = true;
        Ok(())
    }

    pub fn root(&self, abi: &str) -> Result<Arc<VfsView>, &'static str> {
        let state = self.state.read();
        if !state.sealed {
            return Err("environment is not sealed");
        }
        state
            .roots
            .get(abi)
            .cloned()
            .ok_or("ABI unavailable in environment")
    }

    pub fn is_sealed(&self) -> bool {
        self.state.read().sealed
    }

    /// Copy every ABI mount namespace before publishing a derived environment.
    pub fn clone_mount_namespaces(&self) -> Result<Arc<Self>, &'static str> {
        let _composition = super::syscall::lock_composition();
        let roots = {
            let state = self.state.read();
            if !state.sealed {
                return Err("environment is not sealed");
            }
            state.roots.clone()
        };
        let result = Self::new();
        let mut copies: alloc::vec::Vec<(Arc<VfsView>, Arc<VfsView>)> = alloc::vec::Vec::new();
        for (abi, view) in roots {
            let copied = if let Some((_, copied)) = copies
                .iter()
                .find(|(original, _)| Arc::ptr_eq(original, &view))
            {
                copied.clone()
            } else {
                let source = VfsManager::from_view(view.clone());
                let copy = VfsManager::clone_mount_namespace_deep(&source)
                    .map_err(|_| "failed to clone mount namespace")?
                    .view();
                copies.push((view, copy.clone()));
                copy
            };
            result.set_root(abi, copied)?;
        }
        result.seal()?;
        Ok(result)
    }
}

/// Authority belongs to the handle, not the process name or handle metadata.
#[derive(Clone)]
pub struct EnvironmentHandle {
    pub environment: Arc<Environment>,
    pub writable: bool,
}

#[derive(Clone)]
pub struct ViewHandle {
    pub view: Arc<VfsView>,
    pub writable: bool,
}
