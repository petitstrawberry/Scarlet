//! Real PMM ownership and direct user-copy/coherency tests.
use super::*;
use crate::device::gpu::{
    GpuBackendBufferInfo, GpuBackendContextInfo, GpuBackendCpuAccessGuard, GpuBackendInfo,
    GpuBackendQueue, GpuBufferUploadInfo, GpuDeviceInfo, GpuDeviceState,
};
use alloc::{boxed::Box, sync::Weak};

#[derive(Default)]
struct State {
    create: Option<GpuBufferCreateInfo>,
    calls: usize,
    drops: usize,
    free_at_query: usize,
    free_at_drop: usize,
    backing: Option<Weak<GpuBufferBacking>>,
    events: Vec<u8>,
    reserved: bool,
}

struct Backend {
    state: Arc<Mutex<State>>,
    alignment: usize,
    bad_metadata: u8,
    reject: bool,
    fail_upload: bool,
    inline: bool,
}

impl Backend {
    fn new(alignment: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                events: Vec::with_capacity(8),
                ..State::default()
            })),
            alignment,
            bad_metadata: 0,
            reject: false,
            fail_upload: false,
            inline: false,
        }
    }

    fn reserve(&self) -> Result<(), &'static str> {
        // This fails if generic code writes before retiring old GPU readers.
        assert_eq!(self.bytes(), [0; 32]);
        let mut state = self.state.lock();
        if self.reject {
            state.events.push(4);
            return Err("retirement failed");
        }
        assert!(!state.reserved);
        state.reserved = true;
        state.events.push(1);
        Ok(())
    }

    fn bytes(&self) -> [u8; 32] {
        let create = self.state.lock().create.unwrap();
        // SAFETY: each test keeps its GpuBuffer live and uses no actual DMA.
        unsafe { (phys_to_virt(create.paddr) as *const [u8; 32]).read() }
    }
}

struct Buffer {
    state: Arc<Mutex<State>>,
    info: GpuBackendBufferInfo,
    check_snapshot: bool,
}
impl GpuBackendBuffer for Buffer {
    fn query_info(&self) -> GpuBackendBufferInfo {
        self.state.lock().free_at_query = crate::mem::pmm::stats().1;
        self.info
    }
    fn backend_cookie(&self) -> u64 {
        1
    }
}
impl Drop for Buffer {
    fn drop(&mut self) {
        let mut state = self.state.lock();
        // No PMM page may be released before backend resource teardown.
        state.free_at_drop = crate::mem::pmm::stats().1;
        if self.check_snapshot {
            assert_eq!(state.free_at_drop, state.free_at_query);
        }
        if let Some(backing) = &state.backing {
            assert!(backing.upgrade().is_some());
        }
        state.drops += 1;
    }
}
impl GpuBackend for Backend {
    fn query_info(&self) -> GpuBackendInfo {
        GpuBackendInfo::new(
            GpuDeviceInfo::new(GpuDeviceState::Ready, 0, 0),
            0,
            b"buffer-test",
            &[],
        )
    }
    fn buffer_backing_alignment(&self) -> usize {
        self.alignment
    }
    fn create_buffer(
        &self,
        create: GpuBufferCreateInfo,
    ) -> Result<Arc<dyn GpuBackendBuffer>, &'static str> {
        let mut state = self.state.lock();
        state.calls += 1;
        state.create = Some(create);
        if self.bad_metadata == 3 {
            state.free_at_query = crate::mem::pmm::stats().1;
            return Err("backend allocation failed");
        }
        Ok(Arc::new(Buffer {
            state: self.state.clone(),
            check_snapshot: self.bad_metadata != 0,
            info: GpuBackendBufferInfo::new(
                if self.bad_metadata == 1 { 0 } else { 1 },
                create.allocation_size
                    + if self.bad_metadata == 2 {
                        PAGE_SIZE as u64
                    } else {
                        0
                    },
            ),
        }))
    }
}
// Deliberately inherits the real trait default, rather than reproducing it.
struct DefaultBackend(Backend);
impl GpuBackend for DefaultBackend {
    fn query_info(&self) -> GpuBackendInfo {
        GpuBackend::query_info(&self.0)
    }
    fn create_buffer(
        &self,
        create: GpuBufferCreateInfo,
    ) -> Result<Arc<dyn GpuBackendBuffer>, &'static str> {
        self.0.create_buffer(create)
    }
}

#[test_case]
fn buffer_default_granule_preserves_cpu_page_allocation() {
    let backend = Arc::new(DefaultBackend(Backend::new(0)));
    let buffer = GpuBuffer::new(backend.clone(), PAGE_SIZE + 1, 0).unwrap();
    assert_eq!(buffer.allocation_size(), 2 * PAGE_SIZE);
    assert_eq!(buffer.backing.paddr() % PAGE_SIZE as u64, 0);
    backend.0.state.lock().backing = Some(Arc::downgrade(&buffer.backing));
    drop(buffer);
    assert_eq!(backend.0.state.lock().drops, 1);
}

#[test_case]
fn buffer_uat_granule_aligns_and_zeroes_full_allocation() {
    let backend = Arc::new(Backend::new(16 * 1024));
    let buffer = GpuBuffer::new(
        backend.clone(),
        16 * 1024 + 1,
        super::super::GPU_BUFFER_FLAG_CPU_VISIBLE,
    )
    .unwrap();
    assert_eq!(buffer.allocation_size(), 32 * 1024);
    assert_eq!(buffer.backing.paddr() % (16 * 1024), 0);
    assert_eq!(
        backend.state.lock().create.unwrap().allocation_size,
        32 * 1024
    );
    // SAFETY: no CPU writer or DMA user exists; the complete PMM owner is live.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            buffer.backing.pages.as_vaddr() as *const u8,
            buffer.allocation_size(),
        )
    };
    assert!(bytes.iter().all(|byte| *byte == 0));
    assert!(buffer.get_mapping_info(7 * PAGE_SIZE, PAGE_SIZE).is_ok());
    assert!(buffer.get_mapping_info(8 * PAGE_SIZE, PAGE_SIZE).is_err());
    let backing = buffer.backing();
    let weak = Arc::downgrade(&backing);
    backend.state.lock().backing = Some(weak.clone());
    // The owner survives the capability while attachments/in-flight work retain it.
    drop(buffer);
    assert!(weak.upgrade().is_some());
    drop(backing);
    assert!(weak.upgrade().is_none());
}

#[test_case]
fn buffer_invalid_granule_and_overflow_never_call_backend() {
    for alignment in [0, PAGE_SIZE / 2, PAGE_SIZE + 1, 3 * PAGE_SIZE] {
        let backend = Arc::new(Backend::new(alignment));
        assert!(GpuBuffer::new(backend.clone(), 1, 0).is_err());
        assert_eq!(backend.state.lock().calls, 0);
    }
    let backend = Arc::new(Backend::new(16 * 1024));
    assert!(GpuBuffer::new(backend.clone(), usize::MAX, 0).is_err());
    assert!(GpuBuffer::new(backend.clone(), 0, 0).is_err());
    assert_eq!(backend.state.lock().calls, 0);
}

#[test_case]
fn buffer_invalid_backend_metadata_drops_resource_before_reclaiming_pages() {
    for bad_metadata in [1, 2] {
        let mut backend = Backend::new(16 * 1024);
        backend.bad_metadata = bad_metadata;
        let backend = Arc::new(backend);
        assert!(GpuBuffer::new(backend.clone(), PAGE_SIZE + 1, 0).is_err());
        let state = backend.state.lock();
        assert_eq!(state.drops, 1);
        // Buddy alignment can retain more than the four visible pages; every
        // requested page, including the rounded tail, must now be reclaimed.
        assert!(crate::mem::pmm::stats().1 >= state.free_at_drop + 4);
    }
}

#[test_case]
fn buffer_backend_creation_failure_reclaims_complete_granule() {
    let mut backend = Backend::new(16 * 1024);
    backend.bad_metadata = 3;
    let backend = Arc::new(backend);
    assert!(GpuBuffer::new(backend.clone(), 1, 0).is_err());
    let state = backend.state.lock();
    assert_eq!(state.calls, 1);
    assert_eq!(state.drops, 0);
    assert!(crate::mem::pmm::stats().1 >= state.free_at_query + 4);
}

struct Guard<'a>(&'a Backend);
impl GpuBackendCpuAccessGuard for Guard<'_> {}
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock();
        assert!(state.reserved);
        state.reserved = false;
        state.events.push(3);
    }
}
impl GpuBackendContext for Backend {
    fn query_info(&self) -> GpuBackendContextInfo {
        GpuBackendContextInfo::new(0, 1)
    }
    fn create_queue(&self) -> Result<Arc<dyn GpuBackendQueue>, &'static str> {
        Err("no hardware queue")
    }
    fn begin_buffer_cpu_access_inline(
        &self,
        _: &dyn GpuBackendBuffer,
    ) -> Result<Option<u64>, &'static str> {
        if !self.inline {
            return Ok(None);
        }
        self.reserve()?;
        Ok(Some(7))
    }
    fn end_buffer_cpu_access_inline(&self, _: &dyn GpuBackendBuffer, token: u64) {
        assert_eq!(token, 7);
        drop(Guard(self));
    }

    fn begin_buffer_cpu_access(
        &self,
        _: &dyn GpuBackendBuffer,
    ) -> Result<Option<Box<dyn GpuBackendCpuAccessGuard + '_>>, &'static str> {
        assert!(!self.inline, "inline lease must bypass boxed hook");
        self.reserve()?;
        Ok(Some(Box::new(Guard(self))))
    }
    fn upload_buffer(
        &self,
        _: &dyn GpuBackendBuffer,
        upload: GpuBufferUploadInfo,
    ) -> Result<(), &'static str> {
        assert_eq!(
            upload,
            GpuBufferUploadInfo {
                offset: 8,
                size_bytes: 16
            }
        );
        let mut expected = [0; 32];
        expected[8..24].fill(0x52);
        assert_eq!(self.bytes(), expected);
        let mut state = self.state.lock();
        assert!(state.reserved);
        state.events.push(2);
        if self.fail_upload {
            Err("upload failed")
        } else {
            Ok(())
        }
    }
}

#[test_case]
fn buffer_upload_direct_copy_retains_guard_through_callback_and_error() {
    for inline in [false, true] {
        for (reject, fail_upload) in [(false, false), (true, false), (false, true)] {
            let mut backend = Backend::new(16 * 1024);
            backend.inline = inline;
            backend.reject = reject;
            backend.fail_upload = fail_upload;
            let backend = Arc::new(backend);
            let buffer = GpuBuffer::new(backend.clone(), PAGE_SIZE, 0).unwrap();
            let task = crate::task::new_user_task("buffer-upload".into(), 0);
            let address = crate::environment::USER_STACK_END - PAGE_SIZE;
            task.allocate_stack_pages(address, 1).unwrap();
            copy_to_user(&task, address, &[0x52; 16]).unwrap();
            let upload = GpuBufferUploadInfo {
                offset: 8,
                size_bytes: 16,
            };
            assert_eq!(
                buffer
                    .upload_for_task(&task, address, upload, backend.as_ref())
                    .is_err(),
                reject || fail_upload
            );
            assert_eq!(
                backend.state.lock().events.as_slice(),
                if reject { &[4][..] } else { &[1, 2, 3][..] }
            );
            assert!(!backend.state.lock().reserved);
            if reject {
                assert_eq!(backend.bytes(), [0; 32]);
            }
            // User task setup legitimately changes global PMM accounting; refresh
            // the teardown snapshot without mutating the allocation under test.
            let _ = buffer.backend_info();
        }
    }
}

#[test_case]
fn buffer_upload_rejects_all_invalid_ranges_before_guard_or_copy() {
    let backend = Arc::new(Backend::new(16 * 1024));
    let buffer = GpuBuffer::new(backend.clone(), 1, 0).unwrap();
    let task = crate::task::new_user_task("buffer-upload-bounds".into(), 0);
    let source = crate::environment::USER_STACK_END - PAGE_SIZE;
    for (source, offset, size_bytes) in [
        (0, 0, 1),
        (source, 0, 0),
        (
            source,
            0,
            super::super::GPU_MAX_BUFFER_UPLOAD_SIZE as u64 + 1,
        ),
        (source, 16 * 1024, 1),
        (source, u64::MAX, 2),
        (usize::MAX, 0, 2),
    ] {
        assert!(
            buffer
                .upload_for_task(
                    &task,
                    source,
                    GpuBufferUploadInfo { offset, size_bytes },
                    backend.as_ref()
                )
                .is_err()
        );
    }
    assert!(backend.state.lock().events.is_empty());
    assert_eq!(backend.bytes(), [0; 32]);
    let _ = buffer.backend_info();
}

#[test_case]
fn buffer_upload_source_failure_releases_guard_without_callback_or_writes() {
    for inline in [false, true] {
        let mut backend = Backend::new(16 * 1024);
        backend.inline = inline;
        let backend = Arc::new(backend);
        let buffer = GpuBuffer::new(backend.clone(), 1, 0).unwrap();
        let task = crate::task::new_user_task("buffer-upload-source".into(), 0);
        let address = crate::environment::USER_STACK_END - PAGE_SIZE;
        assert!(
            buffer
                .upload_for_task(
                    &task,
                    address,
                    GpuBufferUploadInfo {
                        offset: 8,
                        size_bytes: 16
                    },
                    backend.as_ref()
                )
                .is_err()
        );
        assert_eq!(*backend.state.lock().events, [1, 3]);
        assert!(!backend.state.lock().reserved);
        assert_eq!(backend.bytes(), [0; 32]);
        let _ = buffer.backend_info();
    }
}

struct InvalidInlineLease;
impl GpuBackendContext for InvalidInlineLease {
    fn query_info(&self) -> GpuBackendContextInfo {
        GpuBackendContextInfo::new(0, 1)
    }
    fn create_queue(&self) -> Result<Arc<dyn GpuBackendQueue>, &'static str> {
        Err("no queue")
    }
    fn begin_buffer_cpu_access_inline(
        &self,
        _: &dyn GpuBackendBuffer,
    ) -> Result<Option<u64>, &'static str> {
        Ok(Some(0))
    }
    fn begin_buffer_cpu_access(
        &self,
        _: &dyn GpuBackendBuffer,
    ) -> Result<Option<Box<dyn GpuBackendCpuAccessGuard + '_>>, &'static str> {
        panic!("invalid inline lease must not fall back")
    }
    fn end_buffer_cpu_access_inline(&self, _: &dyn GpuBackendBuffer, _: u64) {
        panic!("zero is not a valid lease to release")
    }
}

#[test_case]
fn buffer_zero_inline_token_rejects_before_copy_or_box_fallback() {
    let backend = Arc::new(Backend::new(16 * 1024));
    let buffer = GpuBuffer::new(backend.clone(), 1, 0).unwrap();
    let task = crate::task::new_user_task("buffer-invalid-lease".into(), 0);
    let address = crate::environment::USER_STACK_END - PAGE_SIZE;
    task.allocate_stack_pages(address, 1).unwrap();
    copy_to_user(&task, address, &[0x52; 16]).unwrap();
    assert!(
        buffer
            .upload_for_task(
                &task,
                address,
                GpuBufferUploadInfo {
                    offset: 8,
                    size_bytes: 16
                },
                &InvalidInlineLease
            )
            .is_err()
    );
    assert_eq!(backend.bytes(), [0; 32]);
}

#[test_case]
fn buffer_upload_rejects_user_alias_of_owned_pages_before_copy() {
    use crate::vm::vmem::{MemoryArea, PhysicalMemoryArea, VirtualMemoryMap};
    let mut backend = Backend::new(16 * 1024);
    backend.inline = true;
    let backend = Arc::new(backend);
    let buffer = GpuBuffer::new(
        backend.clone(),
        1,
        super::super::GPU_BUFFER_FLAG_CPU_VISIBLE,
    )
    .unwrap();
    let task = crate::task::new_user_task("buffer-upload-alias".into(), 0);
    let address = crate::environment::USER_STACK_END - PAGE_SIZE;
    let physical = buffer.backing.paddr();
    task.vm_manager
        .add_memory_map(VirtualMemoryMap::new(
            PhysicalMemoryArea::new(physical, physical + PAGE_SIZE as u64 - 1),
            MemoryArea::new(address, address + PAGE_SIZE - 1),
            3,
            true,
            None,
        ))
        .unwrap();
    assert_eq!(
        buffer.upload_for_task(
            &task,
            address,
            GpuBufferUploadInfo {
                offset: 8,
                size_bytes: 16
            },
            backend.as_ref()
        ),
        Err("GPU buffer upload source aliases destination backing")
    );
    assert_eq!(backend.bytes(), [0; 32]);
    assert_eq!(*backend.state.lock().events, [1, 3]);
    assert!(!backend.state.lock().reserved);
}

#[test_case]
fn buffer_partial_usercopy_failure_releases_inline_guard_without_callback() {
    let mut backend = Backend::new(16 * 1024);
    backend.inline = true;
    let backend = Arc::new(backend);
    let buffer = GpuBuffer::new(backend.clone(), 1, 0).unwrap();
    let task = crate::task::new_user_task("buffer-upload-partial".into(), 0);
    // The first chunk is mapped, the second page is deliberately absent.
    let page = crate::environment::USER_STACK_END - 2 * PAGE_SIZE;
    task.allocate_stack_pages(page, 1).unwrap();
    let address = page + PAGE_SIZE - 8;
    copy_to_user(&task, address, &[0x52; 8]).unwrap();
    assert!(
        buffer
            .upload_for_task(
                &task,
                address,
                GpuBufferUploadInfo {
                    offset: 8,
                    size_bytes: 16
                },
                backend.as_ref()
            )
            .is_err()
    );
    let mut expected = [0; 32];
    expected[8..16].fill(0x52);
    assert_eq!(backend.bytes(), expected);
    assert_eq!(*backend.state.lock().events, [1, 3]);
    assert!(!backend.state.lock().reserved);
}

#[test_case]
fn buffer_query_results_require_cpu_visibility_and_reach_backend() {
    use super::super::{GPU_BUFFER_FLAG_CPU_VISIBLE, GPU_BUFFER_FLAG_QUERY_RESULT};
    let backend = Arc::new(Backend::new(PAGE_SIZE));
    assert!(GpuBuffer::new(backend.clone(), PAGE_SIZE, GPU_BUFFER_FLAG_QUERY_RESULT).is_err());
    assert!(GpuBuffer::new(backend.clone(), PAGE_SIZE, 1 << 31).is_err());
    assert_eq!(backend.state.lock().calls, 0);
    let flags = GPU_BUFFER_FLAG_CPU_VISIBLE | GPU_BUFFER_FLAG_QUERY_RESULT;
    let buffer = GpuBuffer::new(backend.clone(), PAGE_SIZE, flags).unwrap();
    assert_eq!(backend.state.lock().create.unwrap().flags, flags);
    assert!(buffer.get_mapping_info(0, PAGE_SIZE).is_ok());
}
