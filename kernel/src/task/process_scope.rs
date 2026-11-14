//! Explicit lifecycle membership and orphan reaping, independent of PID namespaces.
use crate::{
    sync::IrqSpinLock,
    task::{Task, TaskState, TaskType},
};
use alloc::{
    collections::BTreeMap,
    sync::{Arc, Weak},
    vec::Vec,
};

#[derive(Default)]
struct State {
    closing: bool,
    members: BTreeMap<usize, Weak<Task>>,
    reaper: Weak<Task>,
}
pub struct ProcessScope {
    state: IrqSpinLock<State>,
}
impl ProcessScope {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: IrqSpinLock::new(State::default()),
        })
    }
    fn validate_member(state: &State, task: &Task) -> Result<(), &'static str> {
        if state.closing {
            return Err("process scope is closing");
        }
        if task.task_type != TaskType::User || task.get_id() == 1 {
            return Err("invalid scope member");
        }
        if let Some(reaper) = state.reaper.upgrade() {
            if reaper
                .get_namespace()
                .resolve_local_id(task.get_id())
                .is_none()
            {
                return Err("member is outside reaper visibility");
            }
        }
        Ok(())
    }
    /// Enter once; fork, spawn and exec retain membership without needing a handle.
    pub fn enter(self: &Arc<Self>, task: &Arc<Task>) -> Result<(), &'static str> {
        if !task.can_replace_exec_image() {
            return Err("scope entry requires exclusive process");
        }
        let mut state = self.state.lock();
        Self::validate_member(&state, task)?;
        let mut current = task.process_scope.write();
        if let Some(old) = current.as_ref() {
            return if Arc::ptr_eq(old, self) {
                Ok(())
            } else {
                Err("process already has a scope")
            };
        }
        state.members.insert(task.get_id(), Arc::downgrade(task));
        *current = Some(self.clone());
        Ok(())
    }
    /// Serialize task publication with closure: a concurrent fork cannot escape termination.
    /// The callback only publishes to the task pool and must not reenter a scope.
    pub(crate) fn publish(
        &self,
        task: &Arc<Task>,
        publish: impl FnOnce(),
    ) -> Result<(), &'static str> {
        let mut state = self.state.lock();
        Self::validate_member(&state, task)?;
        state.members.insert(task.get_id(), Arc::downgrade(task));
        publish();
        Ok(())
    }
    pub(crate) fn retire(&self, id: usize) {
        self.state.lock().members.remove(&id);
    }
    /// A controller may nominate a member, or explicitly consent to reap itself.
    pub fn set_reaper(
        self: &Arc<Self>,
        caller: &Task,
        target: &Arc<Task>,
    ) -> Result<(), &'static str> {
        if target.task_type != TaskType::User
            || target.get_id() != target.get_thread_group_id()
            || target.exiting.load(core::sync::atomic::Ordering::Acquire)
            || matches!(
                target.get_state(),
                TaskState::Zombie | TaskState::Terminated
            )
        {
            return Err("invalid reaper");
        }
        if caller.get_id() != target.get_id()
            && !target
                .process_scope
                .read()
                .as_ref()
                .is_some_and(|s| Arc::ptr_eq(s, self))
        {
            return Err("reaper is neither a member nor the consenting caller");
        }
        let mut state = self.state.lock();
        if state.closing {
            return Err("process scope is closing");
        }
        let namespace = target.get_namespace();
        if state
            .members
            .keys()
            .any(|&id| namespace.resolve_local_id(id).is_none())
        {
            return Err("reaper cannot see all members");
        }
        state.reaper = Arc::downgrade(target);
        Ok(())
    }
    pub(crate) fn reaper_for(&self, child: &Task) -> Option<Arc<Task>> {
        self.state.lock().reaper.upgrade().filter(|reaper| {
            reaper.get_id() != child.get_id()
                && reaper
                    .get_namespace()
                    .resolve_local_id(child.get_id())
                    .is_some()
                && !reaper.exiting.load(core::sync::atomic::Ordering::Acquire)
                && !matches!(
                    reaper.get_state(),
                    TaskState::Zombie | TaskState::Terminated
                )
        })
    }
    /// Closing is irreversible. Retry termination if event delivery reports failure.
    pub fn terminate(&self) -> Result<(), &'static str> {
        use crate::ipc::event::{Event, EventManager, EventPriority, ProcessControlType};
        let members = {
            let mut state = self.state.lock();
            state.closing = true;
            state
                .members
                .values()
                .filter_map(Weak::upgrade)
                .collect::<Vec<_>>()
        };
        let mut failed = false;
        for task in members {
            if matches!(task.get_state(), TaskState::Zombie | TaskState::Terminated) {
                continue;
            }
            let id = u32::try_from(task.get_id()).map_err(|_| "task id exceeds event ABI")?;
            let event = Event::direct_process_control(
                id,
                ProcessControlType::Kill,
                EventPriority::Critical,
                true,
            );
            failed |= EventManager::get_manager().send_event(event).is_err();
        }
        if failed {
            Err("scope termination delivery failed; retry required")
        } else {
            Ok(())
        }
    }
    pub fn status(&self) -> (bool, u32, u32) {
        let (closing, members) = {
            let state = self.state.lock();
            (
                state.closing,
                state
                    .members
                    .values()
                    .filter_map(Weak::upgrade)
                    .collect::<Vec<_>>(),
            )
        };
        let live = members
            .iter()
            .filter(|task| !matches!(task.get_state(), TaskState::Zombie | TaskState::Terminated))
            .count();
        (closing, live as u32, members.len() as u32)
    }
}

pub mod syscall {
    use super::*;
    use crate::{
        arch::Trapframe,
        object::{KernelObject, handle::SpecialSemantics},
    };
    fn scope(task: &Task, handle: usize) -> Result<Arc<ProcessScope>, &'static str> {
        match task
            .handle_table
            .get(u32::try_from(handle).map_err(|_| "invalid handle")?)
        {
            Some(KernelObject::ProcessScope(scope)) => Ok(scope),
            _ => Err("process scope capability required"),
        }
    }
    pub fn sys_process_scope_create(tf: &mut Trapframe) -> usize {
        let task = crate::task::mytask().unwrap();
        tf.increment_pc_next(&task);
        let result = (|| {
            let handle = task
                .handle_table
                .insert(KernelObject::ProcessScope(ProcessScope::new()))?;
            let mut metadata = task
                .handle_table
                .get_metadata(handle)
                .ok_or("handle metadata")?;
            metadata.special_semantics = Some(SpecialSemantics::CloseOnExec);
            task.handle_table.update_metadata(handle, metadata)?;
            Ok::<_, &'static str>(handle as usize)
        })();
        result.unwrap_or(usize::MAX)
    }
    pub fn sys_process_scope_enter(tf: &mut Trapframe) -> usize {
        let task = crate::task::mytask().unwrap();
        tf.increment_pc_next(&task);
        let result = scope(&task, tf.get_arg(0)).and_then(|scope| {
            let owned =
                crate::sched::scheduler::get_task_by_id(task.get_id()).ok_or("task unavailable")?;
            scope.enter(&owned)
        });
        result.map_or(usize::MAX, |_| 0)
    }
    pub fn sys_process_scope_set_reaper(tf: &mut Trapframe) -> usize {
        let task = crate::task::mytask().unwrap();
        tf.increment_pc_next(&task);
        let result = scope(&task, tf.get_arg(0)).and_then(|scope| {
            let id = if tf.get_arg(1) == 0 {
                task.get_id()
            } else {
                task.get_namespace()
                    .resolve_global_id(tf.get_arg(1))
                    .ok_or("reaper not visible")?
            };
            let target = crate::sched::scheduler::get_task_by_id(id).ok_or("reaper unavailable")?;
            scope.set_reaper(&task, &target)
        });
        result.map_or(usize::MAX, |_| 0)
    }
    pub fn sys_process_scope_terminate(tf: &mut Trapframe) -> usize {
        let task = crate::task::mytask().unwrap();
        tf.increment_pc_next(&task);
        scope(&task, tf.get_arg(0))
            .and_then(|scope| scope.terminate())
            .map_or(usize::MAX, |_| 0)
    }
    pub fn sys_process_scope_status(tf: &mut Trapframe) -> usize {
        let task = crate::task::mytask().unwrap();
        tf.increment_pc_next(&task);
        let result = scope(&task, tf.get_arg(0)).and_then(|scope| {
            let (closing, live, members) = scope.status();
            let mut bytes = [0u8; 16];
            bytes[..4].copy_from_slice(&u32::from(closing).to_le_bytes());
            bytes[4..8].copy_from_slice(&live.to_le_bytes());
            bytes[8..12].copy_from_slice(&members.to_le_bytes());
            crate::library::std::usercopy::copy_to_user(&task, tf.get_arg(1), &bytes)
                .map_err(|_| "invalid status pointer")
        });
        result.map_or(usize::MAX, |_| 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sched::scheduler::{
        get_all_task_ids, get_task_by_id, register_task, reset, try_register_task,
    };
    fn task(name: &str) -> Arc<Task> {
        let mut task = crate::task::new_user_task(name.into(), 0);
        task.init();
        get_task_by_id(register_task(task)).unwrap()
    }
    fn setup() -> Arc<Task> {
        reset();
        task("host")
    }
    #[test_case]
    fn scope_membership_cannot_be_replaced_and_survives_fork_exec_without_a_handle() {
        let _host = setup();
        let member = task("member");
        let scope = ProcessScope::new();
        scope.enter(&member).unwrap();
        scope.enter(&member).unwrap();
        assert!(ProcessScope::new().enter(&member).is_err());
        let child = member
            .clone_task(crate::task::CloneFlags::default())
            .unwrap();
        assert!(Arc::ptr_eq(&child.lifecycle_scope().unwrap(), &scope));
        let child = get_task_by_id(register_task(child)).unwrap();
        let image = child.new_exec_image();
        assert!(Arc::ptr_eq(&image.lifecycle_scope().unwrap(), &scope));
        assert_eq!(scope.status(), (false, 2, 2));
    }
    #[test_case]
    fn explicitly_selected_non_pid_one_reaper_adopts_orphans() {
        let host = setup();
        let reaper = task("reaper");
        let parent = task("parent");
        assert_ne!(reaper.get_namespace_id(), 1);
        let scope = ProcessScope::new();
        scope.enter(&reaper).unwrap();
        scope.enter(&parent).unwrap();
        scope.set_reaper(&host, &reaper).unwrap();
        let child = get_task_by_id(register_task(
            parent
                .clone_task(crate::task::CloneFlags::default())
                .unwrap(),
        ))
        .unwrap();
        assert!(parent.adopt_registered_child(&child));
        parent.reparent_children();
        assert_eq!(child.get_parent_id(), Some(reaper.get_id()));
        assert!(reaper.get_children().contains(&child.get_id()));
        child.exit(17);
        assert_eq!(reaper.wait(child.get_id()).unwrap(), 17);
    }
    #[test_case]
    fn reaper_requires_consent_or_membership_and_complete_pid_visibility() {
        let host = setup();
        let member = task("member");
        let outsider = task("outsider");
        let scope = ProcessScope::new();
        scope.enter(&member).unwrap();
        assert!(scope.set_reaper(&host, &outsider).is_err());
        scope.set_reaper(&outsider, &outsider).unwrap();
        let hidden =
            crate::task::namespace::TaskNamespace::new_child(host.get_namespace(), "hidden".into());
        outsider.set_namespace(hidden);
        assert!(scope.set_reaper(&outsider, &outsider).is_err());
        assert!(scope.reaper_for(&member).is_none());
        // Restore an explicit visible reaper before admitting more processes.
        scope.set_reaper(&host, &member).unwrap();
    }
    #[test_case]
    fn closing_blocks_both_entry_and_inflight_clone_publication_without_pool_leaks() {
        let _host = setup();
        let member = task("member");
        let outsider = task("outsider");
        let scope = ProcessScope::new();
        scope.enter(&member).unwrap();
        let child = member
            .clone_task(crate::task::CloneFlags::default())
            .unwrap();
        let before = get_all_task_ids().len();
        scope.terminate().unwrap();
        assert!(scope.enter(&outsider).is_err());
        assert!(try_register_task(child).is_err());
        assert_eq!(get_all_task_ids().len(), before);
        assert!(scope.status().0);
        assert_eq!(scope.status().2, 1);
        assert!(outsider.lifecycle_scope().is_none());
        assert!(scope.set_reaper(&outsider, &outsider).is_err());
    }
    #[test_case]
    fn scope_entry_rejects_shared_address_spaces() {
        let _host = setup();
        let member = task("member");
        let shared = member
            .clone_task(crate::task::CloneFlags::from_raw(
                crate::task::CloneFlagsDef::Vm as u64,
            ))
            .unwrap();
        assert!(ProcessScope::new().enter(&member).is_err());
        drop(shared);
    }
}
