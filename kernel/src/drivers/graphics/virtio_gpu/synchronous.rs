//! Synchronous ABI calls over owned control requests. Only admission is locked;
//! task callers sleep with IRQs enabled while the completion worker reaps DMA.

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

use crate::drivers::virtio::device::VirtioDevice;
use crate::sync::{IrqSpinLock, IrqSpinLockGuard};
use crate::task::{Task, TaskState};

use super::VirtioGpuControl;
use super::control::{ControlEnqueueError, ControlQueue, ControlRequest, ControlStatus};

struct Operation {
    request: Arc<ControlRequest>,
    owner: Option<Weak<Task>>,
    awaiting_admission: bool,
    ordered: bool,
    barrier: Option<Arc<ControlRequest>>,
}

pub(super) struct Operations {
    pending: IrqSpinLock<Vec<Operation>>,
}

pub(super) struct Admission<'a>(IrqSpinLockGuard<'a, Vec<Operation>>);

impl Admission<'_> {
    pub(super) fn blocked(&self) -> bool {
        self.0
            .iter()
            .any(|operation| operation.ordered && operation.awaiting_admission)
    }
}

enum Progress {
    Published,
    Wait(Weak<ControlRequest>),
}

impl Operations {
    pub(super) fn new() -> Self {
        Self {
            pending: IrqSpinLock::new(Vec::new()),
        }
    }

    pub(super) fn active(&self) -> bool {
        !self.pending.lock().is_empty()
    }

    pub(super) fn try_admission(&self) -> Option<Admission<'_>> {
        self.pending.try_lock().map(Admission)
    }

    fn register(
        &self,
        request: Arc<ControlRequest>,
        owner: Option<Weak<Task>>,
        ordered: bool,
    ) -> Result<Weak<ControlRequest>, &'static str> {
        let mut pending = self.pending.lock();
        pending
            .try_reserve(1)
            .map_err(|_| "Failed to retain VirtIO GPU control operation")?;
        let identity = Arc::downgrade(&request);
        pending.push(Operation {
            request,
            owner,
            awaiting_admission: true,
            ordered,
            barrier: None,
        });
        // Publication and wake share this guard, including a task that exits
        // before it gets as far as publishing descriptors.
        super::asynchronous::wake_worker();
        Ok(identity)
    }

    #[cfg(test)]
    fn advance(
        &self,
        identity: &Weak<ControlRequest>,
        queue: &mut ControlQueue,
        now_ns: u64,
    ) -> Result<Progress, &'static str> {
        let mut pending = self.pending.lock();
        self.advance_locked(&mut pending, identity, queue, now_ns)
    }

    fn advance_locked(
        &self,
        pending: &mut [Operation],
        identity: &Weak<ControlRequest>,
        queue: &mut ControlQueue,
        now_ns: u64,
    ) -> Result<Progress, &'static str> {
        let operation = pending
            .iter_mut()
            .find(|operation| Weak::ptr_eq(&Arc::downgrade(&operation.request), identity))
            .ok_or("VirtIO GPU control operation was abandoned")?;
        queue.reap(now_ns)?;
        if operation.ordered
            && let Some(barrier) = queue.pending_request()
        {
            let weak = Arc::downgrade(&barrier);
            operation.barrier = Some(barrier);
            return Ok(Progress::Wait(weak));
        }
        match queue.enqueue(&operation.request, now_ns) {
            Ok(()) => {
                operation.awaiting_admission = false;
                operation.barrier = None;
                Ok(Progress::Published)
            }
            Err(ControlEnqueueError::Failed(error)) => Err(error),
            Err(ControlEnqueueError::Busy) => {
                let barrier = queue
                    .pending_request()
                    .ok_or("VirtIO GPU control queue has no free descriptors")?;
                let weak = Arc::downgrade(&barrier);
                operation.barrier = Some(barrier);
                Ok(Progress::Wait(weak))
            }
        }
    }

    fn remove(&self, identity: &Weak<ControlRequest>) {
        let retired = {
            let mut pending = self.pending.lock();
            pending
                .iter()
                .position(|operation| Weak::ptr_eq(&Arc::downgrade(&operation.request), identity))
                .map(|index| pending.swap_remove(index))
        };
        // DMA allocations can be freed only outside the bookkeeping guard.
        // Published requests independently belong to ControlQueue until return
        // (or remain quarantined there after transport failure).
        drop(retired);
    }

    pub(super) fn prune(&self) {
        let owners: Vec<_> = {
            self.pending
                .lock()
                .iter()
                .filter_map(|operation| {
                    operation
                        .owner
                        .as_ref()
                        .map(|owner| (Arc::downgrade(&operation.request), owner.clone()))
                })
                .collect()
        };
        for (identity, owner) in owners {
            // Dropping a last Task Arc may close GPU handles. Do this without
            // either the transport lock or mutable device-core guard.
            let live = owner.upgrade().is_some_and(|task| {
                !matches!(task.get_state(), TaskState::Zombie | TaskState::Terminated)
            });
            if !live {
                self.remove(&identity);
            }
        }
    }

    fn wait(
        control: &VirtioGpuControl,
        request: Weak<ControlRequest>,
        sleep: bool,
    ) -> Result<(), &'static str> {
        if sleep {
            return ControlRequest::wait_owned(
                request
                    .upgrade()
                    .ok_or("VirtIO GPU control owner was abandoned")?,
            );
        }
        // Boot/IRQ-protected cleanup cannot schedule. The completion worker
        // must also reap directly instead of waiting for itself. Neither path
        // retains the mutable GPU core guard here except cold display setup.
        loop {
            {
                let mut queues = control.virtqueues.lock();
                queues.control.reap(crate::timer::get_time_ns())?;
            }
            match request
                .upgrade()
                .ok_or("VirtIO GPU control owner was abandoned")?
                .status()
            {
                ControlStatus::Complete(_) => return Ok(()),
                ControlStatus::Failed(error) => return Err(error),
                ControlStatus::Pending => core::hint::spin_loop(),
            }
        }
    }

    pub(super) fn execute(
        &self,
        control: &VirtioGpuControl,
        commands: &[u8],
        response: &mut [u8],
        ordered: bool,
    ) -> Result<(), &'static str> {
        let sleep = crate::sync::preemptible()
            && control
                .interrupt_state
                .deferred_enabled
                .load(core::sync::atomic::Ordering::Acquire)
            && !super::asynchronous::in_worker()
            && crate::task::mytask().is_some();
        let owner = if sleep {
            let task_id = crate::task::mytask()
                .ok_or("VirtIO GPU control requires a task")?
                .get_id();
            Some(
                crate::sched::scheduler::get_task_by_id(task_id)
                    .map(|task| Arc::downgrade(&task))
                    .ok_or("VirtIO GPU control task is not registered")?,
            )
        } else {
            None
        };
        let identity = self.register(
            ControlRequest::new(commands, response.len())?,
            owner,
            ordered,
        )?;
        let result = (|| {
            loop {
                // Same order as async admission: operation gate, then ring.
                let progress = {
                    let mut operations = self.pending.lock();
                    let mut queues = control.virtqueues.lock();
                    let progress = self.advance_locked(
                        &mut operations,
                        &identity,
                        &mut queues.control,
                        crate::timer::get_time_ns(),
                    )?;
                    if matches!(progress, Progress::Published) {
                        // Do not allow forced exit between publication and kick.
                        control.notify(0);
                        super::asynchronous::wake_worker();
                    }
                    progress
                };
                match progress {
                    Progress::Published => break,
                    Progress::Wait(barrier) => {
                        super::asynchronous::wake_worker();
                        Self::wait(control, barrier, sleep)?;
                    }
                }
            }
            Self::wait(control, identity.clone(), sleep)?;
            identity
                .upgrade()
                .ok_or("VirtIO GPU control owner was abandoned")?
                .copy_response(response)?;
            Ok(())
        })();
        self.remove(&identity);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        VIRTIO_GPU_CONTROL_TIMEOUT_NS, VIRTIO_GPU_RESP_OK_NODATA, VirtioGpuCtrlHdr,
    };
    use super::*;
    use crate::task::TaskType;
    use core::sync::atomic::Ordering;

    fn request() -> Arc<ControlRequest> {
        ControlRequest::new(&[0; 24], 24).unwrap()
    }

    fn respond(queue: &mut ControlQueue, index: usize) {
        let head = queue.ring.avail.ring[index] as usize;
        queue.test_respond(
            head,
            VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_RESP_OK_NODATA,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
        );
    }

    #[test_case]
    fn setup_admits_and_completes_without_holding_core_or_draining_old_work() {
        let mut registers = [0u32; 128];
        let core = super::super::CoreLock::new(super::super::VirtioGpuDeviceCore::new(
            registers.as_mut_ptr() as usize,
        ));
        let control = Arc::clone(&core.lock().control);
        let old = request();
        control.virtqueues.lock().control.enqueue(&old, 0).unwrap();
        let identity = control
            .synchronous
            .register(request(), None, false)
            .unwrap();
        let before = crate::sync::preempt_count();
        {
            let mut queues = control.virtqueues.lock();
            assert!(matches!(
                control
                    .synchronous
                    .advance(&identity, &mut queues.control, 1)
                    .unwrap(),
                Progress::Published
            ));
        }
        assert_eq!(crate::sync::preempt_count(), before);
        assert!(
            core.try_lock().is_some(),
            "a pending host reply must not retain the GPU core"
        );
        assert!(!control.synchronous.try_admission().unwrap().blocked());
        {
            let mut queues = control.virtqueues.lock();
            respond(&mut queues.control, 1);
            queues.control.reap(2).unwrap();
        }
        assert!(matches!(
            identity.upgrade().unwrap().status(),
            ControlStatus::Complete(_)
        ));
        assert_eq!(old.status(), ControlStatus::Pending);
        control.synchronous.remove(&identity);
        assert!(identity.upgrade().is_none());
        // Return the deliberately delayed work so the test frees its DMA too.
        let mut queues = control.virtqueues.lock();
        respond(&mut queues.control, 0);
        queues.control.reap(3).unwrap();
    }

    #[test_case]
    fn ordered_control_preserves_barrier_then_releases_admission_before_response() {
        let operations = Operations::new();
        let mut queue = ControlQueue::new();
        let old = request();
        queue.enqueue(&old, 0).unwrap();
        let identity = operations.register(request(), None, true).unwrap();
        assert!(matches!(
            operations.advance(&identity, &mut queue, 1).unwrap(),
            Progress::Wait(_)
        ));
        assert_eq!(
            *queue.ring.avail.idx, 1,
            "do not publish the destructive command ahead of retirement"
        );
        assert!(operations.try_admission().unwrap().blocked());
        respond(&mut queue, 0);
        assert!(matches!(
            operations.advance(&identity, &mut queue, 2).unwrap(),
            Progress::Published
        ));
        assert!(
            !operations.try_admission().unwrap().blocked(),
            "new draws may follow the published ordered command"
        );
        assert_eq!(identity.upgrade().unwrap().status(), ControlStatus::Pending);
        respond(&mut queue, 1);
        queue.reap(3).unwrap();
        operations.remove(&identity);
        assert!(identity.upgrade().is_none());
    }

    #[test_case]
    fn descriptor_pressure_retries_without_publishing_a_prefix_or_retaining_irq_guard() {
        let operations = Operations::new();
        let mut queue = ControlQueue::new();
        let mut requests = Vec::new();
        for _ in 0..32 {
            let request = request();
            queue.enqueue(&request, 0).unwrap();
            requests.push(request);
        }
        let identity = operations.register(request(), None, false).unwrap();
        let before = crate::sync::preempt_count();
        assert!(matches!(
            operations.advance(&identity, &mut queue, 1).unwrap(),
            Progress::Wait(_)
        ));
        assert_eq!(crate::sync::preempt_count(), before);
        assert_eq!(*queue.ring.avail.idx, 32);
        respond(&mut queue, 0);
        assert!(matches!(
            operations.advance(&identity, &mut queue, 2).unwrap(),
            Progress::Published
        ));
        for index in 1..33 {
            respond(&mut queue, index);
        }
        queue.reap(3).unwrap();
        operations.remove(&identity);
        assert!(identity.upgrade().is_none());
    }

    #[test_case]
    fn exited_control_caller_releases_barrier_and_admission_reservation() {
        let operations = Operations::new();
        let mut queue = ControlQueue::new();
        let owner = Arc::new(Task::new("sync-control-owner".into(), 1, TaskType::Kernel));
        let old = request();
        queue.enqueue(&old, 0).unwrap();
        let identity = operations
            .register(request(), Some(Arc::downgrade(&owner)), true)
            .unwrap();
        assert!(matches!(
            operations.advance(&identity, &mut queue, 1).unwrap(),
            Progress::Wait(_)
        ));
        owner.state.store(TaskState::Zombie, Ordering::SeqCst);
        operations.prune();
        assert!(identity.upgrade().is_none());
        assert!(!operations.try_admission().unwrap().blocked());
        assert_eq!(
            Arc::strong_count(&old),
            2,
            "only the test and transport own the old request"
        );
        respond(&mut queue, 0);
        queue.reap(2).unwrap();
    }

    #[test_case]
    fn exited_published_control_caller_leaves_dma_owned_until_hardware_return() {
        let operations = Operations::new();
        let mut queue = ControlQueue::new();
        let owner = Arc::new(Task::new(
            "published-sync-control".into(),
            1,
            TaskType::Kernel,
        ));
        let identity = operations
            .register(request(), Some(Arc::downgrade(&owner)), true)
            .unwrap();
        assert!(matches!(
            operations.advance(&identity, &mut queue, 0).unwrap(),
            Progress::Published
        ));
        owner.state.store(TaskState::Terminated, Ordering::SeqCst);
        operations.prune();
        assert!(identity.upgrade().is_some());
        assert!(!operations.try_admission().unwrap().blocked());
        respond(&mut queue, 0);
        queue.reap(1).unwrap();
        assert!(identity.upgrade().is_none());
    }

    #[test_case]
    fn failed_control_transport_quarantines_dma_after_caller_exit() {
        let operations = Operations::new();
        let mut queue = ControlQueue::new();
        let owner = Arc::new(Task::new("failed-sync-control".into(), 1, TaskType::Kernel));
        let identity = operations
            .register(request(), Some(Arc::downgrade(&owner)), true)
            .unwrap();
        assert!(matches!(
            operations.advance(&identity, &mut queue, 0).unwrap(),
            Progress::Published
        ));
        assert!(queue.reap(VIRTIO_GPU_CONTROL_TIMEOUT_NS).is_err());
        owner.state.store(TaskState::Zombie, Ordering::SeqCst);
        operations.prune();
        assert!(!operations.try_admission().unwrap().blocked());
        assert!(matches!(
            identity.upgrade().unwrap().status(),
            ControlStatus::Failed(_)
        ));
        drop(queue);
        assert!(
            identity.upgrade().is_some(),
            "failure is not DMA quiescence"
        );
    }
}
