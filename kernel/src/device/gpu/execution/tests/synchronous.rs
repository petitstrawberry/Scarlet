//! Legacy submit keeps attachment authority and backing until every return.

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

use super::{TestBufferBackend, TestContext, TestImageBackend};
use crate::device::gpu::execution::{GpuAttachedBuffer, GpuAttachedImage};
use crate::device::gpu::resource::{GpuBufferBacking, GpuImageBacking};
use crate::device::gpu::{
    GPU_IMAGE_FORMAT_BGRA8_UNORM, GPU_IMAGE_USAGE_SAMPLED, GpuBackendQueue, GpuBackendQueueInfo,
    GpuBackendSubmitError, GpuBuffer, GpuContext, GpuImage, GpuImageCreateInfo,
};
use crate::sync::{IrqSpinLock, Mutex};

struct RetentionCheckingQueue {
    images: Weak<Mutex<Vec<GpuAttachedImage>>>,
    buffers: Weak<Mutex<Vec<GpuAttachedBuffer>>>,
    image_backing: Weak<GpuImageBacking>,
    buffer_backing: Weak<GpuBufferBacking>,
    fail: bool,
}

impl GpuBackendQueue for RetentionCheckingQueue {
    fn query_info(&self) -> GpuBackendQueueInfo {
        GpuBackendQueueInfo::new(4)
    }

    fn submit(&self, _: &[u8]) -> Result<(), GpuBackendSubmitError> {
        assert!(self.image_backing.upgrade().is_some());
        assert!(self.buffer_backing.upgrade().is_some());
        // A concurrent detach must not remove either attachment while the
        // backend releases its own locks to wait for completion.
        assert!(self.images.upgrade().unwrap().try_lock().is_none());
        assert!(self.buffers.upgrade().unwrap().try_lock().is_none());
        if self.fail {
            // Model a fault whose hardware accesses have already quiesced.
            Err(GpuBackendSubmitError::DeviceLost("test GPU stopped"))
        } else {
            Ok(())
        }
    }
}

fn verify_retention(fail: bool) {
    let drops = Arc::new(IrqSpinLock::new(0));
    let context = GpuContext::new(Arc::new(TestContext {
        drops: Arc::clone(&drops),
        buffer_detaches: Arc::new(IrqSpinLock::new(0)),
    }));
    let image = GpuImage::new(
        Arc::new(TestImageBackend {
            drops: Arc::clone(&drops),
        }),
        GpuImageCreateInfo::new(GPU_IMAGE_FORMAT_BGRA8_UNORM, GPU_IMAGE_USAGE_SAMPLED, 2, 2),
    )
    .unwrap();
    let buffer = GpuBuffer::new(
        Arc::new(TestBufferBackend {
            drops: Arc::clone(&drops),
        }),
        4096,
        0,
    )
    .unwrap();
    context.attach_image(&image).unwrap();
    context.attach_buffer(&buffer).unwrap();
    let mut queue = context.create_queue().unwrap();
    let backend = Arc::new(RetentionCheckingQueue {
        images: Arc::downgrade(&context.attached_images),
        buffers: Arc::downgrade(&context.attached_buffers),
        image_backing: Arc::downgrade(&image.backing()),
        buffer_backing: Arc::downgrade(&buffer.backing()),
        fail,
    });
    queue.backend_queue = backend.clone();
    drop(image);
    drop(buffer);
    drop(context);

    assert_eq!(queue.submit_with_retained_attachments(&[1]).is_err(), fail);

    // Both success and quiesced errors release the locks. Closing the final
    // attachments after return must then release their real page allocations.
    queue._attached_images.try_lock().unwrap().clear();
    queue._attached_buffers.try_lock().unwrap().clear();
    assert!(backend.image_backing.upgrade().is_none());
    assert!(backend.buffer_backing.upgrade().is_none());
}

#[test_case]
fn synchronous_submit_retains_image_and_buffer_until_success() {
    verify_retention(false);
}

#[test_case]
fn synchronous_submit_releases_attachments_after_quiesced_error() {
    verify_retention(true);
}
