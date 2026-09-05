//! Deterministic device responses exercise the production submit/retire path.

use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

use super::super::control::ControlQueue;
use super::super::{
    VIRTIO_GPU_FLAG_FENCE, VIRTIO_GPU_MAX_OPAQUE_COMMAND_SIZE,
    VIRTIO_GPU_RESP_ERR_INVALID_PARAMETER, VIRTIO_GPU_RESP_OK_NODATA, VirtioGpuCtrlHdr,
};
use super::{ASYNC_CAPACITY, AsyncSubmissions};
use crate::device::gpu::{
    GpuBackendContext, GpuBackendContextInfo, GpuBackendEnqueueError, GpuBackendQueue,
    GpuBackendQueueInfo, GpuBackendSubmitError, GpuCompletion, GpuCompletionFailure,
    GpuCompletionState, GpuSubmission,
};
use crate::sync::IrqSpinLock;

struct TestContext {
    drops: Arc<AtomicUsize>,
    outer_lock: Arc<IrqSpinLock<()>>,
}

impl GpuBackendContext for TestContext {
    fn query_info(&self) -> GpuBackendContextInfo {
        GpuBackendContextInfo::new(0, 0)
    }
    fn create_queue(&self) -> Result<Arc<dyn GpuBackendQueue>, &'static str> {
        Ok(Arc::new(TestQueue))
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        assert!(
            self.outer_lock.try_lock().is_some(),
            "retirement must release the core lock before destruction"
        );
        self.drops.fetch_add(1, Ordering::Relaxed);
    }
}

struct TestQueue;
impl GpuBackendQueue for TestQueue {
    fn query_info(&self) -> GpuBackendQueueInfo {
        GpuBackendQueueInfo::new(VIRTIO_GPU_MAX_OPAQUE_COMMAND_SIZE)
    }
    fn submit(&self, _: &[u8]) -> Result<(), GpuBackendSubmitError> {
        panic!("async path must not call synchronous submit")
    }
}

fn submission(
    commands: &[u8],
    drops: &Arc<AtomicUsize>,
    lock: &Arc<IrqSpinLock<()>>,
) -> (GpuCompletion, GpuSubmission) {
    GpuSubmission::test_request(
        commands.to_vec(),
        Arc::new(TestContext {
            drops: Arc::clone(drops),
            outer_lock: Arc::clone(lock),
        }),
        Arc::new(TestQueue),
    )
}

fn response(fence: Option<u64>, error: bool) -> VirtioGpuCtrlHdr {
    VirtioGpuCtrlHdr {
        hdr_type: if error {
            VIRTIO_GPU_RESP_ERR_INVALID_PARAMETER
        } else {
            VIRTIO_GPU_RESP_OK_NODATA
        },
        flags: if fence.is_some() {
            VIRTIO_GPU_FLAG_FENCE
        } else {
            0
        },
        fence_id: fence.unwrap_or(0),
        ctx_id: 1,
        padding: 0,
    }
}

fn head(control: &ControlQueue, index: usize) -> usize {
    control.ring.avail.ring[index] as usize
}

fn release_unaccepted(error: GpuBackendEnqueueError) {
    match error {
        GpuBackendEnqueueError::Busy(submission)
        | GpuBackendEnqueueError::Rejected(_, submission) => {
            submission.retire_failed(GpuCompletionFailure::Execution)
        }
        GpuBackendEnqueueError::Failed(_) => panic!("batch rejection cannot accept a prefix"),
    }
}

#[test_case]
fn async_submit_returns_pending_and_retires_out_of_order_responses_in_order() {
    let mut control = ControlQueue::new();
    let mut state = AsyncSubmissions::default();
    let drops = Arc::new(AtomicUsize::new(0));
    let lock = Arc::new(IrqSpinLock::new(()));
    let (first, first_request) = submission(&[0; 4], &drops, &lock);
    let (second, second_request) = submission(&[], &drops, &lock);
    state
        .enqueue(&mut control, 1, 1, first_request, 0)
        .expect("first accepted");
    state
        .enqueue(&mut control, 1, 2, second_request, 0)
        .expect("empty checkpoint accepted");
    assert_eq!(*control.ring.avail.idx, 3);
    assert_eq!(*control.ring.used.idx, 0);
    assert_eq!(first.state(), GpuCompletionState::Pending);
    assert_eq!(second.state(), GpuCompletionState::Pending);
    control.test_respond(head(&control, 2), response(Some(2), false));
    assert!(state.poll_one(&mut control, 1).is_none());
    assert_eq!(second.state(), GpuCompletionState::Pending);
    control.test_respond(head(&control, 0), response(None, false));
    control.test_respond(head(&control, 1), response(Some(1), false));
    let retired = {
        let _guard = lock.lock();
        state.poll_one(&mut control, 2).expect("first retired")
    };
    retired.retire();
    assert_eq!(first.state(), GpuCompletionState::Complete);
    assert_eq!(second.state(), GpuCompletionState::Pending);
    state
        .poll_one(&mut control, 2)
        .expect("second retired")
        .retire();
    assert_eq!(second.state(), GpuCompletionState::Complete);
    assert_eq!(drops.load(Ordering::Relaxed), 2);
    assert!(!state.active());
}

#[test_case]
fn async_observer_drop_does_not_release_resources_before_independent_checkpoint() {
    let mut control = ControlQueue::new();
    let mut state = AsyncSubmissions::default();
    let drops = Arc::new(AtomicUsize::new(0));
    let lock = Arc::new(IrqSpinLock::new(()));
    let (completion, request) = submission(&[0; 4], &drops, &lock);
    state
        .enqueue(&mut control, 1, 8, request, 0)
        .expect("accepted");
    drop(completion);
    control.test_respond(head(&control, 0), response(None, false));
    assert!(state.poll_one(&mut control, 1).is_none());
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    control.test_respond(head(&control, 1), response(Some(8), false));
    state
        .poll_one(&mut control, 2)
        .expect("retired despite no observers")
        .retire();
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

#[test_case]
fn async_execution_error_waits_for_prefix_retirement_without_poisoning_queue() {
    let mut control = ControlQueue::new();
    let mut state = AsyncSubmissions::default();
    let drops = Arc::new(AtomicUsize::new(0));
    let lock = Arc::new(IrqSpinLock::new(()));
    let (completion, request) = submission(&[0; 4], &drops, &lock);
    state
        .enqueue(&mut control, 1, 5, request, 0)
        .expect("accepted");
    control.test_respond(head(&control, 0), response(None, true));
    assert!(state.poll_one(&mut control, 1).is_none());
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    control.test_respond(head(&control, 1), response(Some(5), false));
    state
        .poll_one(&mut control, 2)
        .expect("failed prefix is quiescent")
        .retire();
    assert_eq!(
        completion.state(),
        GpuCompletionState::Failed(GpuCompletionFailure::Execution)
    );
    assert_eq!(drops.load(Ordering::Relaxed), 1);
    assert!(control.check().is_ok());
    let (next, request) = submission(&[], &drops, &lock);
    state
        .enqueue(&mut control, 1, 6, request, 3)
        .expect("queue remains usable");
    control.test_respond(head(&control, 2), response(Some(6), false));
    state
        .poll_one(&mut control, 4)
        .expect("next checkpoint")
        .retire();
    assert_eq!(next.state(), GpuCompletionState::Complete);
}

#[test_case]
fn async_bad_checkpoint_quarantines_all_owners_and_blocks_legacy_drain() {
    for bad_response in [
        response(None, false),
        response(Some(99), false),
        response(Some(7), true),
    ] {
        let mut control = ControlQueue::new();
        let mut state = AsyncSubmissions::default();
        let drops = Arc::new(AtomicUsize::new(0));
        let lock = Arc::new(IrqSpinLock::new(()));
        let (completion, request) = submission(&[], &drops, &lock);
        state
            .enqueue(&mut control, 1, 7, request, 0)
            .expect("accepted");
        control.test_respond(head(&control, 0), bad_response);
        // The same transport reap used by synchronous detach/upload must not
        // confuse a returned descriptor with a successful GPU retirement fence.
        assert!(control.reap(1).is_err());
        assert!(state.poll_one(&mut control, 1).is_none());
        assert_eq!(
            completion.state(),
            GpuCompletionState::Failed(GpuCompletionFailure::DeviceLost)
        );
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        assert!(!state.active());
        let (_, rejected) = submission(&[], &drops, &lock);
        release_unaccepted(
            state
                .enqueue(&mut control, 1, 8, rejected, 2)
                .expect_err("failed device rejects new work"),
        );
        drop((control, state, completion));
        assert_eq!(
            drops.load(Ordering::Relaxed),
            1,
            "unretired owner remains quarantined"
        );
    }
}

#[test_case]
fn async_device_budget_and_batch_admission_never_publish_a_rejected_prefix() {
    let mut control = ControlQueue::new();
    let mut state = AsyncSubmissions::default();
    let drops = Arc::new(AtomicUsize::new(0));
    let lock = Arc::new(IrqSpinLock::new(()));
    for index in 0..ASYNC_CAPACITY {
        let (_, request) = submission(&[0; 4], &drops, &lock);
        state
            .enqueue(&mut control, 1, u64::from(index + 1), request, 0)
            .expect("within device bound");
    }
    let available = *control.ring.avail.idx;
    let (_, request) = submission(&[0; 4], &drops, &lock);
    let error = state
        .enqueue(&mut control, 1, 99, request, 0)
        .expect_err("device full");
    assert!(matches!(&error, GpuBackendEnqueueError::Busy(_)));
    release_unaccepted(error);
    assert_eq!(*control.ring.avail.idx, available);
    for index in 0..ASYNC_CAPACITY as usize {
        control.test_respond(head(&control, index * 2), response(None, false));
        control.test_respond(
            head(&control, index * 2 + 1),
            response(Some(index as u64 + 1), false),
        );
    }
    while let Some(retired) = state.poll_one(&mut control, 1) {
        retired.retire();
    }
    assert_eq!(drops.load(Ordering::Relaxed), ASYNC_CAPACITY as usize + 1);
    // A malformed dword stream also fails before any ring publication.
    let (_, request) = submission(&[1; 3], &drops, &lock);
    release_unaccepted(
        state
            .enqueue(&mut control, 1, 100, request, 2)
            .expect_err("invalid byte length"),
    );
    assert_eq!(*control.ring.avail.idx, available);
}

#[test_case]
fn async_checkpoint_timeout_reports_failure_but_keeps_resources() {
    let mut control = ControlQueue::new();
    let mut state = AsyncSubmissions::default();
    let drops = Arc::new(AtomicUsize::new(0));
    let lock = Arc::new(IrqSpinLock::new(()));
    let (completion, request) = submission(&[], &drops, &lock);
    state
        .enqueue(&mut control, 1, 1, request, 0)
        .expect("accepted");
    assert!(
        state
            .poll_one(&mut control, super::super::VIRTIO_GPU_CONTROL_TIMEOUT_NS)
            .is_none()
    );
    assert_eq!(
        completion.state(),
        GpuCompletionState::Failed(GpuCompletionFailure::DeviceLost)
    );
    drop((control, state));
    assert_eq!(drops.load(Ordering::Relaxed), 0);
}
