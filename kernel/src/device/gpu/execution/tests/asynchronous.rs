//! Deterministic async admission tests; these do not simulate hardware execution.

use alloc::{collections::VecDeque, sync::Arc};
use core::sync::atomic::{AtomicU32, Ordering};

use super::{TestBufferBackend, TestContext};
use crate::device::gpu::{
    GPU_ABI_VERSION, GPU_RESULT_BUSY, GPU_RESULT_INVALID_ABI, GPU_RESULT_INVALID_ARGUMENT,
    GPU_RESULT_OUT_OF_RESOURCES, GPU_RESULT_SUCCESS, GPU_RESULT_UNSUPPORTED,
    GpuBackendEnqueueError, GpuBackendQueue, GpuBackendQueueInfo, GpuBackendSubmitError, GpuBuffer,
    GpuCompletion, GpuCompletionFailure, GpuContext, GpuQueue, GpuQueueAsyncInfo,
    GpuQueueSubmitAsync, GpuSubmission,
};
use crate::object::KernelObject;
use crate::object::capability::selectable::ReadyInterest;
use crate::object::handle::{AccessMode, HandleTable};
use crate::sync::{IrqSpinLock, Mutex};

struct AsyncQueue {
    pending: Mutex<VecDeque<GpuSubmission>>,
    mode: AtomicU32,
}

impl AsyncQueue {
    fn take_next(&self) -> GpuSubmission {
        self.pending
            .lock()
            .pop_front()
            .expect("accepted submission")
    }
}

impl GpuBackendQueue for AsyncQueue {
    fn query_info(&self) -> GpuBackendQueueInfo {
        GpuBackendQueueInfo::new(64)
    }

    fn submit(&self, _: &[u8]) -> Result<(), GpuBackendSubmitError> {
        panic!("async admission must not call synchronous submit")
    }

    fn async_capacity(&self) -> u32 {
        2
    }

    fn enqueue(&self, submission: GpuSubmission) -> Result<(), GpuBackendEnqueueError> {
        let mode = self.mode.load(Ordering::Relaxed);
        if mode == 1 {
            return Err(GpuBackendEnqueueError::Busy(submission));
        }
        if mode == 2 {
            return Err(GpuBackendEnqueueError::Rejected(
                GpuBackendSubmitError::Rejected("invalid test commands"),
                submission,
            ));
        }
        self.pending.lock().push_back(submission);
        if mode == 3 {
            Err(GpuBackendEnqueueError::Failed(
                GpuBackendSubmitError::Rejected("accepted prefix before invalid command"),
            ))
        } else {
            Ok(())
        }
    }
}

fn fixture() -> (GpuContext, GpuQueue, Arc<AsyncQueue>, Arc<IrqSpinLock<u32>>) {
    let drops = Arc::new(IrqSpinLock::new(0));
    let context = GpuContext::new(Arc::new(TestContext {
        drops: Arc::clone(&drops),
        buffer_detaches: Arc::new(IrqSpinLock::new(0)),
    }));
    let mut queue = context.create_queue().expect("test queue");
    let backend = Arc::new(AsyncQueue {
        pending: Mutex::new(VecDeque::new()),
        mode: AtomicU32::new(0),
    });
    queue.backend_queue = backend.clone();
    queue.async_command_limit = backend.query_info().max_opaque_command_size;
    (context, queue, backend, drops)
}

fn request(size: u32) -> GpuQueueSubmitAsync {
    GpuQueueSubmitAsync {
        abi_version: GPU_ABI_VERSION,
        result: 0,
        flags: 0,
        reserved: 0,
        command_ptr: 0x1000,
        command_size: size,
        completion_handle: 0,
        accepted: 0,
        reserved2: 0,
    }
}

fn submit(queue: &GpuQueue, handles: &HandleTable, commands: &[u8]) -> GpuQueueSubmitAsync {
    let mut response = None;
    queue
        .submit_async_request(
            request(commands.len() as u32),
            handles,
            |_, _| Ok(commands.to_vec()),
            |reply| {
                response = Some(*reply);
                Ok(())
            },
        )
        .expect("test response publication");
    response.expect("response must be published")
}

#[test_case]
fn accepted_work_is_pending_owned_and_bounded_without_observers() {
    let (context, queue, backend, context_drops) = fixture();
    let handles = HandleTable::new();
    let mut bytes = alloc::vec![1, 2, 3, 4];
    let first = submit(&queue, &handles, &bytes);
    bytes.fill(9);
    assert_eq!((first.result, first.accepted), (GPU_RESULT_SUCCESS, 1));
    assert_eq!(
        handles
            .get_metadata(first.completion_handle)
            .expect("metadata")
            .access_mode,
        AccessMode::ReadOnly
    );
    let observer = handles
        .get_arc_clone(first.completion_handle)
        .expect("observer");
    assert!(
        !observer
            .as_selectable()
            .expect("selectable")
            .current_ready(ReadyInterest::read())
            .read
    );
    assert_eq!(submit(&queue, &handles, &[5]).accepted, 1);
    assert_eq!(submit(&queue, &handles, &[]).result, GPU_RESULT_BUSY);
    handles.close_all();
    assert_eq!(submit(&queue, &handles, &[]).result, GPU_RESULT_BUSY);

    let retired = backend.take_next();
    assert_eq!(retired.commands(), &[1, 2, 3, 4]);
    retired.complete();
    assert!(
        observer
            .as_selectable()
            .expect("selectable")
            .current_ready(ReadyInterest::read())
            .read
    );
    let checkpoint = submit(&queue, &handles, &[]);
    assert_eq!(checkpoint.accepted, 1);
    drop((handles, observer, queue, context));
    assert_eq!(*context_drops.lock(), 0);
    backend.take_next().complete();
    let checkpoint = backend.take_next();
    assert!(checkpoint.commands().is_empty());
    checkpoint.complete();
    assert_eq!(*context_drops.lock(), 1);
}

#[test_case]
fn rejected_and_busy_backend_calls_return_their_admission_slots() {
    let (_context, queue, backend, _) = fixture();
    let handles = HandleTable::new();
    for mode in [1, 2] {
        backend.mode.store(mode, Ordering::Relaxed);
        for _ in 0..40 {
            let response = submit(&queue, &handles, &[1]);
            assert_eq!(response.accepted, 0);
            assert_eq!(response.completion_handle, 0);
            assert_eq!(
                response.result,
                if mode == 1 {
                    GPU_RESULT_BUSY
                } else {
                    GPU_RESULT_INVALID_ARGUMENT
                }
            );
            assert_eq!(handles.open_count(), 0);
        }
    }
    backend.mode.store(0, Ordering::Relaxed);
    assert_eq!(submit(&queue, &handles, &[1]).accepted, 1);
    backend.take_next().complete();
}

#[test_case]
fn failed_prefix_returns_an_observable_receipt_and_keeps_its_capacity() {
    let (_context, queue, backend, _) = fixture();
    let handles = HandleTable::new();
    backend.mode.store(3, Ordering::Relaxed);
    let failed = submit(&queue, &handles, &[1]);
    assert_eq!(failed.result, GPU_RESULT_INVALID_ARGUMENT);
    assert_eq!(failed.accepted, 1);
    let observer = handles
        .get_arc_clone(failed.completion_handle)
        .expect("failure receipt");
    assert!(
        !observer
            .as_selectable()
            .expect("selectable")
            .current_ready(ReadyInterest::read())
            .read
    );
    backend.mode.store(0, Ordering::Relaxed);
    assert_eq!(submit(&queue, &handles, &[2]).accepted, 1);
    assert_eq!(submit(&queue, &handles, &[3]).result, GPU_RESULT_BUSY);
    backend.take_next().complete();
    assert!(
        observer
            .as_selectable()
            .expect("selectable")
            .current_ready(ReadyInterest::read())
            .read
    );
    backend.take_next().complete();
}

#[test_case]
fn response_copy_failure_and_context_detach_do_not_release_in_flight_backing() {
    let (context, queue, backend, _) = fixture();
    let handles = HandleTable::new();
    let buffer_drops = Arc::new(IrqSpinLock::new(0));
    let buffer = GpuBuffer::new(
        Arc::new(TestBufferBackend {
            drops: buffer_drops.clone(),
        }),
        4096,
        0,
    )
    .expect("buffer");
    let backing = Arc::downgrade(&buffer.backing());
    context.attach_buffer(&buffer).expect("attach");
    assert_eq!(
        queue.submit_async_request(
            request(1),
            &handles,
            |_, _| Ok(alloc::vec![1]),
            |_| Err("response copy failed")
        ),
        Err("response copy failed")
    );
    assert_eq!(handles.open_count(), 0);
    context
        .detach_buffer(&buffer)
        .expect("detach after enqueue");
    drop((buffer, queue, context, handles));
    assert_eq!(*buffer_drops.lock(), 0);
    assert!(backing.upgrade().is_some());

    let mut pending = backend.take_next();
    pending.fail(GpuCompletionFailure::DeviceLost);
    assert!(backing.upgrade().is_some(), "failure is not quiescence");
    pending.retire_failed(GpuCompletionFailure::DeviceLost);
    assert_eq!(*buffer_drops.lock(), 1);
    assert!(backing.upgrade().is_none());
}

#[test_case]
fn async_submission_does_not_wait_for_attachment_locks() {
    let (_context, queue, backend, _) = fixture();
    let handles = HandleTable::new();
    let images = queue._attached_images.lock();
    assert_eq!(submit(&queue, &handles, &[1]).result, GPU_RESULT_BUSY);
    drop(images);
    let buffers = queue._attached_buffers.lock();
    assert_eq!(submit(&queue, &handles, &[1]).result, GPU_RESULT_BUSY);
    drop(buffers);
    assert_eq!(handles.open_count(), 0);
    assert_eq!(backend.pending.lock().len(), 0);
    assert_eq!(submit(&queue, &handles, &[1]).accepted, 1);
    backend.take_next().complete();
}

#[test_case]
fn malformed_async_requests_are_rejected_before_command_copy() {
    let (_context, queue, backend, _) = fixture();
    let handles = HandleTable::new();
    for field in 0..5 {
        let mut invalid = request(1);
        match field {
            0 => invalid.abi_version += 1,
            1 => invalid.flags = 1,
            2 => invalid.reserved = 1,
            3 => invalid.reserved2 = 1,
            _ => invalid.command_size = 65,
        }
        invalid.accepted = 1;
        invalid.completion_handle = 999;
        queue
            .submit_async_request(
                invalid,
                &handles,
                |_, _| panic!("must not copy"),
                |reply| {
                    assert_eq!(
                        reply.result,
                        if field == 0 {
                            GPU_RESULT_INVALID_ABI
                        } else {
                            GPU_RESULT_INVALID_ARGUMENT
                        }
                    );
                    assert_eq!(reply.accepted, 0);
                    assert_eq!(reply.completion_handle, 0);
                    Ok(())
                },
            )
            .expect("rejection response");
    }
    assert_eq!(backend.pending.lock().len(), 0);
    assert_eq!(handles.open_count(), 0);
}

#[test_case]
fn full_handle_table_prevents_acceptance_and_returns_capacity() {
    let (_context, queue, backend, _) = fixture();
    let handles = HandleTable::new();
    let (observer, _signal) = GpuCompletion::pair();
    let observer = Arc::new(observer);
    for _ in 0..HandleTable::MAX_HANDLES {
        handles
            .insert(KernelObject::Gpu(observer.clone()))
            .expect("fill handle table");
    }
    assert_eq!(
        submit(&queue, &handles, &[1]).result,
        GPU_RESULT_OUT_OF_RESOURCES
    );
    assert_eq!(backend.pending.lock().len(), 0);
    handles.remove(0);
    assert_eq!(submit(&queue, &handles, &[1]).accepted, 1);
    backend.take_next().complete();
}

#[test_case]
fn synchronous_backend_does_not_advertise_or_fallback_to_async() {
    let (context, _, _, _) = fixture();
    let queue = context.create_queue().expect("synchronous queue");
    let mut info = GpuQueueAsyncInfo::new();
    queue.fill_async_info(&mut info);
    assert_eq!(info.result, GPU_RESULT_SUCCESS);
    assert_eq!(info.max_pending_submissions, 0);
    assert_eq!(info.max_opaque_command_size, 0);
    assert_eq!(
        submit(&queue, &HandleTable::new(), &[1]).result,
        GPU_RESULT_UNSUPPORTED
    );
}

#[test_case]
fn dropping_unretired_work_quarantines_ownership_and_capacity() {
    let (context, queue, backend, context_drops) = fixture();
    let handles = HandleTable::new();
    let response = submit(&queue, &handles, &[1]);
    let observer = handles
        .get_arc_clone(response.completion_handle)
        .expect("observer");
    // Deliberately exercise the fail-safe. One request's small allocation,
    // context/queue references, and slot remain quarantined for this test boot.
    drop(backend.take_next());
    assert!(
        observer
            .as_selectable()
            .expect("selectable")
            .current_ready(ReadyInterest::read())
            .read
    );
    assert_eq!(submit(&queue, &handles, &[2]).accepted, 1);
    assert_eq!(submit(&queue, &handles, &[3]).result, GPU_RESULT_BUSY);
    backend.take_next().complete();
    drop((handles, observer, queue, context));
    assert_eq!(
        *context_drops.lock(),
        0,
        "quarantine retains device authority"
    );
}

#[test_case]
fn async_queue_info_reports_limits_and_rejects_reserved_fields() {
    let (_context, queue, _backend, _) = fixture();
    let mut info = GpuQueueAsyncInfo::new();
    queue.fill_async_info(&mut info);
    assert_eq!(info.max_pending_submissions, 2);
    assert_eq!(info.max_opaque_command_size, 64);
    info.reserved = 1;
    queue.fill_async_info(&mut info);
    assert_eq!(info.result, GPU_RESULT_INVALID_ARGUMENT);
    assert_eq!(info.reserved, 0);
    info.reserved2 = 1;
    queue.fill_async_info(&mut info);
    assert_eq!(info.result, GPU_RESULT_INVALID_ARGUMENT);
    assert_eq!(info.reserved2, 0);
    info.abi_version += 1;
    queue.fill_async_info(&mut info);
    assert_eq!(info.result, GPU_RESULT_INVALID_ABI);
}
