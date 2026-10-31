//! Real, bounded VirGL submission and autonomous GPU retirement.
//!
//! Each payload is followed by an independent empty, fenced SUBMIT_3D. An
//! error response to the payload alone need not retire an accepted prefix.
//! Publishing both chains together reserves the retirement path before any
//! work becomes visible. Legacy control operations drain these checkpoints
//! before detaching bindings, changing backing, transferring, or presenting.

use alloc::{
    collections::VecDeque,
    string::String,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::device::events::InterruptCapableDevice;
use crate::device::gpu::{
    GPU_MAX_OPAQUE_COMMAND_SIZE, GpuBackendEnqueueError, GpuBackendSubmitError,
    GpuCompletionFailure, GpuSubmission,
};
use crate::drivers::virtio::{device::Register, pci::VirtioPciTransport};
use crate::interrupt::{DeferredInterruptCompletion, InterruptClaim, InterruptId, InterruptResult};
use crate::sync::{IrqSpinLock, Once, Waker};

use super::control::{ControlEnqueueError, ControlQueue, ControlRequest, ControlStatus};
use super::core_lock::CoreLock;
use super::{
    VIRTIO_GPU_CMD_SUBMIT_3D, VIRTIO_GPU_FLAG_FENCE, VirtioGpuCmdSubmit3d, VirtioGpuCtrlHdr,
    VirtioGpuDevice, VirtioGpuDeviceCore, validate_execution_response,
};

// Two descriptor chains (four descriptors) per nonempty submission, shared
// across every context and queue on this device, independently of user handles.
pub(super) const ASYNC_CAPACITY: u32 = 16;
const PROGRESS_INTERVAL_NS: u64 = 1_000_000;

struct PendingSubmission {
    context_id: u32,
    command_len: usize,
    prepared: Prepared,
    submission: GpuSubmission,
}

pub(super) struct RetiredSubmission {
    prepared: Prepared,
    submission: GpuSubmission,
    failed: bool,
}

impl RetiredSubmission {
    pub(super) fn retire(self) {
        // Resource/context destructors issue synchronous GPU commands. This
        // must run after releasing the outer core lock and transport locks.
        let Self { prepared, submission, failed } = self;
        drop(prepared);
        if failed {
            submission
                .retire_failed(GpuCompletionFailure::Execution);
        } else {
            submission.complete();
        }
    }
}

struct Requests {
    payload: Arc<ControlRequest>,
    checkpoint: Arc<ControlRequest>,
}

pub(super) struct RequestPool {
    free: IrqSpinLock<[Option<Requests>; ASYNC_CAPACITY as usize]>,
}
impl RequestPool {
    pub(super) fn new() -> Self {
        Self { free: IrqSpinLock::new(core::array::from_fn(|_| None)) }
    }
}

pub(super) struct Prepared {
    requests: Option<Requests>,
    pool: Arc<RequestPool>,
}
impl Prepared {
    pub(super) fn new(pool: &Arc<RequestPool>, context_id: u32, commands: &[u8]) -> Result<Self, GpuBackendSubmitError> {
        if !commands.len().is_multiple_of(4) || commands.len() > GPU_MAX_OPAQUE_COMMAND_SIZE as usize {
            return Err(GpuBackendSubmitError::Rejected("VirGL commands must be bounded complete dwords"));
        }
        let cached = {
            let mut free = pool.free.lock();
            free.iter_mut().find_map(Option::take)
        };
        let mut prepared = Self { requests: cached, pool: Arc::clone(pool) };
        let refill = |request: &mut Arc<ControlRequest>, header: &[u8], data: &[u8], fence| {
            if let Some(private) = Arc::get_mut(request) {
                private.reuse_parts(header, data, fence)
            } else {
                *request = ControlRequest::allocate_parts(header, data, core::mem::size_of::<VirtioGpuCtrlHdr>(), fence)?;
                Ok(())
            }
        };
        let payload_header = submit_header(context_id, commands.len(), None);
        let checkpoint_header = submit_header(context_id, 0, Some(0));
        // SAFETY: repr(C) transport structures have initialized fields and no
        // implicit padding; the borrowed headers remain live through the copy.
        fn header_bytes(header: &VirtioGpuCmdSubmit3d) -> &[u8] {
            unsafe { core::slice::from_raw_parts(core::ptr::from_ref(header).cast::<u8>(), core::mem::size_of::<VirtioGpuCmdSubmit3d>()) }
        }
        let payload = header_bytes(&payload_header);
        let checkpoint = header_bytes(&checkpoint_header);
        let result = if let Some(requests) = &mut prepared.requests {
            refill(&mut requests.payload, payload, commands, None)
                .and_then(|()| refill(&mut requests.checkpoint, checkpoint, &[], Some(0)))
        } else {
            (|| {
                let payload = ControlRequest::allocate_parts(payload, commands, core::mem::size_of::<VirtioGpuCtrlHdr>(), None)?;
                let checkpoint = ControlRequest::allocate_parts(checkpoint, &[], core::mem::size_of::<VirtioGpuCtrlHdr>(), Some(0))?;
                prepared.requests = Some(Requests { payload, checkpoint });
                Ok(())
            })()
        };
        result.map_err(GpuBackendSubmitError::Unavailable)?;
        Ok(prepared)
    }
}
impl Drop for Prepared {
    fn drop(&mut self) {
        let mut requests = self.requests.take();
        {
            let mut free = self.pool.free.lock();
            if let Some(slot) = free.iter_mut().find(|slot| slot.is_none()) {
                *slot = requests.take();
            }
        }
        // Excess storage and outstanding observer references retire outside
        // the pool lock as well as outside the GPU device-core lock.
        drop(requests);
    }
}

pub(super) struct AsyncSubmissions {
    pending: VecDeque<PendingSubmission>,
    failed: bool,
}
impl Default for AsyncSubmissions {
    fn default() -> Self {
        Self { pending: VecDeque::with_capacity(ASYNC_CAPACITY as usize), failed: false }
    }
}
fn submit_header(context_id: u32, length: usize, fence: Option<u64>) -> VirtioGpuCmdSubmit3d {
    VirtioGpuCmdSubmit3d {
        hdr: VirtioGpuCtrlHdr {
            hdr_type: VIRTIO_GPU_CMD_SUBMIT_3D,
            flags: if fence.is_some() { VIRTIO_GPU_FLAG_FENCE } else { 0 },
            fence_id: fence.unwrap_or(0), ctx_id: context_id, padding: 0,
        },
        size: length as u32, padding: 0,
    }
}

impl AsyncSubmissions {
    pub(super) fn active(&self) -> bool {
        !self.failed && !self.pending.is_empty()
    }

    pub(super) fn enqueue(
        &mut self,
        control: &mut ControlQueue,
        context_id: u32,
        fence: u64,
        submission: GpuSubmission,
        prepared: &mut Option<Prepared>,
        now_ns: u64,
    ) -> Result<(), GpuBackendEnqueueError> {
        if let Err(error) = control.check() {
            return Err(GpuBackendEnqueueError::Rejected(GpuBackendSubmitError::DeviceLost(error), submission));
        }
        if self.pending.len() >= ASYNC_CAPACITY as usize {
            return Err(GpuBackendEnqueueError::Busy(submission));
        }
        let command_len = submission.commands().len();
        let requests = prepared.as_mut().and_then(|p| p.requests.as_mut()).expect("prepared DMA storage");
        let Some(checkpoint) = Arc::get_mut(&mut requests.checkpoint) else {
            return Err(GpuBackendEnqueueError::Rejected(GpuBackendSubmitError::Unavailable("Checkpoint storage is not private"), submission));
        };
        checkpoint.set_checkpoint_fence(fence);
        let published = if command_len != 0 {
            control.enqueue_batch(&[Arc::clone(&requests.payload), Arc::clone(&requests.checkpoint)], now_ns)
        } else {
            control.enqueue(&requests.checkpoint, now_ns)
        };
        if let Err(error) = published {
            return Err(match error {
                ControlEnqueueError::Busy => GpuBackendEnqueueError::Busy(submission),
                ControlEnqueueError::Failed(error) => GpuBackendEnqueueError::Rejected(GpuBackendSubmitError::Unavailable(error), submission),
            });
        }
        // Fixed admission capacity was reserved at device initialization.
        // No allocation or payload copy occurs under the publication lock.
        self.pending.push_back(PendingSubmission {
            context_id, command_len, prepared: prepared.take().expect("accepted DMA storage"), submission,
        });
        Ok(())
    }

    pub(super) fn poll_one(
        &mut self,
        control: &mut ControlQueue,
        now_ns: u64,
    ) -> Option<RetiredSubmission> {
        if control.reap(now_ns).is_err() {
            self.failed = true;
            for pending in &mut self.pending {
                pending.submission.fail(GpuCompletionFailure::DeviceLost);
            }
            // A failed fence/transport cannot certify retirement. Keep every
            // backing reference, driver request and admission slot quarantined.
            return None;
        }
        let pending = self.pending.front()?;
        let requests = pending.prepared.requests.as_ref().expect("pending DMA storage");
        if !matches!(requests.checkpoint.status(), ControlStatus::Complete(_)) {
            return None;
        }
        let failed = if pending.command_len != 0 {
            let payload = &requests.payload;
            if !matches!(payload.status(), ControlStatus::Complete(_)) {
                return None;
            }
            let mut bytes = [0; core::mem::size_of::<VirtioGpuCtrlHdr>()];
            payload.copy_response(&mut bytes).ok()?;
            // SAFETY: byte storage contains one returned control header; it may
            // be unaligned and remains live for this copy.
            let response =
                unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const VirtioGpuCtrlHdr) };
            if let Err(error) = validate_execution_response(response, None) {
                crate::println!(
                    "[VirtIO GPU] submission rejected: context={} bytes={} response={:#x} error={}",
                    pending.context_id,
                    pending.command_len,
                    response.hdr_type,
                    error
                );
                true
            } else {
                false
            }
        } else {
            false
        };
        // Retire in publication order even if the device returns used entries
        // out of order. A later receipt never skips an earlier pending request.
        let pending = self.pending.pop_front().expect("ready front submission");
        Some(RetiredSubmission {
            prepared: pending.prepared,
            submission: pending.submission,
            failed,
        })
    }
}

static DEVICES: Once<IrqSpinLock<Vec<Weak<CoreLock<VirtioGpuDeviceCore>>>>> = Once::new();
static WORKER_STARTED: AtomicBool = AtomicBool::new(false);
static WORKER_TASK: AtomicUsize = AtomicUsize::new(usize::MAX);

pub(super) fn in_worker() -> bool {
    crate::sched::scheduler::current_task_id(crate::arch::get_cpu().get_cpuid())
        == Some(WORKER_TASK.load(Ordering::Acquire))
}
static WORKER_WAKER: Waker = Waker::new_uninterruptible("virtio-gpu-completion");

pub(super) fn wake_worker() {
    WORKER_WAKER.wake_one();
}

pub(super) fn register(core: &Arc<CoreLock<VirtioGpuDeviceCore>>) -> bool {
    let weak = Arc::downgrade(core);
    {
        let mut devices = DEVICES.call_once(|| IrqSpinLock::new(Vec::new())).lock();
        if !devices
            .iter()
            .any(|registered| Weak::ptr_eq(registered, &weak))
        {
            if devices.try_reserve(1).is_err() {
                return false;
            }
            devices.push(weak);
        }
    }
    if WORKER_STARTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        let task =
            crate::task::new_kernel_task(String::from("virtio-gpu-completion"), 1, worker_entry);
        task.init();
        crate::sched::scheduler::add_task(task, crate::arch::get_cpu().get_cpuid());
    }
    core.lock()
        .interrupt_state
        .deferred_enabled
        .store(true, Ordering::Release);
    true
}

fn process_device(core: &Arc<CoreLock<VirtioGpuDeviceCore>>) -> bool {
    let control = {
        let Some(core) = core.try_lock() else {
            return true;
        };
        Arc::clone(&core.control)
    };
    control.synchronous.prune();
    if !super::prune_abandoned_presentations(core) {
        return true;
    }
    for _ in 0..ASYNC_CAPACITY {
        let (retired, presentations, active, interrupt, completion_count, caught_up) = {
            let Some(mut guard) = core.try_lock() else {
                return true;
            };
            let core = &mut *guard;
            let interrupt = Arc::clone(&core.interrupt_state);
            let completion_count = interrupt.completions.lock().len();
            let mut queues = control.virtqueues.lock();
            if completion_count != 0 {
                queues.control.suppress_notifications();
                // Only the owner of a deferred delivery may clear ISR state.
                // Timed polling must not consume a not-yet-claimed interrupt.
                interrupt.acknowledge();
            }
            let retired = core
                .async_submissions
                .poll_one(&mut queues.control, crate::timer::get_time_ns());
            let caught_up = if completion_count == 0 || queues.control.check().is_err() {
                // Timed progress may reap replies but must not rearm an IRQ
                // whose delivery has not yet reached deferred_interrupt_ready.
                // Otherwise it could request a second notification before the
                // first delivery has been acknowledged by its owning worker.
                // Keep a failed queue quiet; no further replies can be retired.
                true
            } else {
                queues.control.rearm_notifications()
            };
            drop(queues);
            let presentations = super::remove_presentations(&mut core.presentations, &[]);
            (
                retired,
                presentations,
                core.async_submissions.active()
                    || control.synchronous.active()
                    || super::presentation_progress_needed(&core.presentations)
                    || {
                        let queues = core.virtqueues.lock();
                        queues.control.check().is_ok() && queues.control.has_pending()
                    },
                interrupt,
                completion_count,
                caught_up,
            )
        };
        // Leases may destroy images and re-enter this core; its guard is gone.
        drop(presentations);
        let released = caught_up && interrupt.complete(completion_count);
        if let Some(retired) = retired {
            retired.retire();
        } else if caught_up {
            return active || !released;
        }
    }
    true
}

fn worker_entry() {
    if let Some(task) = crate::task::mytask() {
        WORKER_TASK.store(task.get_id(), Ordering::Release);
    }
    loop {
        let devices = {
            let mut devices = DEVICES.call_once(|| IrqSpinLock::new(Vec::new())).lock();
            devices.retain(|device| device.strong_count() != 0);
            devices.clone()
        };
        let mut active = false;
        for device in devices {
            if let Some(device) = device.upgrade() {
                active |= process_device(&device);
            }
        }
        let Some(task) = crate::task::mytask() else {
            crate::arch::instruction::idle();
        };
        if active {
            // IRQs wake promptly; the timer also guarantees retirement/timeout
            // progress when an IRQ is absent, lost, or not routed on a platform.
            WORKER_WAKER.wait_with_timeout(
                task.get_id(),
                task.get_trapframe(),
                Some(PROGRESS_INTERVAL_NS),
            );
        } else {
            WORKER_WAKER.wait(task.get_id(), task.get_trapframe());
        }
    }
}

pub(super) struct InterruptState {
    base_addr: usize,
    pci_transport: Option<VirtioPciTransport>,
    id: IrqSpinLock<Option<InterruptId>>,
    pub(super) deferred_enabled: AtomicBool,
    completions: IrqSpinLock<VecDeque<DeferredInterruptCompletion>>,
}

impl InterruptState {
    pub(super) fn new(base_addr: usize, pci_transport: Option<VirtioPciTransport>) -> Self {
        Self {
            base_addr,
            pci_transport,
            id: IrqSpinLock::new(None),
            deferred_enabled: AtomicBool::new(false),
            completions: IrqSpinLock::new(VecDeque::with_capacity(1)),
        }
    }

    fn complete(&self, count: usize) -> bool {
        for _ in 0..count {
            let Some(mut completion) = self.completions.lock().pop_front() else {
                break;
            };
            if completion.complete().is_err() {
                self.completions.lock().push_front(completion);
                return false;
            }
        }
        true
    }

    fn pending(&self) -> bool {
        crate::arch::io_mb();
        // PCI's ISR is read-to-clear, so checking a shared INTx line also
        // acknowledges it. MMIO preserves the cause until the core masks the
        // line and hands ownership to the worker.
        let status = unsafe {
            if let Some(pci) = self.pci_transport {
                u32::from(crate::arch::mmio::read8(pci.isr_cfg))
            } else {
                crate::arch::mmio::read32(self.base_addr + Register::InterruptStatus.offset())
            }
        };
        crate::arch::io_mb();
        status & 3 != 0
    }

    fn acknowledge(&self) -> u32 {
        crate::arch::io_mb();
        // SAFETY: these mapped registers have the same lifetime as the device.
        // Only ISR status/ack is touched, never queue-select state. In particular
        // the interrupt path must not acquire the outer synchronous core lock.
        let status = unsafe {
            if let Some(pci) = self.pci_transport {
                u32::from(crate::arch::mmio::read8(pci.isr_cfg)) // read-to-clear
            } else {
                let status =
                    crate::arch::mmio::read32(self.base_addr + Register::InterruptStatus.offset());
                if status & 3 != 0 {
                    crate::arch::mmio::write32(
                        self.base_addr + Register::InterruptAck.offset(),
                        status & 3,
                    );
                }
                status
            }
        };
        crate::arch::io_mb();
        status & 3
    }
}

impl VirtioGpuDevice {
    /// Connect device-side completion notification to an already registered IRQ.
    ///
    /// # Arguments
    ///
    /// * `interrupt_id` - Interrupt controller line registered for this device.
    ///
    /// # Returns
    ///
    /// Nothing. Acknowledge stale status and wake the completion worker. A timed
    /// fallback remains active for outstanding work even without a usable IRQ.
    pub fn enable_interrupts(&self, interrupt_id: InterruptId) {
        *self.interrupt_state.id.lock() = Some(interrupt_id);
        self.interrupt_state.acknowledge();
        wake_worker();
    }
}

impl InterruptCapableDevice for VirtioGpuDevice {
    fn interrupt_id(&self) -> Option<InterruptId> {
        *self.interrupt_state.id.lock()
    }

    fn handle_interrupt(&self) -> InterruptResult<()> {
        self.claim_interrupt()?;
        Ok(())
    }

    fn claim_interrupt(&self) -> InterruptResult<InterruptClaim> {
        if self
            .interrupt_state
            .deferred_enabled
            .load(Ordering::Acquire)
        {
            return Ok(if self.interrupt_state.pending() {
                InterruptClaim::Deferred
            } else {
                InterruptClaim::NotMine
            });
        }
        if self.interrupt_state.acknowledge() == 0 {
            return Ok(InterruptClaim::NotMine);
        }
        // No allocation, GPU wait, or resource destructor runs in IRQ context.
        wake_worker();
        Ok(InterruptClaim::Handled)
    }

    fn deferred_interrupt_ready(&self, completion: DeferredInterruptCompletion) {
        self.interrupt_state
            .completions
            .lock()
            .push_back(completion);
        wake_worker();
    }
}

#[cfg(test)]
mod tests;
