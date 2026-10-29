//! Per-task mount namespace mappings, independent of execution Environment.
//! A split snapshots the currently selected Environment's views and inherited
//! mappings. Explicitly supplied views outside this set retain their capability
//! identity; joining another Environment does not silently copy its resources.

use alloc::{sync::Arc, vec::Vec};

use super::manager::{VfsManager, VfsView};

#[derive(Clone)]
struct Binding {
    source: Arc<VfsView>,
    local: Arc<VfsView>,
}

const MAX_VIEW_BINDINGS: usize = 1024;

pub struct MountNamespace {
    bindings: Vec<Binding>,
}

impl MountNamespace {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            bindings: Vec::new(),
        })
    }

    pub fn resolve(&self, view: &Arc<VfsView>) -> Arc<VfsView> {
        self.bindings
            .iter()
            .find(|binding| Arc::ptr_eq(&binding.source, view))
            .map(|binding| binding.local.clone())
            .unwrap_or_else(|| view.clone())
    }

    /// Build all copies before publication. Aliases of one view remain aliases;
    /// repeated splits copy this namespace's mounts, not the original mounts.
    pub fn split(&self, roots: &[Arc<VfsView>]) -> Result<Arc<Self>, &'static str> {
        let _composition = crate::executor::syscall::lock_composition();
        let mut bindings = self.bindings.clone();
        for root in roots {
            if !bindings
                .iter()
                .any(|entry| Arc::ptr_eq(&entry.source, root))
            {
                if bindings.len() >= MAX_VIEW_BINDINGS {
                    return Err("mount namespace view limit");
                }
                bindings.push(Binding {
                    source: root.clone(),
                    local: self.resolve(root),
                });
            }
        }
        let mut copies: Vec<(Arc<VfsView>, Arc<VfsView>)> = Vec::new();
        for entry in &mut bindings {
            let copied = if let Some((_, copy)) = copies
                .iter()
                .find(|(view, _)| Arc::ptr_eq(view, &entry.local))
            {
                copy.clone()
            } else {
                let copy = VfsManager::clone_mount_namespace_deep(&VfsManager::from_view(
                    entry.local.clone(),
                ))
                .map_err(|_| "mount namespace copy failed")?
                .view();
                copies.push((entry.local.clone(), copy.clone()));
                copy
            };
            entry.local = copied;
        }
        // Current-view capabilities may be used as roots in a later explicit
        // Environment. Preserve their aliases across this split as well.
        for (source, local) in copies {
            if !bindings
                .iter()
                .any(|entry| Arc::ptr_eq(&entry.source, &source))
            {
                if bindings.len() >= MAX_VIEW_BINDINGS {
                    return Err("mount namespace view limit");
                }
                bindings.push(Binding { source, local });
            }
        }
        Ok(Arc::new(Self { bindings }))
    }
}

impl crate::task::Task {
    pub(crate) fn prepare_mount_namespace(
        &self,
    ) -> Result<(Arc<MountNamespace>, Arc<VfsManager>), &'static str> {
        let source = self.get_vfs().ok_or("missing filesystem context")?;
        let mut roots = self
            .execution_environment
            .read()
            .as_ref()
            .map(|env| {
                env.roots()
                    .into_iter()
                    .map(|(_, view)| view)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        roots.push(source.view());
        let inherited = self.mount_namespace.read().clone();
        let namespace = inherited.split(&roots)?;
        let fs = VfsManager::from_view(namespace.resolve(&source.view()));
        fs.set_cwd_by_path(&source.get_cwd_path())
            .map_err(|_| "mount namespace working directory")?;
        Ok((namespace, fs))
    }

    pub(crate) fn install_mount_namespace(
        &self,
        namespace: Arc<MountNamespace>,
        fs: Arc<VfsManager>,
    ) {
        let irq = crate::sync::IrqGuard::new();
        let old_namespace = core::mem::replace(&mut *self.mount_namespace.write(), namespace);
        let old_fs = core::mem::replace(&mut *self.vfs.write(), Some(fs));
        drop(irq);
        drop((old_namespace, old_fs));
    }
}
