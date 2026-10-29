//! Native policy construction and explicit subject assignment. Management
//! authority is carried by a close-on-exec capability, never a numeric UID.
use super::*;
use crate::{
    arch::Trapframe,
    library::std::usercopy::copy_from_user,
    object::{KernelObject, handle::SpecialSemantics},
    task::Task,
};
use core::sync::atomic::Ordering;

fn insert(task: &Task, cap: PolicyHandle) -> Result<usize, &'static str> {
    let handle = task.handle_table.insert(KernelObject::ShieldPolicy(cap))?;
    let mut metadata = task
        .handle_table
        .get_metadata(handle)
        .ok_or("handle metadata")?;
    metadata.special_semantics = Some(SpecialSemantics::CloseOnExec);
    task.handle_table.update_metadata(handle, metadata)?;
    Ok(handle as usize)
}
pub fn policy_handle(task: &Task, handle: usize, admin: bool) -> Result<Arc<Policy>, &'static str> {
    match task
        .handle_table
        .get(u32::try_from(handle).map_err(|_| "invalid handle")?)
    {
        Some(KernelObject::ShieldPolicy(cap)) if !admin || cap.administer => Ok(cap.policy),
        _ => Err("policy authority required"),
    }
}
fn can_create(task: &Task) -> bool {
    task.bootstrap_environment.load(Ordering::Acquire) || task.handle_table.active_handles().iter().any(|&h| {
        matches!(task.handle_table.get(h), Some(KernelObject::ShieldPolicy(cap)) if cap.administer)
    })
}
fn read<const N: usize>(task: &Task, ptr: usize) -> Result<[u8; N], &'static str> {
    let mut bytes = [0; N];
    copy_from_user(task, ptr, &mut bytes).map_err(|_| "invalid pointer")?;
    Ok(bytes)
}
fn u32_at(b: &[u8], n: usize) -> u32 {
    u32::from_le_bytes(b[n..n + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], n: usize) -> u64 {
    u64::from_le_bytes(b[n..n + 8].try_into().unwrap())
}
fn address(value: u64) -> Result<usize, &'static str> {
    usize::try_from(value).map_err(|_| "invalid address")
}

fn create(
    task: &Task,
    labels_ptr: usize,
    labels_count: usize,
    rules_ptr: usize,
    rules_count: usize,
    transitions_ptr: usize,
    transitions_count: usize,
) -> Result<usize, &'static str> {
    if !can_create(task) {
        return Err("policy construction authority required");
    }
    if labels_count == 0
        || labels_count > MAX_LABELS
        || rules_count > MAX_RULES
        || transitions_count > MAX_RULES
    {
        return Err("policy size limit");
    }
    labels_ptr
        .checked_add(labels_count * 16)
        .ok_or("invalid labels pointer")?;
    rules_ptr
        .checked_add(rules_count * 24)
        .ok_or("invalid rules pointer")?;
    transitions_ptr
        .checked_add(transitions_count * 16)
        .ok_or("invalid transitions pointer")?;
    let mut transitions = Vec::with_capacity(transitions_count);
    for i in 0..transitions_count {
        let raw = read::<16>(task, transitions_ptr + i * 16)?;
        if u32_at(&raw, 12) != 0 {
            return Err("unsupported transition flags");
        }
        transitions.push(TransitionRule {
            source: u32_at(&raw, 0),
            target: u32_at(&raw, 4),
            principal: identity::Principal(u32_at(&raw, 8)),
        });
    }
    let mut labels = Vec::with_capacity(labels_count);
    for i in 0..labels_count {
        let raw = read::<16>(task, labels_ptr + i * 16)?;
        let size = u32_at(&raw, 4) as usize;
        if size == 0 || size > MAX_LABEL_BYTES {
            return Err("label size limit");
        }
        let mut name = alloc::vec![0;size];
        copy_from_user(task, address(u64_at(&raw, 8))?, &mut name)
            .map_err(|_| "invalid label pointer")?;
        labels.push((
            u32_at(&raw, 0),
            String::from_utf8(name).map_err(|_| "label is not UTF-8")?,
        ));
    }
    let mut rules = Vec::with_capacity(rules_count);
    for i in 0..rules_count {
        let raw = read::<24>(task, rules_ptr + i * 24)?;
        if u32_at(&raw, 12) != 0 {
            return Err("unsupported rule flags");
        }
        rules.push(Rule {
            source: u32_at(&raw, 0),
            target: u32_at(&raw, 4),
            class: u32_at(&raw, 8),
            permissions: u64_at(&raw, 16),
        });
    }
    insert(
        task,
        PolicyHandle {
            policy: Policy::with_transitions(labels, rules, transitions)?,
            administer: true,
        },
    )
}
pub fn apply(task: &Task, handle: usize, label: usize) -> Result<(), &'static str> {
    let policy = policy_handle(task, handle, true)?;
    let subject = policy.subject(u32::try_from(label).map_err(|_| "invalid label")?)?;
    if !task.can_replace_exec_image() {
        return Err("subject assignment requires an exclusive single-threaded process");
    }
    *task.security_subject.write() = Some(subject);
    Ok(())
}
pub fn sys_shield_policy_create(tf: &mut Trapframe) -> usize {
    let task = crate::task::mytask().unwrap();
    tf.increment_pc_next(&task);
    create(
        &task,
        tf.get_arg(0),
        tf.get_arg(1),
        tf.get_arg(2),
        tf.get_arg(3),
        0,
        0,
    )
    .unwrap_or(usize::MAX)
}
pub fn sys_shield_policy_create_with_transitions(tf: &mut Trapframe) -> usize {
    let task = crate::task::mytask().unwrap();
    tf.increment_pc_next(&task);
    create(
        &task,
        tf.get_arg(0),
        tf.get_arg(1),
        tf.get_arg(2),
        tf.get_arg(3),
        tf.get_arg(4),
        tf.get_arg(5),
    )
    .unwrap_or(usize::MAX)
}
/// No management capability is needed here: the immutable policy explicitly
/// delegates this particular transition. Both policy and current subject are
/// rechecked at use; open descriptors confer no label-changing authority.
pub fn transition(task: &Task, policy: &Arc<Policy>, target: u32) -> Result<(), &'static str> {
    if !task.can_replace_exec_image() {
        return Err("transition requires exclusive single-threaded task");
    }
    let credentials = task.credentials.read();
    let mut current = task.security_subject.write();
    let subject = current.as_ref().ok_or("task has no subject")?;
    if !Arc::ptr_eq(subject.policy(), policy) {
        return Err("foreign transition policy");
    }
    let next = subject.transition(target, &credentials)?;
    *current = Some(next);
    Ok(())
}

pub fn sys_shield_reserve_exec(tf: &mut Trapframe) -> usize {
    let task = crate::task::mytask().unwrap(); tf.increment_pc_next(&task);
    (|| -> Result<(), &'static str> {
        let policy = policy_handle(&task, tf.get_arg(0), false)?;
        crate::shield::exec_transition::reserve(&task, &policy, u32::try_from(tf.get_arg(1)).map_err(|_| "invalid label")?)
    })().map_or(usize::MAX, |_| 0)
}
pub fn sys_shield_clear_exec(tf: &mut Trapframe) -> usize {
    let task = crate::task::mytask().unwrap(); tf.increment_pc_next(&task);
    crate::shield::exec_transition::cancel(&task).map_or(usize::MAX, |_| 0)
}
pub fn sys_shield_constrain_information(tf: &mut Trapframe) -> usize {
    let task = crate::task::mytask().unwrap(); tf.increment_pc_next(&task);
    let Ok(flags) = u32::try_from(tf.get_arg(0)) else { return usize::MAX };
    crate::shield::information::constrain(&task, flags).map_or(usize::MAX, |_| 0)
}
pub fn sys_shield_information(tf: &mut Trapframe) -> usize {
    let task = crate::task::mytask().unwrap(); tf.increment_pc_next(&task);
    crate::shield::information::constraints(&task) as usize
}

pub fn sys_shield_transition(tf: &mut Trapframe) -> usize {
    let task = crate::task::mytask().unwrap();
    let handle = tf.get_arg(0);
    let target = tf.get_arg(1);
    tf.increment_pc_next(&task);
    policy_handle(&task, handle, false)
        .and_then(|policy| {
            transition(
                &task,
                &policy,
                u32::try_from(target).map_err(|_| "invalid label")?,
            )
        })
        .map_or(usize::MAX, |_| 0)
}

pub fn sys_shield_policy_current(tf: &mut Trapframe) -> usize {
    let task = crate::task::mytask().unwrap();
    tf.increment_pc_next(&task);
    let subject = task.security_subject.read().clone();
    subject
        .and_then(|s| {
            insert(
                &task,
                PolicyHandle {
                    policy: s.policy,
                    administer: false,
                },
            )
            .ok()
        })
        .unwrap_or(usize::MAX)
}
pub fn sys_shield_apply(tf: &mut Trapframe) -> usize {
    let task = crate::task::mytask().unwrap();
    tf.increment_pc_next(&task);
    apply(&task, tf.get_arg(0), tf.get_arg(1)).map_or(usize::MAX, |_| 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test_case]
    fn security_query_capability_cannot_assign_labels_or_construct_policy() {
        let task = crate::task::new_user_task("security-test".into(), 1);
        let policy = Policy::new(alloc::vec![(1, "subject".into())], Vec::new()).unwrap();
        let h = insert(
            &task,
            PolicyHandle {
                policy: policy.clone(),
                administer: false,
            },
        )
        .unwrap();
        assert!(apply(&task, h, 1).is_err());
        assert!(!can_create(&task));
        let h = insert(
            &task,
            PolicyHandle {
                policy,
                administer: true,
            },
        )
        .unwrap();
        assert!(can_create(&task));
        assert!(apply(&task, h, 2).is_err());
        assert!(task.security_subject.read().is_none());
        apply(&task, h, 1).unwrap();
        assert_eq!(
            task.security_subject.read().as_ref().unwrap().name(),
            "subject"
        );
        let image = task.new_exec_image();
        assert!(
            image
                .security_subject
                .read()
                .as_ref()
                .unwrap()
                .same_identity(task.security_subject.read().as_ref().unwrap())
        );
        assert_eq!(
            task.handle_table
                .get_metadata(h as u32)
                .unwrap()
                .special_semantics,
            Some(SpecialSemantics::CloseOnExec)
        );
    }
}
