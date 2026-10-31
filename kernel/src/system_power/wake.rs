//! Wake deadlines owned by timers, independent of any ABI or suspend driver.
//!
//! The outer power manager queries a coherent revision and must keep the
//! machine awake for unread wake events. Pending deadlines require a hardware
//! wake source before suspend; this registry itself does not provide one.
use alloc::collections::BTreeMap;
use crate::sync::IrqSpinLock;

const MAX_RESERVATIONS: usize = 4096;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Entry { deadline: Option<u64>, pending: bool }

struct Registry { entries: BTreeMap<u64, Entry>, next: u64, revision: u64 }
impl Registry {
    const fn new() -> Self { Self { entries: BTreeMap::new(), next: 1, revision: 0 } }
    fn reserve(&mut self) -> Result<u64, &'static str> {
        if self.entries.len() >= MAX_RESERVATIONS { return Err("wake reservation limit"); }
        let id = self.next;
        self.next = id.checked_add(1).ok_or("wake reservation identity exhausted")?;
        self.entries.insert(id, Entry::default());
        Ok(id)
    }
    fn update(&mut self, id: u64, entry: Entry) {
        if let Some(old) = self.entries.get_mut(&id) {
            if *old != entry { *old = entry; self.revision = self.revision.wrapping_add(1); }
        }
    }
    fn remove(&mut self, id: u64) {
        if self.entries.remove(&id).is_some_and(|entry| entry != Entry::default()) {
            self.revision = self.revision.wrapping_add(1);
        }
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot { revision: self.revision,
            deadline_ns: self.entries.values().filter_map(|e| e.deadline).min(),
            pending_events: self.entries.values().filter(|e| e.pending).count() as u32 }
    }
}
static REGISTRY: IrqSpinLock<Registry> = IrqSpinLock::new(Registry::new());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub revision: u64,
    pub deadline_ns: Option<u64>,
    pub pending_events: u32,
}
pub fn snapshot() -> Snapshot { REGISTRY.lock().snapshot() }

/// Lifetime-owned reservation. Closing the last timer handle releases it.
pub struct Reservation(u64);
impl Reservation {
    pub fn new() -> Result<Self, &'static str> { REGISTRY.lock().reserve().map(Self) }
    pub fn update(&self, deadline: Option<u64>, pending: bool) {
        REGISTRY.lock().update(self.0, Entry { deadline, pending });
    }
}
impl Drop for Reservation { fn drop(&mut self) { REGISTRY.lock().remove(self.0); } }

/// Native management query; no hardware wake support is advertised.
pub fn sys_query(frame: &mut crate::arch::Trapframe) -> usize {
    use crate::library::std::usercopy::copy_to_user;
    let Some(task) = crate::task::mytask() else { return usize::MAX; };
    frame.increment_pc_next(&task);
    if frame.get_arg(1) != 32 || !task.credentials.read().permits(crate::shield::identity::MANAGE_RESOURCES) {
        return usize::MAX;
    }
    let state = snapshot();
    let flags: u32 = 4 | u32::from(state.deadline_ns.is_some()) | (u32::from(state.pending_events != 0) << 1);
    let mut bytes = [0;32];
    bytes[..4].copy_from_slice(&1u32.to_ne_bytes());
    bytes[4..8].copy_from_slice(&flags.to_ne_bytes());
    bytes[8..16].copy_from_slice(&state.revision.to_ne_bytes());
    bytes[16..24].copy_from_slice(&state.deadline_ns.unwrap_or(0).to_ne_bytes());
    bytes[24..28].copy_from_slice(&state.pending_events.to_ne_bytes());
    copy_to_user(&task, frame.get_arg(0), &bytes).map_or(usize::MAX, |_| 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test_case]
    fn native_query_copies_a_fixed_record_and_checks_authority_even_for_identity_zero() {
        use crate::{environment::{PAGE_SIZE,USER_STACK_END}, task::{Task,TaskType,set_mock_current_task,clear_mock_current_task}};
        use crate::library::std::usercopy::{copy_from_user,copy_to_user};
        let task = alloc::sync::Arc::new(Task::new("wake-query".into(), 1, TaskType::User));
        let code = USER_STACK_END - PAGE_SIZE;
        task.allocate_stack_pages(code,1).unwrap();
        for index in 0..4 { copy_to_user(&task,code+index*4,&0x73u32.to_ne_bytes()).unwrap(); }
        set_mock_current_task(task.clone());
        let mut frame=crate::arch::Trapframe::new();frame.set_pc(code as u64);
        frame.set_arg(0,code+128);frame.set_arg(1,32);
        assert_eq!(sys_query(&mut frame),0);
        let mut bytes=[0;32];copy_from_user(&task,code+128,&mut bytes).unwrap();
        assert_eq!(u32::from_ne_bytes(bytes[..4].try_into().unwrap()),1);
        assert_eq!(u32::from_ne_bytes(bytes[4..8].try_into().unwrap()) & 4,4);
        assert_eq!(&bytes[28..],&[0;4]);
        task.credentials.write().set_authority(0,0).unwrap();
        assert_eq!(sys_query(&mut frame),usize::MAX);
        clear_mock_current_task();
    }
    #[test_case]
    fn deadlines_rearm_cancel_and_unread_events_change_the_management_snapshot() {
        let mut registry = Registry::new();
        let first = registry.reserve().unwrap(); let second = registry.reserve().unwrap();
        registry.update(first, Entry { deadline: Some(200), pending: false });
        registry.update(second, Entry { deadline: Some(100), pending: false });
        let old = registry.snapshot(); assert_eq!(old.deadline_ns, Some(100));
        registry.update(second, Entry { deadline: None, pending: true });
        let fired = registry.snapshot(); assert_ne!(fired.revision, old.revision);
        assert_eq!(fired.deadline_ns, Some(200)); assert_eq!(fired.pending_events, 1);
        registry.remove(first); assert_eq!(registry.snapshot().deadline_ns, None);
        registry.remove(second); assert_eq!(registry.snapshot().pending_events, 0);
    }
    #[test_case]
    fn reservations_are_bounded_and_released_slots_do_not_reuse_identities() {
        let mut registry = Registry::new();
        for _ in 0..MAX_RESERVATIONS { registry.reserve().unwrap(); }
        assert!(registry.reserve().is_err()); registry.remove(1);
        assert_eq!(registry.reserve().unwrap(), MAX_RESERVATIONS as u64 + 1);
    }
}
