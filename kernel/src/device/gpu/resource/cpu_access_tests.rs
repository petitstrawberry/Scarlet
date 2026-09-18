//! Exercise real private backing and usercopy against a recording backend.

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicBool, Ordering};

use super::{GpuImage, image_readback_layout, image_upload_layout};
use crate::device::gpu::{
    GPU_IMAGE_FORMAT_BGRA8_UNORM, GPU_IMAGE_USAGE_TRANSFER_DST, GPU_IMAGE_USAGE_TRANSFER_SRC,
    GpuBackend, GpuBackendContext, GpuBackendContextInfo, GpuBackendCpuAccessGuard,
    GpuBackendImage, GpuBackendImageInfo, GpuBackendInfo, GpuBackendQueue,
    GpuContextReadbackImageBgra, GpuContextUploadImageBgra, GpuDeviceInfo, GpuDeviceState,
    GpuImageBackingInfo, GpuImageCreateInfo, GpuImageUploadInfo,
};
use crate::device::graphics::GpuDisplayResource;
use crate::library::std::usercopy::{copy_from_user, copy_to_user};
use crate::sync::Mutex;
use crate::vm::addr::phys_to_virt;

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Retired,
    Rejected,
    Uploaded,
    Readback,
    Released,
}

struct RecordingBackend {
    backing: Mutex<Option<GpuImageBackingInfo>>,
    events: Mutex<Vec<Event>>,
    reserved: AtomicBool,
    reject: bool,
    fail_transfer: bool,
}

impl RecordingBackend {
    fn new(reject: bool, fail_transfer: bool) -> Self {
        Self {
            backing: Mutex::new(None),
            events: Mutex::new(Vec::new()),
            reserved: AtomicBool::new(false),
            reject,
            fail_transfer,
        }
    }

    fn pixels(&self) -> [u8; 16] {
        let backing = self.backing.lock();
        let backing = backing.as_ref().expect("image backing is recorded");
        assert!(backing.is_physically_contiguous());
        // SAFETY: The test keeps its GpuImage alive and the test backend never
        // starts concurrent hardware access to this allocation.
        unsafe { (phys_to_virt(backing.paddr) as *const [u8; 16]).read() }
    }

    fn set_pixels(&self, value: u8) {
        let backing = self.backing.lock();
        let backing = backing.as_ref().expect("image backing is recorded");
        // SAFETY: The test owns this live allocation with no hardware users.
        unsafe { (phys_to_virt(backing.paddr) as *mut [u8; 16]).write([value; 16]) };
    }
}

struct RecordingImage(GpuBackendImageInfo);

impl GpuBackendImage for RecordingImage {
    fn query_info(&self) -> GpuBackendImageInfo {
        self.0
    }

    fn backend_cookie(&self) -> u64 {
        1
    }

    fn display_resource(&self) -> Option<GpuDisplayResource> {
        None
    }
}

impl GpuBackend for RecordingBackend {
    fn query_info(&self) -> GpuBackendInfo {
        GpuBackendInfo::new(
            GpuDeviceInfo::new(GpuDeviceState::Ready, 0, 0),
            0,
            b"cpu-access-test",
            &[],
        )
    }

    fn create_image(
        &self,
        create: GpuImageCreateInfo,
        backing: GpuImageBackingInfo,
    ) -> Result<Arc<dyn GpuBackendImage>, &'static str> {
        let info = GpuBackendImageInfo::new(create, 1, backing.allocation_size);
        *self.backing.lock() = Some(backing);
        Ok(Arc::new(RecordingImage(info)))
    }
}

struct AccessGuard<'a>(&'a RecordingBackend);

impl GpuBackendCpuAccessGuard for AccessGuard<'_> {}

impl Drop for AccessGuard<'_> {
    fn drop(&mut self) {
        assert!(self.0.reserved.swap(false, Ordering::AcqRel));
        self.0.events.lock().push(Event::Released);
        // Model a subsequent GPU write admitted immediately upon release. In
        // the readback test, userspace must already contain the previous pixels.
        self.0.set_pixels(0x99);
    }
}

impl GpuBackendContext for RecordingBackend {
    fn query_info(&self) -> GpuBackendContextInfo {
        GpuBackendContextInfo::new(0, 1)
    }

    fn create_queue(&self) -> Result<Arc<dyn GpuBackendQueue>, &'static str> {
        Err("recording backend has no hardware queue")
    }

    fn begin_image_cpu_access(
        &self,
        _image: &dyn GpuBackendImage,
    ) -> Result<Option<Box<dyn GpuBackendCpuAccessGuard + '_>>, &'static str> {
        // A queued GPU read still sees the old pixels when the hook is entered.
        // This assertion fails if generic upload writes before retiring it.
        assert_eq!(self.pixels(), [0x11; 16]);
        if self.reject {
            self.events.lock().push(Event::Rejected);
            return Err("retirement failed");
        }
        assert!(!self.reserved.swap(true, Ordering::AcqRel));
        self.events.lock().push(Event::Retired);
        Ok(Some(Box::new(AccessGuard(self))))
    }

    fn upload_image_bgra(
        &self,
        _image: &dyn GpuBackendImage,
        _upload: GpuImageUploadInfo,
    ) -> Result<(), &'static str> {
        // The backend callback must still own the admission reservation after
        // all generic writes and cache maintenance have finished.
        assert!(self.reserved.load(Ordering::Acquire));
        assert_eq!(self.pixels(), [0x22; 16]);
        self.events.lock().push(Event::Uploaded);
        if self.fail_transfer {
            Err("upload transfer failed")
        } else {
            Ok(())
        }
    }

    fn readback_image_bgra(
        &self,
        _image: &dyn GpuBackendImage,
        _readback: GpuImageUploadInfo,
    ) -> Result<(), &'static str> {
        assert!(self.reserved.load(Ordering::Acquire));
        self.set_pixels(0x33);
        self.events.lock().push(Event::Readback);
        Ok(())
    }
}

fn fixture(
    reject: bool,
    fail_transfer: bool,
) -> (Arc<RecordingBackend>, GpuImage, crate::task::Task, usize) {
    let backend = Arc::new(RecordingBackend::new(reject, fail_transfer));
    let image = GpuImage::new(
        backend.clone(),
        GpuImageCreateInfo::new(
            GPU_IMAGE_FORMAT_BGRA8_UNORM,
            GPU_IMAGE_USAGE_TRANSFER_DST | GPU_IMAGE_USAGE_TRANSFER_SRC,
            2,
            2,
        ),
    )
    .expect("test image allocation succeeds");
    backend.set_pixels(0x11);
    let task = crate::task::new_user_task("gpu-cpu-access".into(), 0);
    let address = crate::environment::USER_STACK_END - crate::environment::PAGE_SIZE;
    task.allocate_stack_pages(address, 1)
        .expect("test user memory allocation succeeds");
    copy_to_user(&task, address, &[0x22; 16]).expect("test upload source is initialized");
    (backend, image, task, address)
}

#[test_case]
fn upload_retires_before_real_usercopy_and_retains_guard_through_transfer() {
    let (backend, image, task, address) = fixture(false, false);
    let request = GpuContextUploadImageBgra::new(1, address as u64, 16, 8, 0, 0, 2, 2);
    let layout = image_upload_layout(&request, image.query_info(), image.layout()).unwrap();
    image
        .upload_bgra_for_task(&task, address, layout, backend.as_ref())
        .unwrap();
    assert_eq!(
        *backend.events.lock(),
        [Event::Retired, Event::Uploaded, Event::Released]
    );
    assert!(!backend.reserved.load(Ordering::Acquire));
}

#[test_case]
fn rejected_upload_leaves_real_backing_unchanged() {
    let (backend, image, task, address) = fixture(true, false);
    let request = GpuContextUploadImageBgra::new(1, address as u64, 16, 8, 0, 0, 2, 2);
    let layout = image_upload_layout(&request, image.query_info(), image.layout()).unwrap();
    assert_eq!(
        image.upload_bgra_for_task(&task, address, layout, backend.as_ref()),
        Err("retirement failed")
    );
    assert_eq!(backend.pixels(), [0x11; 16]);
    assert_eq!(*backend.events.lock(), [Event::Rejected]);
    assert!(!backend.reserved.load(Ordering::Acquire));
}

#[test_case]
fn usercopy_failure_releases_reservation_without_backend_transfer() {
    let (backend, image, task, address) = fixture(false, false);
    let request = GpuContextUploadImageBgra::new(1, address as u64, 16, 8, 0, 0, 2, 2);
    let layout = image_upload_layout(&request, image.query_info(), image.layout()).unwrap();
    assert!(
        image
            .upload_bgra_for_task(&task, 0, layout, backend.as_ref())
            .is_err()
    );
    assert_eq!(*backend.events.lock(), [Event::Retired, Event::Released]);
    assert!(!backend.reserved.load(Ordering::Acquire));
}

#[test_case]
fn failed_backend_transfer_releases_reservation() {
    let (backend, image, task, address) = fixture(false, true);
    let request = GpuContextUploadImageBgra::new(1, address as u64, 16, 8, 0, 0, 2, 2);
    let layout = image_upload_layout(&request, image.query_info(), image.layout()).unwrap();
    assert_eq!(
        image.upload_bgra_for_task(&task, address, layout, backend.as_ref()),
        Err("upload transfer failed")
    );
    assert_eq!(
        *backend.events.lock(),
        [Event::Retired, Event::Uploaded, Event::Released]
    );
    assert!(!backend.reserved.load(Ordering::Acquire));
}

#[test_case]
fn readback_copies_before_reservation_release_admits_later_gpu_write() {
    let (backend, image, task, address) = fixture(false, false);
    let request = GpuContextReadbackImageBgra::new(1, address as u64, 16, 8, 0, 0, 2, 2);
    let layout = image_readback_layout(&request, image.query_info(), image.layout()).unwrap();
    image
        .readback_bgra_for_task(&task, address, layout, backend.as_ref())
        .unwrap();
    let mut pixels = [0; 16];
    copy_from_user(&task, address, &mut pixels).unwrap();
    assert_eq!(pixels, [0x33; 16]);
    assert_eq!(backend.pixels(), [0x99; 16]);
    assert_eq!(
        *backend.events.lock(),
        [Event::Retired, Event::Readback, Event::Released]
    );
}

#[test_case]
fn rejected_readback_preserves_backing_and_user_destination() {
    let (backend, image, task, address) = fixture(true, false);
    let request = GpuContextReadbackImageBgra::new(1, address as u64, 16, 8, 0, 0, 2, 2);
    let layout = image_readback_layout(&request, image.query_info(), image.layout()).unwrap();
    assert_eq!(
        image.readback_bgra_for_task(&task, address, layout, backend.as_ref()),
        Err("retirement failed")
    );
    let mut pixels = [0; 16];
    copy_from_user(&task, address, &mut pixels).unwrap();
    assert_eq!(pixels, [0x22; 16]);
    assert_eq!(backend.pixels(), [0x11; 16]);
    assert_eq!(*backend.events.lock(), [Event::Rejected]);
}
