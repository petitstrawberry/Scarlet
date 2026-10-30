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

fn update_array<const N: usize, T>(
    task: &Task,
    pointer: u64,
    count: u64,
    bound: usize,
    mut parse: impl FnMut([u8; N]) -> Result<T, &'static str>,
) -> Result<Vec<T>, &'static str> {
    let count = address(count)?;
    let pointer = address(pointer)?;
    if count > bound {
        return Err("update array bound");
    }
    pointer
        .checked_add(count.checked_mul(N).ok_or("update array overflow")?)
        .ok_or("update pointer overflow")?;
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        values.push(parse(read::<N>(task, pointer + index * N)?)?);
    }
    Ok(values)
}
pub(crate) fn extend(
    task: &Task,
    handle: usize,
    pointer: usize,
    length: usize,
) -> Result<(), &'static str> {
    let policy = policy_handle(task, handle, true)?;
    if length != 128 {
        return Err("policy update size");
    }
    let spec = read::<128>(task, pointer)?;
    if u32_at(&spec, 0) != 1 || u32_at(&spec, 4) != 0 || spec[112..].iter().any(|byte| *byte != 0) {
        return Err("policy update version or reserved fields");
    }
    let labels = update_array::<16, _>(
        task,
        u64_at(&spec, 16),
        u64_at(&spec, 24),
        MAX_LABELS,
        |raw| {
            let length = u32_at(&raw, 4) as usize;
            if length == 0 || length > MAX_LABEL_BYTES {
                return Err("update label length");
            }
            let mut name = alloc::vec![0;length];
            copy_from_user(task, address(u64_at(&raw, 8))?, &mut name)
                .map_err(|_| "update label pointer")?;
            Ok((
                u32_at(&raw, 0),
                String::from_utf8(name).map_err(|_| "update label encoding")?,
            ))
        },
    )?;
    let rules = update_array::<24, _>(
        task,
        u64_at(&spec, 32),
        u64_at(&spec, 40),
        MAX_RULES,
        |raw| {
            if u32_at(&raw, 12) != 0 {
                return Err("update rule flags");
            }
            Ok(Rule {
                source: u32_at(&raw, 0),
                target: u32_at(&raw, 4),
                class: u32_at(&raw, 8),
                permissions: u64_at(&raw, 16),
            })
        },
    )?;
    let transitions = update_array::<16, _>(
        task,
        u64_at(&spec, 48),
        u64_at(&spec, 56),
        MAX_RULES,
        |raw| {
            if u32_at(&raw, 12) != 0 {
                return Err("update transition flags");
            }
            Ok(TransitionRule {
                source: u32_at(&raw, 0),
                target: u32_at(&raw, 4),
                principal: identity::Principal(u32_at(&raw, 8)),
            })
        },
    )?;
    let exec_transitions = update_array::<24, _>(
        task,
        u64_at(&spec, 64),
        u64_at(&spec, 72),
        MAX_RULES,
        |raw| {
            if u64_at(&raw, 16) != 0 {
                return Err("update exec flags");
            }
            Ok(ExecTransitionRule {
                source: u32_at(&raw, 0),
                resource: u32_at(&raw, 4),
                target: u32_at(&raw, 8),
                principal: identity::Principal(u32_at(&raw, 12)),
            })
        },
    )?;
    let create = update_array::<24, _>(
        task,
        u64_at(&spec, 80),
        u64_at(&spec, 88),
        MAX_RULES,
        |raw| {
            if u32_at(&raw, 16) > 1 || u32_at(&raw, 20) != 0 {
                return Err("update file flags");
            }
            Ok(file::CreateRule {
                source: u32_at(&raw, 0),
                parent: u32_at(&raw, 4),
                target: u32_at(&raw, 8),
                kind: u32_at(&raw, 12),
                default: u32_at(&raw, 16) == 1,
            })
        },
    )?;
    let relabel = update_array::<16, _>(
        task,
        u64_at(&spec, 96),
        u64_at(&spec, 104),
        MAX_RULES,
        |raw| {
            Ok(file::RelabelRule {
                source: u32_at(&raw, 0),
                old: u32_at(&raw, 4),
                target: u32_at(&raw, 8),
                kind: u32_at(&raw, 12),
            })
        },
    )?;
    policy.extend(
        u64_at(&spec, 8),
        update::PolicyUpdate {
            labels,
            rules,
            transitions,
            exec_transitions,
            create,
            relabel,
        },
    )?;
    Ok(())
}
pub fn sys_shield_policy_extend(tf: &mut Trapframe) -> usize {
    let task = crate::task::mytask().unwrap();
    tf.increment_pc_next(&task);
    extend(&task, tf.get_arg(0), tf.get_arg(1), tf.get_arg(2)).map_or(usize::MAX, |_| 0)
}
pub fn sys_shield_policy_generation(tf: &mut Trapframe) -> usize {
    let task = crate::task::mytask().unwrap();
    tf.increment_pc_next(&task);
    (|| -> Result<usize, &'static str> {
        let policy = policy_handle(&task, tf.get_arg(0), false)?;
        crate::library::std::usercopy::copy_to_user(
            &task,
            tf.get_arg(1),
            &policy.generation().to_ne_bytes(),
        )
        .map_err(|_| "generation pointer")?;
        Ok(0)
    })()
    .unwrap_or(usize::MAX)
}

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
    exec_transitions: Vec<ExecTransitionRule>,
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
            policy: Policy::with_exec_transitions(labels, rules, transitions, exec_transitions)?,
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
        Vec::new(),
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
        Vec::new(),
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

/// Extended immutable policy specification: eight u64 pointer/count fields.
pub fn sys_shield_policy_create_exec(tf: &mut Trapframe) -> usize {
    let task = crate::task::mytask().unwrap();
    let pointer = tf.get_arg(0); let length = tf.get_arg(1);
    tf.increment_pc_next(&task);
    (|| {
        if !can_create(&task) || length != 64 { return Err("invalid exec policy request"); }
        let spec = read::<64>(&task, pointer)?;
        let fields: Vec<usize> = (0..8).map(|i| address(u64_at(&spec, i * 8))).collect::<Result<_, _>>()?;
        if fields[7] > MAX_RULES { return Err("exec rule bound"); }
        fields[6].checked_add(fields[7] * 24).ok_or("exec rule pointer overflow")?;
        let mut rules = Vec::with_capacity(fields[7]);
        for i in 0..fields[7] {
            let rule = read::<24>(&task, fields[6] + i * 24)?;
            if u64_at(&rule, 16) != 0 { return Err("exec rule reserved bits"); }
            rules.push(ExecTransitionRule {source:u32_at(&rule,0),resource:u32_at(&rule,4),
                target:u32_at(&rule,8),principal:identity::Principal(u32_at(&rule,12))});
        }
        create(&task, fields[0],fields[1],fields[2],fields[3],fields[4],fields[5], rules)
    })().unwrap_or(usize::MAX)
}

pub fn bind_file(task: &Task, policy_handle_id: usize, file: usize, label: usize) -> Result<(), &'static str> {
    let policy = policy_handle(task, policy_handle_id, true)?;
    let object = task.handle_table.get(u32::try_from(file).map_err(|_| "file handle")?).ok_or("file handle")?;
    let file = object.as_file().and_then(|file| file.as_any().downcast_ref::<crate::fs::vfs_v2::core::VfsFileObject>()).ok_or("retained VFS file required")?;
    policy.bind_resource(file.get_vfs_entry().node(), u32::try_from(label).map_err(|_| "resource label")?)
}
pub fn sys_shield_bind_file(tf: &mut Trapframe) -> usize {
    let task = crate::task::mytask().unwrap();
    tf.increment_pc_next(&task);
    bind_file(&task, tf.get_arg(0),tf.get_arg(1),tf.get_arg(2)).map_or(usize::MAX, |_| 0)
}

/// Construct a new policy identity from an administrator-owned immutable blueprint.
pub fn sys_shield_file_policy(tf:&mut Trapframe)->usize {
    let task=crate::task::mytask().unwrap();tf.increment_pc_next(&task);
    (||->Result<usize,&'static str> {
        let policy=policy_handle(&task,tf.get_arg(0),true)?;
        if tf.get_arg(2)!=40 {return Err("file policy specification size");}
        let raw=read::<40>(&task,tf.get_arg(1))?;
        if u32_at(&raw,4)!=0 {return Err("file policy flags");}
        let create_count=address(u64_at(&raw,16))?;let relabel_count=address(u64_at(&raw,32))?;
        if create_count>MAX_RULES || relabel_count>MAX_RULES {return Err("file rule bound");}
        let create_ptr=address(u64_at(&raw,8))?;let relabel_ptr=address(u64_at(&raw,24))?;
        create_ptr.checked_add(create_count*24).ok_or("create pointer overflow")?;
        relabel_ptr.checked_add(relabel_count*16).ok_or("relabel pointer overflow")?;
        let mut create=Vec::with_capacity(create_count);let mut relabel=Vec::with_capacity(relabel_count);
        for index in 0..create_count {
            let record=read::<24>(&task,create_ptr+index*24)?;let flags=u32_at(&record,16);
            if flags>1 || u32_at(&record,20)!=0 {return Err("file create flags");}
            create.push(file::CreateRule {source:u32_at(&record,0),parent:u32_at(&record,4),target:u32_at(&record,8),kind:u32_at(&record,12),default:flags==1});
        }
        for index in 0..relabel_count {
            let record=read::<16>(&task,relabel_ptr+index*16)?;
            relabel.push(file::RelabelRule {source:u32_at(&record,0),old:u32_at(&record,4),target:u32_at(&record,8),kind:u32_at(&record,12)});
        }
        let default=u32_at(&raw,0);
        insert(&task,PolicyHandle {policy:policy.with_file_rules((default!=0).then_some(default),create,relabel)?,administer:true})
    })().unwrap_or(usize::MAX)
}
pub fn sys_shield_creation_label(tf:&mut Trapframe)->usize {
    let task=crate::task::mytask().unwrap();tf.increment_pc_next(&task);
    let Ok(target)=u32::try_from(tf.get_arg(0)) else {return usize::MAX;};
    file::select_creation_label(&task,(target!=0).then_some(target)).map_or(usize::MAX,|_|0)
}

pub fn sys_shield_relabel_file(tf:&mut Trapframe)->usize {
    let task=crate::task::mytask().unwrap();tf.increment_pc_next(&task);
    (||->Result<(),&'static str> {
        let policy=policy_handle(&task,tf.get_arg(0),false)?;
        let object=task.handle_table.get(u32::try_from(tf.get_arg(1)).map_err(|_|"file handle")?).ok_or("file handle")?;
        let file=object.as_file().and_then(|file|file.as_any().downcast_ref::<crate::fs::vfs_v2::core::VfsFileObject>()).ok_or("retained VFS file required")?;
        let node=file.get_vfs_entry().node();
        if node.filesystem().and_then(|weak|weak.upgrade()).is_none_or(|fs|fs.is_read_only()) {return Err("resource filesystem is read-only or gone");}
        let subject=task.security_subject.read().clone().ok_or("subject missing")?;
        policy.relabel_file(&subject,node.as_ref(),u32::try_from(tf.get_arg(2)).map_err(|_|"file label")?)
    })().map_or(usize::MAX,|_|0)
}

pub fn sys_shield_socket_creation_label(tf:&mut Trapframe)->usize {
    let task=crate::task::mytask().unwrap();let target=tf.get_arg(0);tf.increment_pc_next(&task);
    let Ok(target)=u32::try_from(target) else {return usize::MAX;};
    socket::select(&task,(target!=0).then_some(target)).map_or(usize::MAX,|_|0)
}

#[cfg(test)]
mod update_tests {
    use super::*;
    use alloc::vec;
    #[test_case]
    fn update_requires_the_admin_capability_and_copies_the_whole_batch_before_publication() {
        use crate::library::std::usercopy::copy_to_user;
        let mut task = Task::new("policy-update".into(), 0, crate::task::TaskType::User);
        task.set_id(60031);
        let base = crate::environment::USER_STACK_END - crate::environment::PAGE_SIZE;
        task.allocate_stack_pages(base, 1).unwrap();
        let policy = Policy::new(vec![(1, "manager".into())], vec![]).unwrap();
        let admin = insert(
            &task,
            PolicyHandle {
                policy: policy.clone(),
                administer: true,
            },
        )
        .unwrap();
        let readonly = insert(
            &task,
            PolicyHandle {
                policy: policy.clone(),
                administer: false,
            },
        )
        .unwrap();
        assert!(matches!(
            task.handle_table
                .get_metadata(admin as u32)
                .unwrap()
                .special_semantics,
            Some(SpecialSemantics::CloseOnExec)
        ));
        assert!(extend(&task, readonly, usize::MAX, 128).is_err());
        let mut spec = [0u8; 128];
        spec[..4].copy_from_slice(&1u32.to_ne_bytes());
        spec[8..16].copy_from_slice(&1u64.to_ne_bytes());
        spec[16..24].copy_from_slice(&((base + 256) as u64).to_ne_bytes());
        spec[24..32].copy_from_slice(&1u64.to_ne_bytes());
        let mut label = [0u8; 16];
        label[..4].copy_from_slice(&2u32.to_ne_bytes());
        label[4..8].copy_from_slice(&3u32.to_ne_bytes());
        label[8..].copy_from_slice(&((base + 512) as u64).to_ne_bytes());
        copy_to_user(&task, base + 256, &label).unwrap();
        copy_to_user(&task, base + 512, b"app").unwrap();
        spec[112] = 1;
        copy_to_user(&task, base, &spec).unwrap();
        assert!(extend(&task, admin, base, 128).is_err());
        assert!(policy.label(2).is_none());
        assert_eq!(policy.generation(), 1);
        spec[112] = 0;
        spec[104..112].copy_from_slice(&1u64.to_ne_bytes());
        spec[96..104].copy_from_slice(&u64::MAX.to_ne_bytes());
        copy_to_user(&task, base, &spec).unwrap();
        assert!(extend(&task, admin, base, 128).is_err());
        assert!(policy.label(2).is_none());
        spec[96..112].fill(0);
        copy_to_user(&task, base, &spec).unwrap();
        extend(&task, admin, base, 128).unwrap();
        assert_eq!(policy.generation(), 2);
        assert_eq!(policy.label(2).as_deref(), Some("app"));
        assert!(extend(&task, admin, base, 128).is_err());
        assert_eq!(policy.generation(), 2);
        assert_eq!(
            policy_handle(&task, readonly, false).unwrap().generation(),
            2
        );
    }
}
