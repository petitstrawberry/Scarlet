//! Lease a native execution thread only for an issued vCPU permit. Sleeping
//! Tasks retain their logical identity and exclusive retry PC.
use super::*;
use std::collections::BTreeSet;

type TaskKey = (usize, u64);
// The worker sees interrupts in physical lease order, even when two
// independent vCPUs deliver their kernel permits out of order.
#[derive(Clone, Copy, Debug)]
struct Lease {
    lane: u64,
    sequence: u64,
}

#[derive(Debug)]
struct Lane {
    id: u64,
    channel: ExecutionStream,
}

#[derive(Default, Debug)]
struct TaskRuns {
    last_epoch: u64,
    early_interrupt: Option<RunPermit>,
    active: Option<(RunPermit, Lease)>,
}

#[derive(Default, Debug)]
pub(super) struct Lanes {
    tasks: BTreeMap<TaskKey, TaskRuns>,
    idle: Vec<Lane>,
    busy_cpus: BTreeSet<usize>,
    last_sequence: u64,
}

#[derive(Debug)]
pub struct WorkerThread {
    task: TaskId,
    control: Arc<WorkerControl>,
    process: Arc<WorkerProcess>,
    running: Option<(RunPermit, Lane)>,
    retry_pc: Option<u64>,
}

impl Worker {
    pub fn open_thread(&mut self, task: TaskId) -> io::Result<WorkerThread> {
        let mut lanes = self.channel.lanes.lock().unwrap();
        let slot = lanes.tasks.entry((task.id, task.generation));
        let std::collections::btree_map::Entry::Vacant(slot) = slot else {
            return Err(invalid("Duplicate worker task"));
        };
        slot.insert(TaskRuns::default());
        Ok(WorkerThread {
            task,
            control: self.channel.clone(),
            process: self.process.clone(),
            running: None,
            retry_pc: None,
        })
    }
}

impl WorkerControl {
    fn attach_lane(&self, id: u64) -> io::Result<Lane> {
        let (channel, child) = UnixStream::pair()?;
        let channel = execution_stream(channel.into())?;
        let mut wire = self.wire.lock().unwrap();
        write_words(&mut *wire, [ATTACH, id])?;
        send_backing(&wire, child.as_fd())?;
        read_ack(&mut wire)?;
        Ok(Lane { id, channel })
    }

    fn interrupt_lane(&self, lease: Lease) -> io::Result<()> {
        let mut wire = self.wire.lock().unwrap();
        write_words(&mut *wire, [INTERRUPT, lease.lane, lease.sequence])?;
        read_ack(&mut wire)
    }

    pub fn interrupt(&self, permit: RunPermit) -> io::Result<()> {
        let mut lanes = self.lanes.lock().unwrap();
        let Some(task) = lanes
            .tasks
            .get_mut(&(permit.task.id, permit.task.generation))
        else {
            return Ok(());
        };
        if let Some((active, lease)) = task.active
            && active == permit
        {
            drop(lanes);
            // A delayed interrupt cannot hit a replacement thread (new id)
            // or a later run on this thread (monotonic lease sequence).
            return self.interrupt_lane(lease);
        }
        if permit.epoch > task.last_epoch
            && task
                .early_interrupt
                .is_none_or(|old| old.epoch < permit.epoch)
        {
            task.early_interrupt = Some(permit);
        }
        Ok(())
    }
}

impl WorkerThread {
    pub fn issue(&mut self, permit: RunPermit, regs: &Registers, tls: u64) -> io::Result<()> {
        if permit.task != self.task || self.running.is_some() {
            return Err(invalid("Misrouted or overlapping guest run"));
        }
        let mut lanes = self.control.lanes.lock().unwrap();
        let key = (self.task.id, self.task.generation);
        if permit.epoch <= lanes.tasks[&key].last_epoch {
            return Err(invalid("Stale task epoch"));
        }
        if lanes.busy_cpus.contains(&permit.vcpu) {
            return Err(invalid("Worker vCPU is already running"));
        }
        let sequence = lanes
            .last_sequence
            .checked_add(1)
            .ok_or_else(|| invalid("Worker lease sequence exhausted"))?;
        let mut lane = match lanes.idle.pop() {
            Some(lane) => lane,
            None => self.control.attach_lane(sequence)?,
        };
        let lease = Lease {
            lane: lane.id,
            sequence,
        };
        lanes.last_sequence = sequence;
        lanes.busy_cpus.insert(permit.vcpu);
        let task = lanes.tasks.get_mut(&key).expect("registered worker task");
        task.last_epoch = permit.epoch;
        task.active = Some((permit, lease));
        let cancelled = task.early_interrupt.take() == Some(permit);
        drop(lanes);

        let mut frame = [0; RUN_HEADER + REGISTER_WORDS];
        frame[..7].copy_from_slice(&identity(permit));
        frame[7] = tls;
        frame[8] = u64::from(self.retry_pc.take() == Some(regs.pc));
        frame[9] = sequence;
        encode_registers(&mut frame[RUN_HEADER..], regs);
        let result = (|| {
            if cancelled {
                self.control.interrupt_lane(lease)?;
            }
            write_words(&mut lane.channel, frame)
        })();
        if result.is_err() {
            // A partial send has uncertain execution ownership.
            self.process.terminate()?;
        } else {
            self.running = Some((permit, lane));
        }
        result
    }

    pub fn receive_stop(&mut self) -> io::Result<Stop> {
        let (permit, mut lane) = self
            .running
            .take()
            .ok_or_else(|| invalid("No issued run"))?;
        let result = read_stop(&mut lane.channel).and_then(|stop| {
            (stop.identity == identity(permit))
                .then_some(stop)
                .ok_or_else(|| invalid("Misrouted worker stop"))
        });
        let stop = match result {
            Ok(stop) => stop,
            Err(error) => {
                self.process.terminate()?;
                return Err(error);
            }
        };
        self.retry_pc = stop
            .fault_instruction
            .and_then(crate::exclusive::Access::decode)
            .filter(|a| !a.load && stop.exclusive.is_none())
            .and_then(|_| stop.guest.registers.pc.checked_add(4));
        let mut lanes = self.control.lanes.lock().unwrap();
        lanes
            .tasks
            .get_mut(&(self.task.id, self.task.generation))
            .unwrap()
            .active = None;
        lanes.busy_cpus.remove(&permit.vcpu);
        lanes.idle.push(lane);
        Ok(stop)
    }
}

impl Drop for WorkerThread {
    fn drop(&mut self) {
        if let Some((permit, _)) = self.running.as_ref() {
            // The caller abandoned a live permit. Confirm a stop or process
            // death before another Task can use its executor.
            if self.control.interrupt(*permit).is_err() {
                let _ = self.process.terminate();
            } else {
                let _ = self.receive_stop();
            }
        }
        let mut lanes = self.control.lanes.lock().unwrap();
        lanes.tasks.remove(&(self.task.id, self.task.generation));
        let possible_runs = lanes.tasks.len().saturating_sub(lanes.busy_cpus.len());
        lanes.idle.truncate(possible_runs);
    }
}
