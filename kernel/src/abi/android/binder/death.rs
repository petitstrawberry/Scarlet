//! Death work is attached to a handle until CLEAR. Detached work survives until
//! the matching delivery/acknowledgement completes, even if the handle is reused
//! for a new subscription. Cookies are opaque userspace values.
use super::*;
use crate::ipc::rpc::{RpcDeathWatch, RpcObject};

pub(super) const BC_REQUEST_DEATH_NOTIFICATION: u32 = 0x400c630e;
pub(super) const BC_CLEAR_DEATH_NOTIFICATION: u32 = 0x400c630f;
pub(super) const BC_DEAD_BINDER_DONE: u32 = 0x40086310;
const BR_DEAD_BINDER: u32 = 0x8008720f;
const BR_CLEAR_DEATH_NOTIFICATION_DONE: u32 = 0x80087210;
const MAX_DEATHS: usize = 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Watching,
    DeadQueued,
    DeadDelivered,
    DeadAcknowledged,
    ClearQueued,
    Finished,
}
struct State {
    handle: Option<u32>,
    phase: Phase,
    clearing: bool,
    target_thread: Option<usize>,
    delivering: bool,
}
struct Death {
    watch: RpcDeathWatch,
    cookie: usize,
    waker: Weak<Waker>,
    state: IrqSpinLock<State>,
}
impl Death {
    fn refresh(&self, state: &mut State) {
        if state.phase == Phase::Watching && self.watch.ready() {
            state.phase = Phase::DeadQueued;
        }
    }
    fn next(&self, state: &mut State, tid: usize, process_work: bool) -> Option<u32> {
        self.refresh(state);
        if state.delivering {
            return None;
        }
        match state.phase {
            Phase::DeadQueued if process_work => Some(BR_DEAD_BINDER),
            Phase::ClearQueued
                if state
                    .target_thread
                    .map_or(process_work, |target| target == tid) =>
            {
                Some(BR_CLEAR_DEATH_NOTIFICATION_DONE)
            }
            _ => None,
        }
    }
    fn notify(&self) {
        if let Some(waker) = self.waker.upgrade() {
            waker.wake_all();
        }
    }
}

#[derive(Default)]
pub(super) struct Deaths {
    entries: Vec<Arc<Death>>,
}
impl Deaths {
    pub fn request(
        &mut self,
        handle: u32,
        cookie: usize,
        object: &Arc<RpcObject>,
        waker: &Arc<Waker>,
    ) -> Result<(), usize> {
        self.entries.retain(|entry| {
            let state = entry.state.lock();
            state.phase != Phase::Finished || state.delivering
        });
        if self
            .entries
            .iter()
            .any(|entry| entry.state.lock().handle == Some(handle))
        {
            return Err(errno::EINVAL);
        }
        if self.entries.len() >= MAX_DEATHS {
            return Err(errno::ENOSPC);
        }
        self.entries.push(Arc::new(Death {
            watch: object.watch_death(waker),
            cookie,
            waker: Arc::downgrade(waker),
            state: IrqSpinLock::new(State {
                handle: Some(handle),
                phase: Phase::Watching,
                clearing: false,
                target_thread: None,
                delivering: false,
            }),
        }));
        Ok(())
    }
    pub fn clear(
        &mut self,
        handle: u32,
        cookie: usize,
        target_thread: Option<usize>,
    ) -> Result<(), usize> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.state.lock().handle == Some(handle))
            .ok_or(errno::EINVAL)?;
        if entry.cookie != cookie {
            return Err(errno::EINVAL);
        }
        let mut state = entry.state.lock();
        entry.refresh(&mut state);
        state.handle = None;
        state.clearing = true;
        if matches!(state.phase, Phase::Watching | Phase::DeadAcknowledged) {
            state.phase = Phase::ClearQueued;
            state.target_thread = target_thread;
        }
        // A queued or delivered death must be acknowledged before CLEAR_DONE.
        drop(state);
        entry.notify();
        Ok(())
    }
    pub fn done(&mut self, cookie: usize, target_thread: Option<usize>) -> Result<(), usize> {
        let entry = self
            .entries
            .iter()
            .find(|entry| {
                entry.cookie == cookie && entry.state.lock().phase == Phase::DeadDelivered
            })
            .ok_or(errno::EINVAL)?;
        let mut state = entry.state.lock();
        state.phase = if state.clearing {
            Phase::ClearQueued
        } else {
            Phase::DeadAcknowledged
        };
        state.target_thread = target_thread;
        drop(state);
        entry.notify();
        Ok(())
    }
    pub fn forget(&mut self, handle: u32) {
        self.entries.retain(|entry| {
            let mut state = entry.state.lock();
            if state.handle == Some(handle) {
                state.handle = None;
                state.phase = Phase::Finished;
                false
            } else {
                true
            }
        });
    }
    pub fn thread_exit(&mut self, tid: usize) {
        for entry in &self.entries {
            let mut state = entry.state.lock();
            if state.target_thread == Some(tid) {
                state.target_thread = None;
            }
        }
    }
    pub fn ready(&self, tid: usize, process_work: bool) -> bool {
        self.entries.iter().any(|entry| {
            entry
                .next(&mut entry.state.lock(), tid, process_work)
                .is_some()
        })
    }
    pub fn record(&self, tid: usize, process_work: bool) -> Option<DeathRecord> {
        for entry in &self.entries {
            let mut state = entry.state.lock();
            if let Some(command) = entry.next(&mut state, tid, process_work) {
                state.delivering = true;
                state.phase = if command == BR_DEAD_BINDER {
                    Phase::DeadDelivered
                } else {
                    Phase::Finished
                };
                return Some(DeathRecord {
                    entry: entry.clone(),
                    command,
                    committed: false,
                });
            }
        }
        None
    }
}

pub(super) struct DeathRecord {
    entry: Arc<Death>,
    command: u32,
    committed: bool,
}
impl DeathRecord {
    pub fn write(mut self, task: &Task, output: usize) -> Result<usize, usize> {
        let mut bytes = [0u8; 12];
        bytes[..4].copy_from_slice(&self.command.to_le_bytes());
        put64(&mut bytes, 4, self.entry.cookie);
        write(task, output, &bytes)?;
        self.committed = true;
        Ok(bytes.len())
    }
}
impl Drop for DeathRecord {
    fn drop(&mut self) {
        let mut state = self.entry.state.lock();
        if !self.committed {
            if self.command == BR_DEAD_BINDER && state.phase == Phase::DeadDelivered {
                state.phase = Phase::DeadQueued;
            } else if self.command == BR_CLEAR_DEATH_NOTIFICATION_DONE {
                state.phase = Phase::ClearQueued;
            }
        }
        state.delivering = false;
        drop(state);
        self.entry.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn delivered(deaths: &Deaths, tid: usize, process_work: bool) -> u32 {
        let mut record = deaths.record(tid, process_work).unwrap();
        let command = record.command;
        record.committed = true;
        drop(record);
        command
    }
    #[test_case]
    fn binder_death_clear_before_close_cancels_and_targets_its_looper() {
        let endpoint = RpcEndpoint::new();
        let object = endpoint.create_object().unwrap();
        let waker = endpoint.waker();
        let mut deaths = Deaths::default();
        deaths.request(1, 0x1234, &object, &waker).unwrap();
        assert_eq!(deaths.request(1, 77, &object, &waker), Err(errno::EINVAL));
        assert_eq!(deaths.clear(1, 77, None), Err(errno::EINVAL));
        assert!(!deaths.ready(2, true));
        deaths.clear(1, 0x1234, Some(1)).unwrap();
        endpoint.close();
        assert!(!deaths.ready(2, true));
        assert_eq!(
            delivered(&deaths, 1, false),
            BR_CLEAR_DEATH_NOTIFICATION_DONE
        );
        assert!(!deaths.ready(1, true));
        assert_eq!(deaths.done(0x1234, None), Err(errno::EINVAL));
    }
    #[test_case]
    fn binder_death_clear_waits_for_ack_before_or_during_or_after_delivery() {
        for order in 0..4 {
            let endpoint = RpcEndpoint::new();
            let object = endpoint.create_object().unwrap();
            let mut deaths = Deaths::default();
            deaths.request(1, 88, &object, &endpoint.waker()).unwrap();
            object.revoke();
            assert!(!deaths.ready(1, false));
            if order == 0 {
                deaths.clear(1, 88, Some(1)).unwrap();
            }
            let mut record = deaths.record(1, true).unwrap();
            assert_eq!(record.command, BR_DEAD_BINDER);
            assert!(deaths.record(2, true).is_none());
            if order == 1 {
                deaths.clear(1, 88, Some(1)).unwrap();
            }
            record.committed = true;
            drop(record);
            if order == 2 {
                deaths.clear(1, 88, Some(1)).unwrap();
            }
            assert!(!deaths.ready(1, true));
            deaths.done(88, Some(1)).unwrap();
            if order == 3 {
                deaths.clear(1, 88, Some(1)).unwrap();
            }
            assert_eq!(deaths.done(88, None), Err(errno::EINVAL));
            assert_eq!(
                delivered(&deaths, 1, true),
                BR_CLEAR_DEATH_NOTIFICATION_DONE
            );
            assert!(!deaths.ready(1, true));
        }
    }
    #[test_case]
    fn binder_death_failed_copy_retries_and_preserves_concurrent_clear() {
        let endpoint = RpcEndpoint::new();
        let object = endpoint.create_object().unwrap();
        let mut deaths = Deaths::default();
        object.revoke();
        deaths.request(1, 88, &object, &endpoint.waker()).unwrap();
        let record = deaths.record(1, true).unwrap();
        deaths.clear(1, 88, Some(1)).unwrap();
        drop(record); // failed copy; CLEAR must not skip the undelivered death
        assert_eq!(delivered(&deaths, 1, true), BR_DEAD_BINDER);
        deaths.done(88, Some(1)).unwrap();
        drop(deaths.record(1, true).unwrap());
        assert_eq!(
            delivered(&deaths, 1, true),
            BR_CLEAR_DEATH_NOTIFICATION_DONE
        );
        assert!(!deaths.ready(1, true));
    }
    #[test_case]
    fn binder_death_reregistration_does_not_consume_old_clear_or_death() {
        let endpoint = RpcEndpoint::new();
        let object = endpoint.create_object().unwrap();
        let mut deaths = Deaths::default();
        deaths.request(1, 88, &object, &endpoint.waker()).unwrap();
        deaths.clear(1, 88, Some(1)).unwrap();
        deaths.request(1, 99, &object, &endpoint.waker()).unwrap();
        endpoint.close();
        assert_eq!(
            delivered(&deaths, 1, true),
            BR_CLEAR_DEATH_NOTIFICATION_DONE
        );
        let mut record = deaths.record(1, true).unwrap();
        assert_eq!(record.entry.cookie, 99);
        assert_eq!(record.command, BR_DEAD_BINDER);
        record.committed = true;
        drop(record);
        deaths.done(99, None).unwrap();
        assert!(!deaths.ready(1, true));
        deaths.clear(1, 99, Some(2)).unwrap();
        deaths.thread_exit(2);
        assert_eq!(
            delivered(&deaths, 1, true),
            BR_CLEAR_DEATH_NOTIFICATION_DONE
        );
    }
    #[test_case]
    fn binder_death_work_is_bounded_and_handle_release_cancels_attached_work() {
        let endpoint = RpcEndpoint::new();
        let object = endpoint.create_object().unwrap();
        let mut deaths = Deaths::default();
        for cookie in 0..MAX_DEATHS {
            deaths
                .request(1, cookie, &object, &endpoint.waker())
                .unwrap();
            deaths.clear(1, cookie, None).unwrap();
        }
        assert_eq!(
            deaths.request(1, 88, &object, &endpoint.waker()),
            Err(errno::ENOSPC)
        );
        for _ in 0..MAX_DEATHS {
            assert_eq!(
                delivered(&deaths, 1, true),
                BR_CLEAR_DEATH_NOTIFICATION_DONE
            );
        }
        deaths.request(1, 88, &object, &endpoint.waker()).unwrap();
        deaths.forget(1);
        endpoint.close();
        assert!(!deaths.ready(1, true));
        assert!(deaths.entries.is_empty());
    }
}
