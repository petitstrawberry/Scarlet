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
use core::sync::atomic::{AtomicBool, Ordering};

use crate::device::events::InterruptCapableDevice;
use crate::device::gpu::{
    GpuBackendEnqueueError, GpuBackendSubmitError, GpuCompletionFailure, GpuSubmission,
};
use crate::drivers::virtio::{device::Register, pci::VirtioPciTransport};
use crate::interrupt::{InterruptClaim, InterruptId, InterruptResult};
use crate::sync::{IrqSpinLock, Once, Waker};

use super::control::{ControlEnqueueError, ControlQueue, ControlRequest, ControlStatus};
use super::{
    VIRTIO_GPU_CMD_SUBMIT_3D, VIRTIO_GPU_FLAG_FENCE, VIRTIO_GPU_MAX_OPAQUE_COMMAND_SIZE,
    VirtioGpuCmdSubmit3d, VirtioGpuCtrlHdr, VirtioGpuDevice, VirtioGpuDeviceCore, append_pod_bytes,
    validate_execution_response,
};

// Two descriptor chains (four descriptors) per nonempty submission, shared
// across every context and queue on this device, independently of user handles.
pub(super) const ASYNC_CAPACITY: u32 = 16;
const PROGRESS_INTERVAL_NS: u64 = 1_000_000;

struct PendingSubmission {
    payload: Option<Arc<ControlRequest>>,
    checkpoint: Arc<ControlRequest>,
    submission: GpuSubmission,
}

pub(super) struct RetiredSubmission {
    submission: GpuSubmission,
    failed: bool,
}

impl RetiredSubmission {
    pub(super) fn retire(self) {
        // Resource/context destructors issue synchronous GPU commands. This
        // must run after releasing the outer core lock and transport locks.
        if self.failed {
            self.submission
                .retire_failed(GpuCompletionFailure::Execution);
        } else {
            self.submission.complete();
        }
    }
}

#[derive(Default)]
pub(super) struct AsyncSubmissions {
    pending: VecDeque<PendingSubmission>,
    failed: bool,
}

fn command_bytes(
    context_id: u32,
    commands: &[u8],
    fence: Option<u64>,
) -> Result<Vec<u8>, &'static str> {
    let header = VirtioGpuCmdSubmit3d {
        hdr: VirtioGpuCtrlHdr {
            hdr_type: VIRTIO_GPU_CMD_SUBMIT_3D,
            flags: if fence.is_some() {
                VIRTIO_GPU_FLAG_FENCE
            } else {
                0
            },
            fence_id: fence.unwrap_or(0),
            ctx_id: context_id,
            padding: 0,
        },
        size: commands.len() as u32,
        padding: 0,
    };
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(core::mem::size_of::<VirtioGpuCmdSubmit3d>() + commands.len())
        .map_err(|_| "Failed to allocate VirtIO GPU submit header")?;
    append_pod_bytes(&mut bytes, &header);
    bytes.extend_from_slice(commands);
    Ok(bytes)
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
        now_ns: u64,
    ) -> Result<(), GpuBackendEnqueueError> {
        if let Err(error) = control.check() {
            return Err(GpuBackendEnqueueError::Rejected(
                GpuBackendSubmitError::DeviceLost(error),
                submission,
            ));
        }
        if self.pending.len() >= ASYNC_CAPACITY as usize {
            return Err(GpuBackendEnqueueError::Busy(submission));
        }
        let commands = submission.commands();
        if !commands.len().is_multiple_of(4)
            || commands.len() > VIRTIO_GPU_MAX_OPAQUE_COMMAND_SIZE as usize
        {
            return Err(GpuBackendEnqueueError::Rejected(
                GpuBackendSubmitError::Rejected("VirGL commands must be bounded complete dwords"),
                submission,
            ));
        }
        let prepared = (|| {
            self.pending
                .try_reserve(1)
                .map_err(|_| "Failed to retain VirtIO GPU async submission")?;
            let payload = if commands.is_empty() {
                None
            } else {
                Some(ControlRequest::new(
                    &command_bytes(context_id, commands, None)?,
                    core::mem::size_of::<VirtioGpuCtrlHdr>(),
                )?)
            };
            let checkpoint = ControlRequest::new_checkpoint(
                &command_bytes(context_id, &[], Some(fence))?,
                fence,
            )?;
            Ok::<_, &'static str>((payload, checkpoint))
        })();
        let (payload, checkpoint) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                return Err(GpuBackendEnqueueError::Rejected(
                    GpuBackendSubmitError::Unavailable(error),
                    submission,
                ));
            }
        };
        let published = if let Some(payload) = &payload {
            control.enqueue_batch(&[Arc::clone(payload), Arc::clone(&checkpoint)], now_ns)
        } else {
            control.enqueue(&checkpoint, now_ns)
        };
        if let Err(error) = published {
            return Err(match error {
                ControlEnqueueError::Busy => GpuBackendEnqueueError::Busy(submission),
                ControlEnqueueError::Failed(error) => GpuBackendEnqueueError::Rejected(
                    GpuBackendSubmitError::Unavailable(error),
                    submission,
                ),
            });
        }
        // Capacity is reserved and publication has no fallible successor. The
        // caller holds the core lock until this owner is visible to the worker.
        self.pending.push_back(PendingSubmission {
            payload,
            checkpoint,
            submission,
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
        if !matches!(pending.checkpoint.status(), ControlStatus::Complete(_)) {
            return None;
        }
        let failed = if let Some(payload) = &pending.payload {
            if !matches!(payload.status(), ControlStatus::Complete(_)) {
                return None;
            }
            let mut bytes = [0; core::mem::size_of::<VirtioGpuCtrlHdr>()];
            payload.copy_response(&mut bytes).ok()?;
            // SAFETY: byte storage contains one returned control header; it may
            // be unaligned and remains live for this copy.
            let response =
                unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const VirtioGpuCtrlHdr) };
            validate_execution_response(response, None).is_err()
        } else {
            false
        };
        // Retire in publication order even if the device returns used entries
        // out of order. A later receipt never skips an earlier pending request.
        let pending = self.pending.pop_front().expect("ready front submission");
        Some(RetiredSubmission {
            submission: pending.submission,
            failed,
        })
    }
}

static DEVICES: Once<IrqSpinLock<Vec<Weak<IrqSpinLock<VirtioGpuDeviceCore>>>>> = Once::new();
static WORKER_STARTED: AtomicBool = AtomicBool::new(false);
static WORKER_WAKER: Waker = Waker::new_uninterruptible("virtio-gpu-completion");

pub(super) fn wake_worker() {
    WORKER_WAKER.wake_one();
}

pub(super) fn register(core: &Arc<IrqSpinLock<VirtioGpuDeviceCore>>) -> bool {
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
    true
}

fn process_device(core: &Arc<IrqSpinLock<VirtioGpuDeviceCore>>) -> bool {
    for _ in 0..ASYNC_CAPACITY {
        let retired = {
            let Some(mut guard) = core.try_lock() else {
                return true;
            };
            let core = &mut *guard;
            let mut queues = core.virtqueues.lock();
            let retired = core
                .async_submissions
                .poll_one(&mut queues.control, crate::timer::get_time_ns());
            if retired.is_none() {
                return core.async_submissions.active();
            }
            retired
        };
        if let Some(retired) = retired {
            retired.retire();
        }
    }
    true
}

fn worker_entry() {
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
}

impl InterruptState {
    pub(super) fn new(base_addr: usize, pci_transport: Option<VirtioPciTransport>) -> Self {
        Self {
            base_addr,
            pci_transport,
            id: IrqSpinLock::new(None),
        }
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
        if self.interrupt_state.acknowledge() == 0 {
            return Ok(InterruptClaim::NotMine);
        }
        // No allocation, GPU wait, or resource destructor runs in IRQ context.
        wake_worker();
        Ok(InterruptClaim::Handled)
    }
}

#[cfg(test)]
mod tests;
