//! Bounded, owned VirtIO GPU control requests with descriptor-keyed responses.
//!
//! Submission only publishes descriptors. Any driver execution context can
//! reap responses; a caller never owns the sole reference to live DMA storage.
//! Transport completion is not, by itself, proof of GPU execution retirement:
//! execution callers must also validate the returned fence.

use alloc::sync::Arc;
use core::mem::ManuallyDrop;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::drivers::virtio::queue::{DescriptorFlag, VirtQueue};
use crate::mem::page::ContiguousPages;
use crate::sync::IrqSpinLock;

use super::{
    VIRTIO_GPU_CONTROL_QUEUE_SIZE, VIRTIO_GPU_CONTROL_TIMEOUT_NS, VirtioGpuCtrlHdr,
    validate_execution_response,
};

pub(super) const CONTROL_TIMEOUT: &str = "VirtIO GPU control queue timed out";
const INVALID_RESPONSE: &str = "VirtIO GPU control queue returned an invalid response";
const INVALID_FENCE: &str = "VirtIO GPU retirement checkpoint did not return its fence";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ControlStatus {
    Pending,
    Complete(u32),
    Failed(&'static str),
}

pub(super) struct ControlRequest {
    command: ContiguousPages,
    response: ContiguousPages,
    command_len: u32,
    response_len: u32,
    expected_fence: Option<u64>,
    published: AtomicBool,
    status: IrqSpinLock<ControlStatus>,
}

impl ControlRequest {
    pub(super) fn new(commands: &[u8], response_len: usize) -> Result<Arc<Self>, &'static str> {
        Self::allocate(commands, response_len, None)
    }

    pub(super) fn new_checkpoint(commands: &[u8], fence: u64) -> Result<Arc<Self>, &'static str> {
        Self::allocate(
            commands,
            core::mem::size_of::<VirtioGpuCtrlHdr>(),
            Some(fence),
        )
    }

    fn allocate(
        commands: &[u8],
        response_len: usize,
        expected_fence: Option<u64>,
    ) -> Result<Arc<Self>, &'static str> {
        if commands.is_empty() || response_len < core::mem::size_of::<VirtioGpuCtrlHdr>() {
            return Err("VirtIO GPU control buffers must include a command and response header");
        }
        let command_len = u32::try_from(commands.len())
            .map_err(|_| "VirtIO GPU command buffer exceeds the descriptor limit")?;
        let response_len = u32::try_from(response_len)
            .map_err(|_| "VirtIO GPU response buffer exceeds the descriptor limit")?;
        let page_size = crate::environment::PAGE_SIZE;
        let command = ContiguousPages::new(commands.len().div_ceil(page_size))
            .ok_or("Failed to allocate VirtIO GPU command DMA buffer")?;
        let response = ContiguousPages::new((response_len as usize).div_ceil(page_size))
            .ok_or("Failed to allocate VirtIO GPU response DMA buffer")?;
        // SAFETY: both private allocations cover their respective byte lengths.
        // They are not visible to the device until enqueue publishes the chain.
        unsafe {
            core::ptr::copy_nonoverlapping(
                commands.as_ptr(),
                command.as_ptr() as *mut u8,
                commands.len(),
            );
            core::ptr::write_bytes(response.as_ptr() as *mut u8, 0, response_len as usize);
        }
        Ok(Arc::new(Self {
            command,
            response,
            command_len,
            response_len,
            expected_fence,
            published: AtomicBool::new(false),
            status: IrqSpinLock::new(ControlStatus::Pending),
        }))
    }

    pub(super) fn status(&self) -> ControlStatus {
        *self.status.lock()
    }

    pub(super) fn copy_response(&self, destination: &mut [u8]) -> Result<u32, &'static str> {
        let written = match self.status() {
            ControlStatus::Pending => return Err("VirtIO GPU response is still pending"),
            ControlStatus::Failed(error) => return Err(error),
            ControlStatus::Complete(written) => written,
        };
        if destination.len() > self.response_len as usize {
            return Err("VirtIO GPU response destination exceeds its allocation");
        }
        // SAFETY: Complete is published only after a matching used entry and
        // the DMA read barrier. The device has returned this allocation, and
        // the Arc keeps it alive for this copy. Unwritten trailing bytes were
        // initialized to zero before publication, as in the legacy interface.
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.response.as_ptr() as *const u8,
                destination.as_mut_ptr(),
                destination.len(),
            );
        }
        Ok(written)
    }
}

struct PendingControl {
    request: Arc<ControlRequest>,
    response_desc: usize,
    submitted_ns: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ControlEnqueueError {
    Busy,
    Failed(&'static str),
}

pub(super) struct ControlQueue {
    pub(super) ring: ManuallyDrop<VirtQueue<'static>>,
    pending: [Option<PendingControl>; VIRTIO_GPU_CONTROL_QUEUE_SIZE],
    failed: Option<&'static str>,
    device_owned: bool,
}

impl ControlQueue {
    #[cfg(test)]
    pub(super) fn test_respond(&mut self, head: usize, header: VirtioGpuCtrlHdr) {
        let request = &self.pending[head]
            .as_ref()
            .expect("test device-owned request")
            .request;
        // SAFETY: the fake device owns this live response DMA allocation. The
        // test publishes the used entry only after writing the complete header.
        unsafe {
            core::ptr::write_unaligned(request.response.as_ptr() as *mut VirtioGpuCtrlHdr, header)
        };
        let index = *self.ring.used.idx as usize % VIRTIO_GPU_CONTROL_QUEUE_SIZE;
        self.ring.used.ring[index].id = head as u32;
        self.ring.used.ring[index].len = core::mem::size_of::<VirtioGpuCtrlHdr>() as u32;
        *self.ring.used.idx = self.ring.used.idx.wrapping_add(1);
    }

    pub(super) fn new() -> Self {
        let mut ring = VirtQueue::new(VIRTIO_GPU_CONTROL_QUEUE_SIZE);
        ring.init();
        Self {
            ring: ManuallyDrop::new(ring),
            pending: core::array::from_fn(|_| None),
            failed: None,
            device_owned: false,
        }
    }

    pub(super) fn mark_device_owned(&mut self) {
        self.device_owned = true;
    }

    pub(super) fn check(&self) -> Result<(), &'static str> {
        self.failed.map_or(Ok(()), Err)
    }

    pub(super) fn has_pending(&self) -> bool {
        self.pending.iter().any(Option::is_some)
    }

    pub(super) fn enqueue(
        &mut self,
        request: &Arc<ControlRequest>,
        now_ns: u64,
    ) -> Result<(), ControlEnqueueError> {
        self.enqueue_batch(core::slice::from_ref(request), now_ns)
    }

    /// Publish a command and its independent retirement checkpoint together.
    /// The two-chain limit keeps pre-publication rollback bounded and leaves
    /// no possibly accepted prefix when admission returns an error.
    pub(super) fn enqueue_batch(
        &mut self,
        requests: &[Arc<ControlRequest>],
        now_ns: u64,
    ) -> Result<(), ControlEnqueueError> {
        self.check().map_err(ControlEnqueueError::Failed)?;
        if requests.is_empty() || requests.len() > 2 {
            return Err(ControlEnqueueError::Failed(
                "Invalid VirtIO GPU control batch size",
            ));
        }
        if self.ring.free_descriptors.len() < requests.len() * 2 {
            return Err(ControlEnqueueError::Busy);
        }
        for (index, request) in requests.iter().enumerate() {
            if request.published.swap(true, Ordering::AcqRel) {
                for claimed in &requests[..index] {
                    claimed.published.store(false, Ordering::Release);
                }
                return Err(ControlEnqueueError::Failed(
                    "VirtIO GPU control request was already published",
                ));
            }
        }
        let mut heads = [0; 2];
        for (index, request) in requests.iter().enumerate() {
            let command_desc = self.ring.alloc_desc().expect("reserved command descriptor");
            let response_desc = self
                .ring
                .alloc_desc()
                .expect("reserved response descriptor");
            heads[index] = command_desc;
            let command = &mut self.ring.desc[command_desc];
            command.addr = request.command.as_paddr() as u64;
            command.len = request.command_len;
            command.flags = DescriptorFlag::Next as u16;
            command.next = response_desc as u16;
            let response = &mut self.ring.desc[response_desc];
            response.addr = request.response.as_paddr() as u64;
            response.len = request.response_len;
            response.flags = DescriptorFlag::Write as u16;
            response.next = 0;
            self.pending[command_desc] = Some(PendingControl {
                request: Arc::clone(request),
                response_desc,
                submitted_ns: now_ns,
            });
        }
        // One available-index update publishes the whole pair in order.
        if let Err(error) = self.ring.push_many(&heads[..requests.len()]) {
            for head in &heads[..requests.len()] {
                let pending = self.pending[*head].take().expect("unpublished request");
                pending.request.published.store(false, Ordering::Release);
                self.ring.free_desc(pending.response_desc);
                self.ring.free_desc(*head);
            }
            return Err(ControlEnqueueError::Failed(error));
        }
        Ok(())
    }

    pub(super) fn reap(&mut self, now_ns: u64) -> Result<(), &'static str> {
        self.check()?;
        // Bound the loop even if a broken device keeps advancing used.idx.
        for _ in 0..VIRTIO_GPU_CONTROL_QUEUE_SIZE {
            let Some((head, written)) = self.ring.pop_used() else {
                break;
            };
            // Order the device-written entry and response after observing idx.
            crate::arch::io_mb();
            let Some(Some(pending)) = self.pending.get(head) else {
                return self.fail(INVALID_RESPONSE);
            };
            if written < core::mem::size_of::<VirtioGpuCtrlHdr>() as u32
                || written > pending.request.response_len
            {
                return self.fail(INVALID_RESPONSE);
            }
            if let Some(fence) = pending.request.expected_fence {
                // SAFETY: the used length covers the header, its DMA storage is
                // live, and the I/O read barrier above follows used.idx.
                let response = unsafe {
                    core::ptr::read_unaligned(
                        pending.request.response.as_ptr() as *const VirtioGpuCtrlHdr
                    )
                };
                if validate_execution_response(response, Some(fence)).is_err() {
                    return self.fail(INVALID_FENCE);
                }
            }
            let pending = self.pending[head]
                .take()
                .expect("validated live control head");
            self.ring.free_desc(pending.response_desc);
            self.ring.free_desc(head);
            *pending.request.status.lock() = ControlStatus::Complete(written);
        }
        if self.pending.iter().flatten().any(|pending| {
            now_ns.saturating_sub(pending.submitted_ns) >= VIRTIO_GPU_CONTROL_TIMEOUT_NS
        }) {
            return self.fail(CONTROL_TIMEOUT);
        }
        Ok(())
    }

    pub(super) fn fail(&mut self, error: &'static str) -> Result<(), &'static str> {
        let error = *self.failed.get_or_insert(error);
        for pending in self.pending.iter().flatten() {
            *pending.request.status.lock() = ControlStatus::Failed(error);
        }
        // No reset/quiescence proof exists. Leave DMA and descriptors reserved.
        Err(error)
    }
}

impl Drop for ControlQueue {
    fn drop(&mut self) {
        let had_pending = self.has_pending();
        for pending in self.pending.iter_mut().filter_map(Option::take) {
            if pending.request.status() == ControlStatus::Pending {
                *pending.request.status.lock() =
                    ControlStatus::Failed("VirtIO GPU control queue was abandoned");
            }
            core::mem::forget(pending);
        }
        if !self.device_owned && !had_pending {
            // SAFETY: this ring was never installed on hardware and no request
            // is outstanding. ManuallyDrop is released exactly once here.
            unsafe { ManuallyDrop::drop(&mut self.ring) };
        }
        // Installed rings require device reset before deallocation. There is
        // no reset path yet; quarantine the ring as well as any outstanding DMA.
    }
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;
    use core::sync::atomic::Ordering;

    use super::{
        CONTROL_TIMEOUT, ControlEnqueueError, ControlQueue, ControlRequest, ControlStatus,
        INVALID_RESPONSE, VIRTIO_GPU_CONTROL_QUEUE_SIZE, VIRTIO_GPU_CONTROL_TIMEOUT_NS,
    };

    fn request(value: u8) -> Arc<ControlRequest> {
        ControlRequest::new(&[value; 32], 24).expect("test DMA allocation")
    }

    fn last_head(queue: &ControlQueue) -> usize {
        let index = queue.ring.avail.idx.wrapping_sub(1) as usize % VIRTIO_GPU_CONTROL_QUEUE_SIZE;
        queue.ring.avail.ring[index] as usize
    }

    fn respond(queue: &mut ControlQueue, head: usize, written: u32, value: u8) {
        if let Some(Some(pending)) = queue.pending.get(head) {
            // SAFETY: fake device owns this live response allocation, and the
            // test holds exclusive transport access before publishing used.idx.
            unsafe {
                core::ptr::write_bytes(
                    pending.request.response.as_ptr() as *mut u8,
                    value,
                    pending.request.response_len as usize,
                );
            }
        }
        let index = *queue.ring.used.idx as usize % VIRTIO_GPU_CONTROL_QUEUE_SIZE;
        queue.ring.used.ring[index].id = head as u32;
        queue.ring.used.ring[index].len = written;
        *queue.ring.used.idx = queue.ring.used.idx.wrapping_add(1);
    }

    #[test_case]
    fn control_routes_out_of_order_responses_and_retains_dropped_callers() {
        let mut queue = ControlQueue::new();
        let first = request(1);
        let second = request(2);
        queue.enqueue(&first, 0).expect("first enqueue");
        let first_head = last_head(&queue);
        queue.enqueue(&second, 0).expect("second enqueue");
        let second_head = last_head(&queue);
        let weak_first = Arc::downgrade(&first);
        drop(first);
        assert!(weak_first.upgrade().is_some());
        respond(&mut queue, second_head, 24, 9);
        queue.reap(1).expect("second response");
        let mut response = [0; 24];
        assert_eq!(second.copy_response(&mut response), Ok(24));
        assert_eq!(response, [9; 24]);
        assert_eq!(
            weak_first.upgrade().expect("still owned").status(),
            ControlStatus::Pending
        );
        respond(&mut queue, first_head, 24, 8);
        queue.reap(2).expect("first response");
        assert!(weak_first.upgrade().is_none());
        assert!(!queue.has_pending());
        assert_eq!(
            queue.ring.free_descriptors.len(),
            VIRTIO_GPU_CONTROL_QUEUE_SIZE
        );
    }

    #[test_case]
    fn control_capacity_is_bounded_and_uses_owned_command_bytes() {
        let mut queue = ControlQueue::new();
        let mut bytes = [7; 32];
        let owned = ControlRequest::new(&bytes, 24).expect("owned command");
        bytes.fill(3);
        // SAFETY: request is not published, and its command allocation is live.
        assert_eq!(unsafe { *(owned.command.as_ptr() as *const u8) }, 7);
        for _ in 0..VIRTIO_GPU_CONTROL_QUEUE_SIZE / 2 {
            queue.enqueue(&request(1), 0).expect("available capacity");
        }
        assert_eq!(queue.enqueue(&owned, 0), Err(ControlEnqueueError::Busy));
        assert_eq!(owned.status(), ControlStatus::Pending);
        let heads: alloc::vec::Vec<_> = queue
            .pending
            .iter()
            .enumerate()
            .filter_map(|(index, pending)| pending.as_ref().map(|_| index))
            .collect();
        for head in heads {
            respond(&mut queue, head, 24, 0);
        }
        queue.reap(1).expect("all returned");
        assert_eq!(
            queue.ring.free_descriptors.len(),
            VIRTIO_GPU_CONTROL_QUEUE_SIZE
        );
    }

    #[test_case]
    fn control_timeout_quarantines_dma_even_after_queue_drop() {
        let mut queue = ControlQueue::new();
        let owned = request(1);
        queue.enqueue(&owned, 5).expect("enqueue");
        assert_eq!(queue.reap(VIRTIO_GPU_CONTROL_TIMEOUT_NS + 4), Ok(()));
        assert_eq!(
            queue.reap(VIRTIO_GPU_CONTROL_TIMEOUT_NS + 5),
            Err(CONTROL_TIMEOUT)
        );
        assert_eq!(owned.status(), ControlStatus::Failed(CONTROL_TIMEOUT));
        assert_eq!(
            queue.enqueue(&request(2), 0),
            Err(ControlEnqueueError::Failed(CONTROL_TIMEOUT))
        );
        let weak = Arc::downgrade(&owned);
        drop((owned, queue));
        assert!(weak.upgrade().is_some());
    }

    #[test_case]
    fn control_batch_pressure_and_duplicate_requests_never_publish_a_prefix() {
        let mut queue = ControlQueue::new();
        for _ in 0..VIRTIO_GPU_CONTROL_QUEUE_SIZE / 2 - 1 {
            queue
                .enqueue(&request(1), 0)
                .expect("fill all but one chain");
        }
        let first = request(2);
        let second = request(3);
        let available = *queue.ring.avail.idx;
        assert_eq!(
            queue.enqueue_batch(&[first.clone(), second.clone()], 0),
            Err(ControlEnqueueError::Busy)
        );
        assert_eq!(*queue.ring.avail.idx, available);
        assert!(!first.published.load(Ordering::Acquire));
        assert!(!second.published.load(Ordering::Acquire));
        let heads: alloc::vec::Vec<_> = queue
            .pending
            .iter()
            .enumerate()
            .filter_map(|(head, pending)| pending.as_ref().map(|_| head))
            .collect();
        for head in heads {
            respond(&mut queue, head, 24, 0);
        }
        queue.reap(1).expect("retire preceding requests");
        assert!(
            queue
                .enqueue_batch(&[first.clone(), first.clone()], 2)
                .is_err()
        );
        assert_eq!(*queue.ring.avail.idx, available);
        assert!(!first.published.load(Ordering::Acquire));
        queue
            .enqueue_batch(&[first.clone(), second.clone()], 2)
            .expect("whole pair fits");
        assert_eq!(*queue.ring.avail.idx, available + 2);
        assert!(
            queue.enqueue(&first, 2).is_err(),
            "a live request cannot be republished"
        );
        let first_head = queue.ring.avail.ring[available as usize] as usize;
        let second_head = queue.ring.avail.ring[available as usize + 1] as usize;
        respond(&mut queue, first_head, 24, 0);
        respond(&mut queue, second_head, 24, 0);
        queue.reap(3).expect("retire pair");
        assert!(
            queue.enqueue(&first, 4).is_err(),
            "a returned receipt cannot be reused as a new request"
        );
    }

    #[test_case]
    fn control_invalid_used_entries_do_not_free_live_dma() {
        for (wrong_head, written) in [(true, 24), (false, 23), (false, 25)] {
            let mut queue = ControlQueue::new();
            let owned = request(1);
            queue.enqueue(&owned, 0).expect("enqueue");
            let head = if wrong_head {
                VIRTIO_GPU_CONTROL_QUEUE_SIZE
            } else {
                last_head(&queue)
            };
            respond(&mut queue, head, written, 0);
            assert_eq!(queue.reap(1), Err(INVALID_RESPONSE));
            assert_eq!(owned.status(), ControlStatus::Failed(INVALID_RESPONSE));
            assert_eq!(
                queue.ring.free_descriptors.len(),
                VIRTIO_GPU_CONTROL_QUEUE_SIZE - 2
            );
        }
    }
}
