//! Multiple imported views may share a physical image during resize retirement.

use super::{TestImageBackend, TestQueue};
use crate::device::gpu::{
    GpuBackendContext, GpuBackendContextInfo, GpuBackendImage, GpuBackendQueue, GpuContext,
    GpuImage, GpuImageCreateInfo,
};
use crate::sync::IrqSpinLock;
use alloc::sync::Arc;

#[derive(Default)]
struct AttachmentState {
    attach_calls: usize,
    detach_calls: usize,
    attached: bool,
    fail_attach: bool,
    fail_detach: bool,
}

struct AttachmentContext(Arc<IrqSpinLock<AttachmentState>>);

impl GpuBackendContext for AttachmentContext {
    fn query_info(&self) -> GpuBackendContextInfo {
        GpuBackendContextInfo::new(7, 0x1234)
    }

    fn create_queue(&self) -> Result<Arc<dyn GpuBackendQueue>, &'static str> {
        Ok(Arc::new(TestQueue))
    }

    fn attach_image(&self, image: &dyn GpuBackendImage) -> Result<u64, &'static str> {
        let mut state = self.0.lock();
        state.attach_calls += 1;
        // Different image objects deliberately report the same resource token.
        // Such aliases must still reach backend identity/ownership validation.
        assert_eq!(image.query_info().command_resource_token, 9);
        if state.fail_attach || state.attached || image.backend_cookie() != 1 {
            return Err("backend rejected image");
        }
        state.attached = true;
        Ok(19)
    }

    fn detach_image(&self, _: &dyn GpuBackendImage) -> Result<(), &'static str> {
        let mut state = self.0.lock();
        state.detach_calls += 1;
        if state.fail_detach {
            return Err("backend detach failed");
        }
        assert!(state.attached);
        state.attached = false;
        Ok(())
    }
}

fn image(drops: &Arc<IrqSpinLock<u32>>) -> Arc<GpuImage> {
    Arc::new(
        GpuImage::new(
            Arc::new(TestImageBackend {
                drops: Arc::clone(drops),
            }),
            GpuImageCreateInfo::new(1, 3, 8, 8),
        )
        .unwrap(),
    )
}

fn context() -> (GpuContext, Arc<IrqSpinLock<AttachmentState>>) {
    let state = Arc::new(IrqSpinLock::new(AttachmentState::default()));
    (
        GpuContext::new(Arc::new(AttachmentContext(Arc::clone(&state)))),
        state,
    )
}

#[test_case]
fn repeated_and_duplicated_image_capabilities_detach_only_last_reference() {
    let drops = Arc::new(IrqSpinLock::new(0));
    let image = image(&drops);
    // Handle duplication retains this same capability object, not a new image.
    let duplicate = Arc::clone(&image);
    let (context, state) = context();
    assert_eq!(context.attach_image(&image), Ok(19));
    assert_eq!(context.attach_image(&image), Ok(19));
    assert_eq!(context.attach_image(&duplicate), Ok(19));
    assert_eq!(state.lock().attach_calls, 1);
    assert_eq!(context.detach_image(&duplicate), Ok(()));
    assert_eq!(context.detach_image(&image), Ok(()));
    assert_eq!(state.lock().detach_calls, 0);
    assert!(context.image_is_attached(&image));
    assert_eq!(context.detach_image(&image), Ok(()));
    assert_eq!(state.lock().detach_calls, 1);
    assert!(!context.image_is_attached(&image));
    assert!(context.detach_image(&duplicate).is_err());
    assert_eq!(state.lock().detach_calls, 1);
    assert_eq!(context.attach_image(&duplicate), Ok(19));
    assert_eq!(state.lock().attach_calls, 2);
}

#[test_case]
fn different_images_with_same_resource_token_are_not_deduplicated() {
    let drops = Arc::new(IrqSpinLock::new(0));
    let first = image(&drops);
    let second = image(&drops);
    let (context, state) = context();
    assert_eq!(context.attach_image(&first), Ok(19));
    assert!(context.attach_image(&second).is_err());
    assert_eq!(state.lock().attach_calls, 2);
    assert_eq!(context.attached_images.lock().len(), 1);
    assert_eq!(context.attached_images.lock()[0].attachment_count, 1);
    assert!(!context.image_is_attached(&second));
}

#[test_case]
fn failed_first_attach_and_failed_final_detach_preserve_reference_accounting() {
    let drops = Arc::new(IrqSpinLock::new(0));
    let image = image(&drops);
    let (context, state) = context();
    state.lock().fail_attach = true;
    assert!(context.attach_image(&image).is_err());
    assert!(context.attached_images.lock().is_empty());
    state.lock().fail_attach = false;
    assert_eq!(context.attach_image(&image), Ok(19));
    state.lock().fail_detach = true;
    assert!(context.detach_image(&image).is_err());
    assert_eq!(context.attached_images.lock()[0].attachment_count, 1);
    assert!(context.image_is_attached(&image));
    state.lock().fail_detach = false;
    assert_eq!(context.detach_image(&image), Ok(()));
    assert!(!context.image_is_attached(&image));
}

#[test_case]
fn image_attachment_count_overflow_is_rejected_without_backend_mutation() {
    let drops = Arc::new(IrqSpinLock::new(0));
    let image = image(&drops);
    let (context, state) = context();
    assert_eq!(context.attach_image(&image), Ok(19));
    context.attached_images.lock()[0].attachment_count = usize::MAX;
    assert!(context.attach_image(&image).is_err());
    assert_eq!(
        context.attached_images.lock()[0].attachment_count,
        usize::MAX
    );
    assert_eq!(state.lock().attach_calls, 1);
    context.attached_images.lock()[0].attachment_count = 1;
    assert_eq!(context.detach_image(&image), Ok(()));
}

#[test_case]
fn aliased_image_and_backing_survive_until_derived_queue_retires() {
    let drops = Arc::new(IrqSpinLock::new(0));
    let image = image(&drops);
    let (context, state) = context();
    let queue = context.create_queue().unwrap();
    let backing = Arc::downgrade(&image.backing());
    assert_eq!(context.attach_image(&image), Ok(19));
    assert_eq!(context.attach_image(&image), Ok(19));
    drop(image);
    drop(context);
    assert_eq!(*drops.lock(), 0);
    assert!(backing.upgrade().is_some());
    assert_eq!(queue._attached_images.lock().len(), 1);
    assert_eq!(state.lock().attach_calls, 1);
    drop(queue);
    assert_eq!(*drops.lock(), 1);
    assert!(backing.upgrade().is_none());
}

#[test_case]
fn same_image_in_different_contexts_has_independent_attachment_references() {
    let drops = Arc::new(IrqSpinLock::new(0));
    let image = image(&drops);
    let (first, first_state) = context();
    let (second, second_state) = context();
    assert_eq!(first.attach_image(&image), Ok(19));
    assert_eq!(second.attach_image(&image), Ok(19));
    assert_eq!(first.detach_image(&image), Ok(()));
    assert!(!first_state.lock().attached);
    assert!(second_state.lock().attached);
    assert_eq!(second.detach_image(&image), Ok(()));
}

#[test_case]
fn failed_attachment_response_rolls_back_only_its_new_reference() {
    let drops = Arc::new(IrqSpinLock::new(0));
    let image = image(&drops);
    let (context, state) = context();
    assert!(
        context
            .attach_image_and_publish(&image, |result| {
                assert_eq!(result, Ok(19));
                Err("response page disappeared")
            })
            .is_err()
    );
    assert!(!context.image_is_attached(&image));
    assert_eq!(state.lock().detach_calls, 1);
    assert_eq!(context.attach_image(&image), Ok(19));
    assert!(
        context
            .attach_image_and_publish(&image, |result| {
                assert_eq!(result, Ok(19));
                Err("response page disappeared")
            })
            .is_err()
    );
    assert_eq!(context.attached_images.lock()[0].attachment_count, 1);
    assert_eq!(state.lock().detach_calls, 1);
    assert_eq!(context.detach_image(&image), Ok(()));
    assert_eq!(state.lock().detach_calls, 2);
}

#[test_case]
fn failed_response_and_failed_backend_rollback_retain_the_image() {
    let drops = Arc::new(IrqSpinLock::new(0));
    let image = image(&drops);
    let (context, state) = context();
    state.lock().fail_detach = true;
    assert!(
        context
            .attach_image_and_publish(&image, |_| Err("response failed"))
            .is_err()
    );
    assert!(context.image_is_attached(&image));
    assert_eq!(context.attached_images.lock()[0].attachment_count, 1);
    state.lock().fail_detach = false;
    assert_eq!(context.detach_image(&image), Ok(()));
}
