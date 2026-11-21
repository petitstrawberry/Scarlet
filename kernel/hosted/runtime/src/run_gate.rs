//! Admission and stop acknowledgement for one address-space worker generation.

use std::{
    collections::BTreeMap,
    sync::{Condvar, Mutex},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkerId {
    pub address_space: usize,
    pub image_generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskId {
    pub id: usize,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunPermit {
    pub worker: WorkerId,
    pub task: TaskId,
    pub vcpu: usize,
    pub mapping_generation: u64,
    pub epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateError {
    Closed,
    Terminated,
    AlreadyRunning,
    GenerationExhausted,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Admission {
    Open,
    Closed,
    Terminated,
}

struct State {
    admission: Admission,
    mapping_generation: u64,
    next_epoch: u64,
    active: BTreeMap<usize, RunPermit>,
}

pub struct RunGate {
    worker: WorkerId,
    state: Mutex<State>,
    stopped: Condvar,
}

impl RunGate {
    pub fn new(worker: WorkerId) -> Self {
        Self {
            worker,
            state: Mutex::new(State {
                admission: Admission::Open,
                mapping_generation: 0,
                next_epoch: 1,
                active: BTreeMap::new(),
            }),
            stopped: Condvar::new(),
        }
    }

    /// Record the permit before sending the worker its RUN command.
    pub fn issue(&self, task: TaskId, vcpu: usize) -> Result<RunPermit, GateError> {
        let mut state = self.state.lock().unwrap();
        match state.admission {
            Admission::Open => {}
            Admission::Closed => return Err(GateError::Closed),
            Admission::Terminated => return Err(GateError::Terminated),
        }
        if state.active.contains_key(&task.id)
            || state.active.values().any(|permit| permit.vcpu == vcpu)
        {
            return Err(GateError::AlreadyRunning);
        }
        let epoch = state.next_epoch;
        state.next_epoch = epoch.checked_add(1).ok_or(GateError::GenerationExhausted)?;
        let permit = RunPermit {
            worker: self.worker,
            task,
            vcpu,
            mapping_generation: state.mapping_generation,
            epoch,
        };
        state.active.insert(task.id, permit);
        Ok(permit)
    }

    /// Consume a matching stop once after the worker guarantees that this
    /// permit cannot execute, including delayed RUN delivery.
    pub fn acknowledge_stop(&self, permit: RunPermit) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.active.get(&permit.task.id) != Some(&permit) {
            return false;
        }
        state.active.remove(&permit.task.id);
        if state.active.is_empty() {
            self.stopped.notify_all();
        }
        true
    }

    pub fn try_parked(&self) -> bool {
        self.state
            .try_lock()
            .is_ok_and(|state| state.active.is_empty())
    }

    /// Close guest entry and collect every issued permit in one operation.
    pub fn close_for_mapping(&self) -> Result<MappingBarrier<'_>, GateError> {
        let mut state = self.state.lock().unwrap();
        match state.admission {
            Admission::Open => state.admission = Admission::Closed,
            Admission::Closed => return Err(GateError::Closed),
            Admission::Terminated => return Err(GateError::Terminated),
        }
        Ok(MappingBarrier {
            gate: self,
            permits: state.active.values().copied().collect(),
        })
    }

    /// Retire a worker after its process termination is confirmed. The caller
    /// delivers terminal events to each returned permit's owning vCPU.
    pub fn confirm_worker_death(&self) -> Vec<RunPermit> {
        let mut state = self.state.lock().unwrap();
        state.admission = Admission::Terminated;
        let permits = std::mem::take(&mut state.active);
        self.stopped.notify_all();
        permits.into_values().collect()
    }
}

#[must_use = "the address space stays closed until the mapping operation finishes"]
pub struct MappingBarrier<'a> {
    gate: &'a RunGate,
    permits: Vec<RunPermit>,
}

impl<'a> MappingBarrier<'a> {
    pub fn issued_permits(&self) -> &[RunPermit] {
        &self.permits
    }

    /// Wait for active guest execution to stop. Kernel continuations that
    /// already handled their stop are outside this wait.
    pub fn wait(self) -> Result<ParkedWorker<'a>, GateError> {
        let state = self.gate.state.lock().unwrap();
        let state = self
            .gate
            .stopped
            .wait_while(state, |state| !state.active.is_empty())
            .unwrap();
        if state.admission == Admission::Terminated {
            return Err(GateError::Terminated);
        }
        Ok(ParkedWorker { gate: self.gate })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingOutcome {
    Committed,
    Restored,
}

#[must_use = "the address space stays closed until its mapping is acknowledged"]
pub struct ParkedWorker<'a> {
    gate: &'a RunGate,
}

impl ParkedWorker<'_> {
    /// Reopen entry after a mapping ACK. A restored mapping preserves the
    /// previous generation. A committed mapping advances the generation.
    pub fn finish_after_mapping_ack(self, outcome: MappingOutcome) -> Result<u64, GateError> {
        let mut state = self.gate.state.lock().unwrap();
        if state.admission == Admission::Terminated {
            return Err(GateError::Terminated);
        }
        if outcome == MappingOutcome::Committed {
            state.mapping_generation = state
                .mapping_generation
                .checked_add(1)
                .ok_or(GateError::GenerationExhausted)?;
        }
        state.admission = Admission::Open;
        Ok(state.mapping_generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: usize) -> TaskId {
        TaskId { id, generation: 1 }
    }

    #[test]
    fn mapping_gate_covers_all_issued_permits_and_rejects_stale_acknowledgements() {
        let gate = RunGate::new(WorkerId {
            address_space: 1,
            image_generation: 2,
        });
        let first = gate.issue(task(1), 0).unwrap();
        let second = gate.issue(task(2), 1).unwrap();
        let barrier = gate.close_for_mapping().unwrap();
        assert_eq!(barrier.issued_permits(), &[first, second]);
        assert_eq!(gate.issue(task(3), 2), Err(GateError::Closed));
        std::thread::scope(|scope| {
            let stopped = scope.spawn(|| barrier.wait().unwrap());
            assert!(gate.acknowledge_stop(first));
            assert!(!gate.acknowledge_stop(first));
            assert!(!stopped.is_finished());
            assert!(gate.acknowledge_stop(second));
            let parked = stopped.join().unwrap();
            assert_eq!(gate.issue(task(3), 2), Err(GateError::Closed));
            assert_eq!(
                parked.finish_after_mapping_ack(MappingOutcome::Committed),
                Ok(1)
            );
        });
        let resumed = gate.issue(task(1), 0).unwrap();
        assert_eq!(resumed.mapping_generation, 1);
        assert!(!gate.acknowledge_stop(first));
        assert!(gate.acknowledge_stop(resumed));
        let parked = gate.close_for_mapping().unwrap().wait().unwrap();
        assert_eq!(
            parked.finish_after_mapping_ack(MappingOutcome::Restored),
            Ok(1)
        );
    }

    #[test]
    fn confirmed_worker_death_unblocks_mapping_and_retires_all_permits() {
        let gate = RunGate::new(WorkerId {
            address_space: 1,
            image_generation: 2,
        });
        let first = gate.issue(task(1), 2).unwrap();
        let second = gate.issue(task(2), 3).unwrap();
        let barrier = gate.close_for_mapping().unwrap();
        std::thread::scope(|scope| {
            let waiting = scope.spawn(|| barrier.wait().err());
            assert_eq!(gate.confirm_worker_death(), [first, second]);
            assert_eq!(waiting.join().unwrap(), Some(GateError::Terminated));
        });
        assert_eq!(gate.issue(task(1), 0), Err(GateError::Terminated));
        assert!(!gate.acknowledge_stop(first));
    }
}
