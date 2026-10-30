//! Linux UID/GID and capability translation over Shield identities. No user
//! namespaces, file capabilities, ambient or inheritable grants are advertised.
use super::{LinuxAbi, errno};
use crate::library::std::usercopy::{copy_from_user, copy_to_user};
use crate::{
    arch::Trapframe,
    shield::identity::*,
    task::{Task, mytask},
};
use alloc::vec;

const KEEP: u32 = u32::MAX;
#[derive(Clone, Copy)]
pub(crate) enum SetIds {
    One,
    Pair,
    Triple,
}

fn next_ids(
    old: Identities<u32>,
    ids: [u32; 3],
    kind: SetIds,
    privileged: bool,
) -> Result<Identities<u32>, usize> {
    let mut next = old;
    match kind {
        SetIds::One => {
            if ids[0] == KEEP {
                return Err(errno::EINVAL);
            }
            if privileged {
                next = Identities::all(ids[0]);
            } else {
                if ids[0] != old.base && ids[0] != old.retained {
                    return Err(errno::EPERM);
                }
                next.active = ids[0];
            }
        }
        SetIds::Pair => {
            if ids[0] != KEEP {
                if !privileged && ids[0] != old.base && ids[0] != old.active {
                    return Err(errno::EPERM);
                }
                next.base = ids[0];
            }
            if ids[1] != KEEP {
                if !privileged && !old.contains(ids[1]) {
                    return Err(errno::EPERM);
                }
                next.active = ids[1];
            }
            if ids[0] != KEEP || (ids[1] != KEEP && ids[1] != old.base) {
                next.retained = next.active;
            }
        }
        SetIds::Triple => {
            for id in ids {
                if id != KEEP && !privileged && !old.contains(id) {
                    return Err(errno::EPERM);
                }
            }
            if ids[0] != KEEP {
                next.base = ids[0];
            }
            if ids[1] != KEEP {
                next.active = ids[1];
            }
            if ids[2] != KEEP {
                next.retained = ids[2];
            }
        }
    }
    Ok(next)
}

pub(crate) fn change_ids(
    creds: &mut Credentials,
    ids: [u32; 3],
    kind: SetIds,
    group: bool,
    keep_caps: bool,
) -> Result<(), usize> {
    // Prepare the whole change before publication, including capability fixups.
    let mut next = creds.clone();
    if group {
        let old = creds.groups();
        let ids = next_ids(
            Identities {
                base: old.base.0,
                active: old.active.0,
                retained: old.retained.0,
            },
            ids,
            kind,
            creds.permits(CHANGE_GROUPS),
        )?;
        next.set_groups(Identities {
            base: Group(ids.base),
            active: Group(ids.active),
            retained: Group(ids.retained),
        })
        .map_err(|_| errno::EPERM)?;
    } else {
        let old = creds.principals();
        let ids = next_ids(
            Identities {
                base: old.base.0,
                active: old.active.0,
                retained: old.retained.0,
            },
            ids,
            kind,
            creds.permits(CHANGE_PRINCIPAL),
        )?;
        next.set_principals(Identities {
            base: Principal(ids.base),
            active: Principal(ids.active),
            retained: Principal(ids.retained),
        })
        .map_err(|_| errno::EPERM)?;
        let mut permitted = creds.permitted_authority();
        let mut active = creds.active_authority();
        if old.contains(Principal(0)) && !ids.contains(0) && !keep_caps {
            permitted = 0;
            active = 0;
        }
        if old.active.0 == 0 && ids.active != 0 {
            active = 0;
        }
        if old.active.0 != 0 && ids.active == 0 {
            active = permitted;
        }
        next.set_authority(permitted, active)
            .map_err(|_| errno::EPERM)?;
    }
    *creds = next;
    Ok(())
}

fn set_ids(abi: &mut LinuxAbi, frame: &mut Trapframe, group: bool, kind: SetIds) -> usize {
    let task = mytask().unwrap();
    let ids = [
        frame.get_arg(0) as u32,
        frame.get_arg(1) as u32,
        frame.get_arg(2) as u32,
    ];
    frame.increment_pc_next(&task);
    change_ids(
        &mut task.credentials.write(),
        ids,
        kind,
        group,
        abi.keep_caps,
    )
    .map_or_else(errno::to_result, |_| 0)
}
macro_rules! setter {
    ($name:ident, $group:expr, $kind:ident) => {
        pub fn $name(abi: &mut LinuxAbi, frame: &mut Trapframe) -> usize {
            set_ids(abi, frame, $group, SetIds::$kind)
        }
    };
}
setter!(sys_setuid, false, One);
setter!(sys_setgid, true, One);
setter!(sys_setreuid, false, Pair);
setter!(sys_setregid, true, Pair);
setter!(sys_setresuid, false, Triple);
setter!(sys_setresgid, true, Triple);

fn ids(task: &Task, group: bool) -> [u32; 3] {
    let c = task.credentials.read();
    if group {
        let g = c.groups();
        [g.base.0, g.active.0, g.retained.0]
    } else {
        let p = c.principals();
        [p.base.0, p.active.0, p.retained.0]
    }
}
macro_rules! getter {
    ($name:ident, $group:expr, $slot:expr) => {
        pub fn $name(_abi: &mut LinuxAbi, frame: &mut Trapframe) -> usize {
            let task = mytask().unwrap();
            frame.increment_pc_next(&task);
            ids(&task, $group)[$slot] as usize
        }
    };
}
getter!(sys_getuid, false, 0);
getter!(sys_geteuid, false, 1);
getter!(sys_getgid, true, 0);
getter!(sys_getegid, true, 1);
fn get_res(frame: &mut Trapframe, group: bool) -> usize {
    let task = mytask().unwrap();
    let pointers = [frame.get_arg(0), frame.get_arg(1), frame.get_arg(2)];
    frame.increment_pc_next(&task);
    for (ptr, value) in pointers.into_iter().zip(ids(&task, group)) {
        if copy_to_user(&task, ptr, &value.to_ne_bytes()).is_err() {
            return errno::to_result(errno::EFAULT);
        }
    }
    0
}
pub fn sys_getresuid(_abi: &mut LinuxAbi, frame: &mut Trapframe) -> usize {
    get_res(frame, false)
}
pub fn sys_getresgid(_abi: &mut LinuxAbi, frame: &mut Trapframe) -> usize {
    get_res(frame, true)
}

pub fn sys_getgroups(_abi: &mut LinuxAbi, frame: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let size = frame.get_arg(0) as i32;
    let ptr = frame.get_arg(1);
    frame.increment_pc_next(&task);
    if size < 0 {
        return errno::to_result(errno::EINVAL);
    }
    let groups = task.credentials.read().supplementary().to_vec();
    if size == 0 {
        return groups.len();
    }
    if (size as usize) < groups.len() {
        return errno::to_result(errno::EINVAL);
    }
    let bytes: alloc::vec::Vec<u8> = groups.iter().flat_map(|g| g.0.to_ne_bytes()).collect();
    if copy_to_user(&task, ptr, &bytes).is_err() {
        return errno::to_result(errno::EFAULT);
    }
    groups.len()
}
pub fn sys_setgroups(_abi: &mut LinuxAbi, frame: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let size = frame.get_arg(0) as u32 as usize;
    let ptr = frame.get_arg(1);
    frame.increment_pc_next(&task);
    if !task.credentials.read().permits(CHANGE_GROUPS) {
        return errno::to_result(errno::EPERM);
    }
    if size > MAX_GROUPS {
        return errno::to_result(errno::EINVAL);
    }
    let mut bytes = vec![0; size * 4];
    if copy_from_user(&task, ptr, &mut bytes).is_err() {
        return errno::to_result(errno::EFAULT);
    }
    let mut groups = alloc::vec::Vec::with_capacity(size);
    for raw in bytes.chunks_exact(4) {
        let id = u32::from_ne_bytes(raw.try_into().unwrap());
        if id == KEEP {
            return errno::to_result(errno::EINVAL);
        }
        groups.push(Group(id));
    }
    task.credentials
        .write()
        .set_supplementary(groups)
        .map_or_else(|_| errno::to_result(errno::EPERM), |_| 0)
}

const CAP_MAP: [(u32, u64); 7] = [
    (CHANGE_PRINCIPAL, 1 << 7),
    (CHANGE_GROUPS, 1 << 6),
    (MANAGE_MOUNTS, 1 << 21),
    (MANAGE_RESOURCES, 1 << 21),
    (MANAGE_OWNERS, 1 << 0),
    (MANAGE_DEVICES, 1 << 27),
    (MANAGE_LAYOUT, 1 << 21),
];
fn linux_caps(authority: u32) -> u64 {
    CAP_MAP
        .iter()
        .filter(|(native, _)| authority & native != 0)
        .fold(0, |bits, (_, linux)| bits | linux)
}
fn native_caps(bits: u64) -> Result<u32, usize> {
    let supported = linux_caps(ALL_AUTHORITY);
    if bits & !supported != 0 {
        return Err(errno::EOPNOTSUPP);
    }
    Ok(CAP_MAP
        .iter()
        .filter(|(_, linux)| bits & linux != 0)
        .fold(0, |bits, (native, _)| bits | native))
}
fn capabilities(frame: &mut Trapframe, write: bool) -> usize {
    let task = mytask().unwrap();
    let header_ptr = frame.get_arg(0);
    let data_ptr = frame.get_arg(1);
    frame.increment_pc_next(&task);
    let mut header = [0u8; 8];
    if copy_from_user(&task, header_ptr, &mut header).is_err() {
        return errno::to_result(errno::EFAULT);
    }
    let words = match u32::from_ne_bytes(header[..4].try_into().unwrap()) {
        0x19980330 => 1,
        0x20071026 | 0x20080522 => 2,
        _ => {
            if copy_to_user(&task, header_ptr, &0x20080522u32.to_ne_bytes()).is_err() {
                return errno::to_result(errno::EFAULT);
            }
            return errno::to_result(errno::EINVAL);
        }
    };
    let pid = i32::from_ne_bytes(header[4..].try_into().unwrap());
    let own = task
        .get_namespace()
        .resolve_local_id(task.get_thread_group_id());
    if pid < 0 {
        return errno::to_result(errno::EINVAL);
    }
    if pid != 0 && Some(pid as usize) != own && Some(pid as usize) != task.try_get_namespace_id() {
        return errno::to_result(if write {
            errno::EPERM
        } else {
            errno::EOPNOTSUPP
        });
    }
    let mut data = [0u8; 24];
    if write {
        if copy_from_user(&task, data_ptr, &mut data[..words * 12]).is_err() {
            return errno::to_result(errno::EFAULT);
        }
        let mut sets = [0u64; 3];
        for word in 0..words {
            for (field, value) in sets.iter_mut().enumerate() {
                let offset = word * 12 + field * 4;
                *value |= (u32::from_ne_bytes(data[offset..offset + 4].try_into().unwrap()) as u64)
                    << (word * 32);
            }
        }
        if sets[2] != 0 {
            return errno::to_result(errno::EOPNOTSUPP);
        }
        let effective = match native_caps(sets[0]) {
            Ok(v) => v,
            Err(e) => return errno::to_result(e),
        };
        let permitted = match native_caps(sets[1]) {
            Ok(v) => v,
            Err(e) => return errno::to_result(e),
        };
        task.credentials
            .write()
            .set_authority(permitted, effective)
            .map_or_else(|_| errno::to_result(errno::EPERM), |_| 0)
    } else {
        let (effective, permitted) = {
            let c = task.credentials.read();
            (
                linux_caps(c.active_authority()),
                linux_caps(c.permitted_authority()),
            )
        };
        for word in 0..words {
            for (field, value) in [effective, permitted, 0].into_iter().enumerate() {
                let offset = word * 12 + field * 4;
                data[offset..offset + 4]
                    .copy_from_slice(&((value >> (word * 32)) as u32).to_ne_bytes());
            }
        }
        copy_to_user(&task, data_ptr, &data[..words * 12])
            .map_or_else(|_| errno::to_result(errno::EFAULT), |_| 0)
    }
}
pub fn sys_capget(_abi: &mut LinuxAbi, frame: &mut Trapframe) -> usize {
    capabilities(frame, false)
}
pub fn sys_capset(_abi: &mut LinuxAbi, frame: &mut Trapframe) -> usize {
    capabilities(frame, true)
}

pub(crate) fn prepare_exec(task: &Task) {
    let mut c = task.credentials.write();
    let mut p = c.principals();
    p.retained = p.active;
    let mut g = c.groups();
    g.retained = g.active;
    c.set_principals(p).unwrap();
    c.set_groups(g).unwrap();
    // File/ambient capabilities are unsupported. Non-root exec cannot retain
    // authority kept temporarily by PR_SET_KEEPCAPS before exec.
    if p.active.0 != 0 {
        c.set_authority(0, 0).unwrap();
    }
}
pub(crate) fn update_auxv(task: &Task, auxv: &mut [crate::task::elf_loader::AuxVec]) {
    use crate::task::elf_loader::*;
    let p = ids(task, false);
    let g = ids(task, true);
    for entry in auxv {
        match entry.a_type {
            AT_UID => entry.a_val = p[0] as u64,
            AT_EUID => entry.a_val = p[1] as u64,
            AT_GID => entry.a_val = g[0] as u64,
            AT_EGID => entry.a_val = g[1] as u64,
            AT_SECURE => entry.a_val = u64::from(p[0] != p[1] || g[0] != g[1]),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test_case]
    fn linux_identity_drop_saved_restore_and_permanent_capability_loss() {
        let mut c = Credentials::bootstrap();
        change_ids(&mut c, [KEEP, 10001, KEEP], SetIds::Triple, false, false).unwrap();
        assert_eq!(c.principals().base, Principal(0));
        assert_eq!(c.active_authority(), 0);
        assert_eq!(c.permitted_authority(), ALL_AUTHORITY);
        change_ids(&mut c, [KEEP, 0, KEEP], SetIds::Triple, false, false).unwrap();
        assert_eq!(c.active_authority(), ALL_AUTHORITY);
        change_ids(&mut c, [10001, 0, 0], SetIds::One, false, false).unwrap();
        assert_eq!(c.principals(), Identities::all(Principal(10001)));
        assert_eq!(c.permitted_authority(), 0);
        let old = c.clone();
        assert_eq!(
            change_ids(&mut c, [0, 0, 0], SetIds::One, false, false),
            Err(errno::EPERM)
        );
        assert_eq!(
            change_ids(&mut c, [0, 0, 0], SetIds::Triple, true, false),
            Ok(())
        );
        assert_eq!(
            change_ids(&mut c, [7, 7, 7], SetIds::Triple, true, false),
            Err(errno::EPERM)
        );
        assert_eq!(c, old);
    }
    #[test_case]
    fn linux_keepcaps_retains_ceiling_but_cannot_regain_removed_grants() {
        let mut c = Credentials::bootstrap();
        change_ids(&mut c, [123, 123, 123], SetIds::Triple, false, true).unwrap();
        assert_eq!(c.active_authority(), 0);
        assert_eq!(c.permitted_authority(), ALL_AUTHORITY);
        c.set_authority(CHANGE_GROUPS, CHANGE_GROUPS).unwrap();
        change_ids(&mut c, [77, 0, 0], SetIds::One, true, false).unwrap();
        assert_eq!(c.groups(), Identities::all(Group(77)));
        assert_eq!(
            change_ids(&mut c, [0, 0, 0], SetIds::Triple, false, true),
            Err(errno::EPERM)
        );
        assert!(c.set_authority(ALL_AUTHORITY, ALL_AUTHORITY).is_err());
        assert_eq!(native_caps(1u64 << 63), Err(errno::EOPNOTSUPP));
    }
    #[test_case]
    fn linux_pair_saved_id_rules_and_failed_updates_are_atomic() {
        let mut c = Credentials::bootstrap();
        change_ids(&mut c, [5, 6, 7], SetIds::Triple, true, false).unwrap();
        c.set_authority(0, 0).unwrap();
        let old = c.clone();
        assert_eq!(
            change_ids(&mut c, [7, KEEP, KEEP], SetIds::Pair, true, false),
            Err(errno::EPERM)
        );
        assert_eq!(c, old);
        change_ids(&mut c, [KEEP, 7, KEEP], SetIds::Pair, true, false).unwrap();
        assert_eq!(
            c.groups(),
            Identities {
                base: Group(5),
                active: Group(7),
                retained: Group(7)
            }
        );
        assert_eq!(
            change_ids(&mut c, [KEEP, KEEP, KEEP], SetIds::One, true, false),
            Err(errno::EINVAL)
        );
    }
}
