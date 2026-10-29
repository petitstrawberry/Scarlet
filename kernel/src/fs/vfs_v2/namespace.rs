//! Per-task mount namespace mappings, independent of execution Environment.
//! A split snapshots the currently selected Environment's views and inherited
//! mappings. Explicitly supplied views outside this set retain their capability
//! identity; joining another Environment does not silently copy its resources.

use alloc::{sync::Arc, vec::Vec};
use crate::sync::{IrqRwSpinLock, sequence::IdSequence};
static IDS: IdSequence = IdSequence::new();

use super::manager::{VfsManager, VfsView};

#[derive(Clone)]
struct Binding {
    source: Arc<VfsView>,
    local: Arc<VfsView>,
}

const MAX_VIEW_BINDINGS: usize = 1024;

pub struct MountNamespace {
    id: u64,
    bindings: IrqRwSpinLock<Vec<Binding>>,
}

impl MountNamespace {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            id: IDS.reserve().expect("MountNamespace IDs exhausted").get(),
            bindings: IrqRwSpinLock::new(Vec::new()),
        })
    }

    pub fn resolve(&self, view: &Arc<VfsView>) -> Arc<VfsView> {
        self.bindings.read()
            .iter()
            .find(|binding| Arc::ptr_eq(&binding.source, view))
            .map(|binding| binding.local.clone())
            .unwrap_or_else(|| view.clone())
    }

    pub fn id(&self) -> u64 { self.id }
    pub fn contains_view(&self, view: &Arc<VfsView>) -> bool {
        self.bindings.read().iter().any(|binding| Arc::ptr_eq(&binding.source, view))
    }
    fn source_view(&self, local: &Arc<VfsView>) -> Arc<VfsView> {
        self.bindings.read().iter().find(|binding| Arc::ptr_eq(&binding.local, local))
            .map_or_else(|| local.clone(), |binding| binding.source.clone())
    }
    /// Materialize only identity mappings already available to this caller.
    /// This retains implicit roots in an initial namespace without changing its
    /// identity or replacing any existing private View correspondence.
    fn retain_roots(&self, roots: &[Arc<VfsView>]) -> Result<(), &'static str> {
        let mut bindings = self.bindings.write();
        let mut missing = Vec::new();
        for root in roots {
            if !bindings.iter().any(|binding| Arc::ptr_eq(&binding.source, root))
                && !missing.iter().any(|old| Arc::ptr_eq(old, root)) { missing.push(root.clone()); }
        }
        if bindings.len() + missing.len() > MAX_VIEW_BINDINGS { return Err("mount namespace view limit"); }
        bindings.extend(missing.into_iter().map(|root| Binding { source: root.clone(), local: root }));
        Ok(())
    }

    /// Build all copies before publication. Aliases of one view remain aliases;
    /// repeated splits copy this namespace's mounts, not the original mounts.
    pub fn split(&self, roots: &[Arc<VfsView>]) -> Result<Arc<Self>, &'static str> {
        let _composition = crate::executor::syscall::lock_composition();
        let mut bindings = self.bindings.read().clone();
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
        Ok(Arc::new(Self { id: IDS.reserve().expect("MountNamespace IDs exhausted").get(), bindings: IrqRwSpinLock::new(bindings) }))
    }
}

impl crate::task::Task {
    pub(crate) fn capture_mount_namespace(&self) -> Result<Arc<MountNamespace>, &'static str> {
        let namespace = self.mount_namespace.read().clone();
        let fs = self.get_vfs().ok_or("missing filesystem context")?;
        let mut roots: Vec<_> = self.execution_environment.read().as_ref()
            .map(|env| env.roots().into_iter().map(|(_, view)| view).collect()).unwrap_or_default();
        roots.push(namespace.source_view(&fs.view()));
        namespace.retain_roots(&roots)?;
        Ok(namespace)
    }

    pub(crate) fn join_mount_namespace(&self, namespace: Arc<MountNamespace>) -> Result<(), JoinError> {
        if !self.credentials.read().permits(crate::shield::identity::MANAGE_MOUNTS) { return Err(JoinError::Authority); }
        let current = self.get_vfs().ok_or(JoinError::Context)?;
        // Joining never changes another thread/process's CLONE_FS context.
        if crate::sched::scheduler::get_task_pool().task_ids_snapshot().into_iter().any(|id| {
            crate::sched::scheduler::get_task_by_id(id).is_some_and(|other| {
                other.get_id() != self.get_id()
                    && !matches!(other.get_state(), crate::task::TaskState::Zombie | crate::task::TaskState::Terminated)
                    && other.get_vfs().is_some_and(|fs| Arc::ptr_eq(&fs, &current))
            })
        }) { return Err(JoinError::SharedContext); }
        let _composition = crate::executor::syscall::lock_composition();
        let environment = self.execution_environment.read().clone();
        let source = if let Some(env) = environment {
            if env.roots().iter().any(|(_, view)| !namespace.contains_view(view)) { return Err(JoinError::IncompatibleView); }
            env.root(&self.with_default_abi(|abi| abi.get_name())).map_err(|_| JoinError::IncompatibleView)?
        } else {
            self.mount_namespace.read().source_view(&current.view())
        };
        if !namespace.contains_view(&source) { return Err(JoinError::IncompatibleView); }
        let fs = VfsManager::from_view(namespace.resolve(&source));
        fs.set_cwd_by_path(&current.get_cwd_path()).map_err(|_| JoinError::WorkingDirectory)?;
        self.install_mount_namespace(namespace, fs);
        Ok(())
    }

    pub(crate) fn prepare_mount_namespace(
        &self,
    ) -> Result<(Arc<MountNamespace>, Arc<VfsManager>), &'static str> {
        if !self
            .credentials
            .read()
            .permits(crate::shield::identity::MANAGE_MOUNTS)
        {
            return Err("mount namespace authority required");
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JoinError { Authority, SharedContext, IncompatibleView, Context, WorkingDirectory }

pub mod syscall {
    use super::*;
    use crate::{arch::Trapframe, object::{KernelObject, handle::SpecialSemantics}};
    pub fn sys_mount_namespace_current(tf: &mut Trapframe) -> usize {
        let task = crate::task::mytask().unwrap(); tf.increment_pc_next(&task);
        (|| -> Result<usize, &'static str> {
            let namespace = task.capture_mount_namespace()?;
            let handle = task.handle_table.insert(KernelObject::MountNamespace(namespace))?;
            let mut metadata = task.handle_table.get_metadata(handle).ok_or("handle metadata")?;
            metadata.special_semantics = Some(SpecialSemantics::CloseOnExec);
            task.handle_table.update_metadata(handle, metadata)?;
            Ok(handle as usize)
        })().unwrap_or(usize::MAX)
    }
    pub fn sys_mount_namespace_join(tf: &mut Trapframe) -> usize {
        let task = crate::task::mytask().unwrap(); tf.increment_pc_next(&task);
        let Ok(handle) = u32::try_from(tf.get_arg(0)) else { return usize::MAX };
        match task.handle_table.get(handle) {
            Some(KernelObject::MountNamespace(namespace)) => task.join_mount_namespace(namespace).map_or(usize::MAX, |_| 0),
            _ => usize::MAX,
        }
    }
    pub fn sys_mount_namespace_identity(tf: &mut Trapframe) -> usize {
        let task = crate::task::mytask().unwrap(); tf.increment_pc_next(&task);
        let Ok(handle) = u32::try_from(tf.get_arg(0)) else { return usize::MAX };
        match task.handle_table.get(handle) {
            Some(KernelObject::MountNamespace(namespace)) => crate::library::std::usercopy::copy_to_user(&task, tf.get_arg(1), &namespace.id().to_le_bytes()).map_or(usize::MAX, |_| 0),
            _ => usize::MAX,
        }
    }
}
