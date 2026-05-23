//! Session / process-group / controlling-terminal job-control primitives.
//!
//! This module exposes the kernel-internal building blocks used to implement
//! Linux-style `setsid(2)`, `setpgid(2)`, `getsid(2)`, `getpgid(2)`, and the
//! controlling-terminal helpers. The functions here operate on **global** task
//! IDs; namespace translation is the responsibility of the caller (typically
//! an ABI adapter or a syscall wrapper).
//!
//! See `docs/tty_pty_design.md` for the full design rationale and the
//! inheritance rules used by `Task::clone_task`.

extern crate alloc;

use alloc::sync::Arc;
use core::sync::atomic::Ordering;

use crate::device::char::tty::TtyDevice;
use crate::sched::scheduler::{get_all_task_ids, get_task_by_id};
use crate::task::Task;

/// Detach the calling task into a new session.
///
/// Mirrors `setsid(2)`. On success the caller becomes the leader of a new
/// session and the sole member of a new process group; both IDs are set to
/// the caller's global task ID. The controlling terminal is cleared.
///
/// # Arguments
/// * `task` - The task that is calling `setsid`.
///
/// # Returns
/// `Ok(sid)` where `sid` is the new session ID (= the caller's global task
/// ID), or `Err` if the caller is already a process-group leader (matching
/// Linux semantics: this prevents the caller from becoming a session leader
/// while still being a PG leader in its previous session).
pub fn setsid(task: &Task) -> Result<usize, &'static str> {
    let my_id = task.get_id();

    // Linux rule: setsid() fails with EPERM if the caller is already a
    // process group leader. The kernel detects this by checking whether any
    // other task in the same namespace has its PGID equal to the caller's
    // PID.
    if is_process_group_leader(task) {
        return Err("setsid: caller is already a process group leader");
    }

    task.set_session_id(my_id);
    task.set_process_group_id(my_id);
    task.set_session_leader(true);
    task.clear_controlling_tty_raw();
    Ok(my_id)
}

/// Move `target` into the process group identified by `pgid`.
///
/// Mirrors `setpgid(2)` semantics. `target` and `pgid` are **global** task
/// IDs. If `pgid == target_id`, a new process group is created with the
/// target as its leader.
///
/// The caller must ensure the operation is permitted (Linux's restrictions:
/// caller is the target itself or its parent, the target has not yet
/// `exec`ed if changing it from outside, target and pgid live in the same
/// session). This function enforces:
///
/// * `target` exists.
/// * `pgid` either equals `target` (new group), or names an existing task in
///   the same session as `target`.
///
/// # Arguments
/// * `caller` - The task invoking `setpgid` (used for the same-namespace
///   guard).
/// * `target_id` - Global task ID of the task whose PGID is being changed.
/// * `pgid` - Global task ID to use as the new PGID.
///
/// # Returns
/// `Ok(())` on success, or `Err` describing the violated constraint.
pub fn setpgid(caller: &Task, target_id: usize, pgid: usize) -> Result<(), &'static str> {
    let target = get_task_by_id(target_id).ok_or("setpgid: no such task")?;

    // Same-namespace check (we identify by namespace Arc pointer equality).
    let caller_ns = caller.get_namespace();
    let target_ns = target.get_namespace();
    if !Arc::ptr_eq(&caller_ns, &target_ns) {
        return Err("setpgid: target is in a different namespace");
    }

    // Session leaders cannot change their PGID (Linux: EPERM).
    if target.is_session_leader() {
        return Err("setpgid: target is a session leader");
    }

    if pgid == target_id {
        // Creating a new process group with `target` as the leader.
        target.set_process_group_id(target_id);
        return Ok(());
    }

    // Joining an existing process group: the group's leader (the task whose
    // global ID equals `pgid`) must be in the same session as `target`.
    let leader = get_task_by_id(pgid).ok_or("setpgid: process group does not exist")?;
    if leader.get_session_id() != target.get_session_id() {
        return Err("setpgid: target session differs from process group session");
    }
    target.set_process_group_id(pgid);
    Ok(())
}

/// Return the session ID of `target_id`, or `None` if it does not exist.
pub fn getsid(target_id: usize) -> Option<usize> {
    get_task_by_id(target_id).map(|t| t.get_session_id())
}

/// Return the process group ID of `target_id`, or `None` if it does not exist.
pub fn getpgid(target_id: usize) -> Option<usize> {
    get_task_by_id(target_id).map(|t| t.get_process_group_id())
}

/// Install `tty` as the controlling terminal of `task`.
///
/// This is the low-level operation; higher-level policy (such as auto-acquire
/// on `open(/dev/ttyN)` by a session leader without `O_NOCTTY`) belongs in
/// the TTY layer and will be wired up in Phase 3.
///
/// # Arguments
/// * `task` - The task whose controlling terminal is being set.
/// * `tty` - Strong reference to the TTY device. A `Weak` clone is stored.
pub fn set_controlling_tty(task: &Task, tty: &Arc<TtyDevice>) {
    task.set_controlling_tty_raw(tty);
}

/// Clear the controlling terminal of `task`.
pub fn clear_controlling_tty(task: &Task) {
    task.clear_controlling_tty_raw();
}

/// Return whether `task` is currently a process group leader, i.e. whether
/// its PGID equals its own global task ID.
fn is_process_group_leader(task: &Task) -> bool {
    task.get_process_group_id() == task.get_id()
}

/// Determine whether any task with the given PGID currently exists in the
/// system. Useful for `setpgid` validation; exposed for tests.
#[allow(dead_code)]
pub(crate) fn pgid_in_use(pgid: usize) -> bool {
    for id in get_all_task_ids() {
        if let Some(t) = get_task_by_id(id) {
            if t.get_process_group_id() == pgid {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sched::scheduler::{add_task, get_task_by_id, reset};
    use alloc::string::ToString;

    fn spawn(name: &str) -> usize {
        let mut t = crate::task::new_user_task(name.to_string(), 0);
        t.init();
        add_task(t, 0)
    }

    #[test_case]
    fn test_default_session_and_pgid_are_own_id() {
        reset();
        let id = spawn("test_default_sid_pgid");
        let task = get_task_by_id(id).unwrap();
        assert_eq!(task.get_session_id(), id);
        assert_eq!(task.get_process_group_id(), id);
        assert!(!task.is_session_leader());
        assert!(task.get_controlling_tty().is_none());
    }

    #[test_case]
    fn test_setsid_succeeds_when_not_pg_leader() {
        reset();
        let parent_id = spawn("test_setsid_parent");
        let child_id = spawn("test_setsid_child");
        let child = get_task_by_id(child_id).unwrap();
        child.set_process_group_id(parent_id);
        assert!(setsid(child).is_ok());
        assert_eq!(child.get_session_id(), child_id);
        assert_eq!(child.get_process_group_id(), child_id);
        assert!(child.is_session_leader());
        assert!(child.get_controlling_tty().is_none());
    }

    #[test_case]
    fn test_setsid_fails_when_pg_leader() {
        reset();
        let id = spawn("test_setsid_leader");
        let task = get_task_by_id(id).unwrap();
        // Default: PGID == own ID, so caller is a PG leader.
        assert!(setsid(task).is_err());
    }

    #[test_case]
    fn test_setpgid_creates_new_group() {
        reset();
        let caller_id = spawn("test_setpgid_caller");
        let caller = get_task_by_id(caller_id).unwrap();
        let target_id = spawn("test_setpgid_target");
        let target = get_task_by_id(target_id).unwrap();
        // Move target into its own group (no-op since default already is,
        // but exercises the create-new-group code path).
        assert!(setpgid(caller, target_id, target_id).is_ok());
        assert_eq!(target.get_process_group_id(), target_id);
    }

    #[test_case]
    fn test_setpgid_rejects_session_leader() {
        reset();
        let id = spawn("test_setpgid_sl");
        let task = get_task_by_id(id).unwrap();
        let helper_id = spawn("test_setpgid_sl_helper");
        task.set_process_group_id(helper_id);
        setsid(task).unwrap();
        // Task is now a session leader; setpgid must reject changing it.
        assert!(setpgid(task, id, helper_id).is_err());
    }

    #[test_case]
    fn test_getsid_getpgid() {
        reset();
        let id = spawn("test_get_helpers");
        assert_eq!(getsid(id), Some(id));
        assert_eq!(getpgid(id), Some(id));
        assert!(getsid(usize::MAX).is_none());
        assert!(getpgid(usize::MAX).is_none());
    }
}
