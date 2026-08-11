//! GPU control device interface.
//!
//! This module exposes the backend-neutral GPU control ABI. Display scanout
//! remains in [`crate::device::graphics`]. GPU connections expose generic
//! device information and create kernel-owned memory, timeline, execution
//! context, and queue capability objects. Queue command bytes are opaque to
//! this module and synchronously complete in the current ABI phase.

mod abi;
mod backend;
mod connection;
mod execution;
mod object;
mod resource;

pub use abi::{
    GPU_ABI_VERSION, GPU_BACKEND_ID_BYTES, GPU_BACKEND_INFO_BYTES, GPU_BUFFER_FLAG_CPU_VISIBLE,
    GPU_BUFFER_FLAGS_VALID, GPU_BUFFER_QUERY_INFO, GPU_CONTEXT_ATTACH_BUFFER,
    GPU_CONTEXT_ATTACH_IMAGE, GPU_CONTEXT_DETACH_IMAGE, GPU_CONTEXT_QUERY,
    GPU_CONTEXT_TRANSFER_IMPORTED_IMAGE_BGRA, GPU_CONTEXT_UPLOAD_IMAGE_BGRA, GPU_CREATE_BUFFER,
    GPU_CREATE_CONTEXT, GPU_CREATE_IMAGE, GPU_CREATE_IMPORTED_IMAGE_BGRA, GPU_CREATE_QUEUE,
    GPU_CREATE_TIMELINE, GPU_DIALECT_INFO_BYTES, GPU_IMAGE_FORMAT_BGRA8_UNORM,
    GPU_IMAGE_FORMAT_DEPTH32_FLOAT, GPU_IMAGE_QUERY_INFO, GPU_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT,
    GPU_IMAGE_USAGE_PRESENTABLE, GPU_IMAGE_USAGE_RENDER_TARGET, GPU_IMAGE_USAGE_SAMPLED,
    GPU_IMAGE_USAGE_TRANSFER_DST, GPU_IMAGE_USAGE_VALID, GPU_MAX_IMAGE_UPLOAD_SIZE,
    GPU_MAX_OPAQUE_COMMAND_SIZE, GPU_QUERY_DIALECT, GPU_QUERY_INFO, GPU_QUEUE_QUERY,
    GPU_QUEUE_SUBMIT, GPU_QUEUE_SUBMIT_FLAG_SIGNAL_TIMELINE, GPU_QUEUE_SUBMIT_FLAGS_VALID,
    GPU_RESULT_INVALID_ABI, GPU_RESULT_INVALID_ARGUMENT, GPU_RESULT_INVALID_STATE,
    GPU_RESULT_OUT_OF_RESOURCES, GPU_RESULT_SUCCESS, GPU_RESULT_UNSUPPORTED,
    GPU_TIMELINE_CREATE_POINT, GPU_TIMELINE_FAIL, GPU_TIMELINE_QUERY, GPU_TIMELINE_SIGNAL,
    GpuBufferInfo, GpuContextAttachBuffer, GpuContextAttachImage, GpuContextDetachImage,
    GpuContextInfo, GpuContextTransferImportedImageBgra, GpuContextUploadImageBgra,
    GpuCreateBuffer, GpuCreateContext, GpuCreateImage, GpuCreateImportedImageBgra, GpuCreateQueue,
    GpuCreateTimeline, GpuImageInfo, GpuQueryDialect, GpuQueryInfo, GpuQueueInfo, GpuQueueSubmit,
    GpuTimelineCreatePoint, GpuTimelineFail, GpuTimelineInfo, GpuTimelineSignal,
};
pub use backend::{
    GPU_EXECUTION_SUPPORT_ADDRESS_SPACE, GPU_EXECUTION_SUPPORT_DEPTH,
    GPU_EXECUTION_SUPPORT_IMAGE_UPLOAD, GPU_EXECUTION_SUPPORT_MEMORY, GPU_EXECUTION_SUPPORT_NONE,
    GPU_EXECUTION_SUPPORT_PRESENTATION, GPU_EXECUTION_SUPPORT_QUEUE,
    GPU_EXECUTION_SUPPORT_TIMELINE, GpuBackend, GpuBackendBuffer, GpuBackendBufferInfo,
    GpuBackendContext, GpuBackendContextInfo, GpuBackendDialectDescriptor, GpuBackendDialectInfo,
    GpuBackendImage, GpuBackendImageInfo, GpuBackendInfo, GpuBackendQueue, GpuBackendQueueInfo,
    GpuBufferCreateInfo, GpuDeviceInfo, GpuDeviceState, GpuImageBackingInfo, GpuImageCreateInfo,
    GpuImageUploadInfo,
};
pub use connection::GpuConnection;
pub use execution::{GpuContext, GpuQueue};
pub use object::GpuControlDevice;
pub use resource::{GpuBuffer, GpuImage, GpuObject, GpuTimeline, GpuTimelinePoint};

fn child_handle_metadata(
    access_mode: crate::object::handle::AccessMode,
) -> crate::object::handle::HandleMetadata {
    crate::object::handle::HandleMetadata {
        handle_type: crate::object::handle::HandleType::Regular,
        access_mode,
        special_semantics: None,
    }
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;

    use super::{
        GPU_ABI_VERSION, GPU_BUFFER_FLAG_CPU_VISIBLE, GPU_EXECUTION_SUPPORT_MEMORY,
        GPU_EXECUTION_SUPPORT_TIMELINE, GPU_IMAGE_FORMAT_BGRA8_UNORM,
        GPU_IMAGE_FORMAT_DEPTH32_FLOAT, GPU_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT,
        GPU_IMAGE_USAGE_PRESENTABLE, GPU_IMAGE_USAGE_RENDER_TARGET, GPU_IMAGE_USAGE_SAMPLED,
        GPU_IMAGE_USAGE_TRANSFER_DST, GPU_RESULT_INVALID_ABI, GPU_RESULT_INVALID_ARGUMENT,
        GPU_RESULT_SUCCESS, GpuBackend, GpuBackendBuffer, GpuBackendBufferInfo,
        GpuBackendImageInfo, GpuBackendInfo, GpuBuffer, GpuBufferCreateInfo, GpuConnection,
        GpuContextUploadImageBgra, GpuControlDevice, GpuDeviceInfo, GpuDeviceState,
        GpuImageCreateInfo, GpuObject, GpuQueryInfo, GpuTimeline, GpuTimelinePoint,
    };
    use crate::device::Device;
    use crate::object::KernelObject;
    use crate::object::capability::selectable::ReadyInterest;
    use crate::object::capability::{ControlOps, MemoryMappingOps, Selectable};
    use crate::object::handle::{AccessMode, HandleMetadata, HandleTable, HandleType};

    struct TestBackend;

    struct TestBackendBuffer {
        allocation_size: u64,
    }

    impl GpuBackendBuffer for TestBackendBuffer {
        fn query_info(&self) -> GpuBackendBufferInfo {
            GpuBackendBufferInfo::new(1, self.allocation_size)
        }

        fn backend_cookie(&self) -> u64 {
            1
        }
    }

    impl GpuBackend for TestBackend {
        fn query_info(&self) -> GpuBackendInfo {
            GpuBackendInfo::new(
                GpuDeviceInfo::new(
                    GpuDeviceState::Ready,
                    GPU_EXECUTION_SUPPORT_MEMORY | GPU_EXECUTION_SUPPORT_TIMELINE,
                    0,
                ),
                0x4d,
                b"test-gpu",
                &[0xa5, 0x5a],
            )
        }

        fn create_buffer(
            &self,
            create: GpuBufferCreateInfo,
        ) -> Result<Arc<dyn GpuBackendBuffer>, &'static str> {
            Ok(Arc::new(TestBackendBuffer {
                allocation_size: create.allocation_size,
            }))
        }
    }

    fn open_connection(device: Arc<GpuControlDevice>) -> Arc<dyn Device> {
        Device::open(device).expect("GPU control device should open")
    }

    #[test_case]
    fn gpu_control_device_creates_independent_connections() {
        let device = Arc::new(GpuControlDevice::new(Arc::new(TestBackend)));
        let first = open_connection(Arc::clone(&device));
        let second = open_connection(device);
        let first = first
            .as_any()
            .downcast_ref::<GpuConnection>()
            .expect("GPU open endpoint should be a GpuConnection");
        let second = second
            .as_any()
            .downcast_ref::<GpuConnection>()
            .expect("GPU open endpoint should be a GpuConnection");

        assert!(!core::ptr::eq(first, second));
    }

    #[test_case]
    fn gpu_connection_reports_backend_information() {
        let device = Arc::new(GpuControlDevice::new(Arc::new(TestBackend)));
        let connection = open_connection(device);
        let connection = connection
            .as_any()
            .downcast_ref::<GpuConnection>()
            .expect("GPU open endpoint should be a GpuConnection");
        let mut query = GpuQueryInfo::new();

        connection.query_info(&mut query);

        assert_eq!(query.abi_version, GPU_ABI_VERSION);
        assert_eq!(query.result, GPU_RESULT_SUCCESS);
        assert_eq!(query.device_state, GpuDeviceState::Ready as u32);
        assert_eq!(
            query.execution_support,
            GPU_EXECUTION_SUPPORT_MEMORY | GPU_EXECUTION_SUPPORT_TIMELINE
        );
        assert_eq!(query.backend_feature_bits, 0x4d);
        assert_eq!(query.backend_id_len, 8);
        assert_eq!(&query.backend_id[..8], b"test-gpu");
        assert_eq!(query.backend_info_len, 2);
        assert_eq!(&query.backend_info[..2], &[0xa5, 0x5a]);
    }

    #[test_case]
    fn gpu_connection_reports_invalid_abi_in_response() {
        let device = Arc::new(GpuControlDevice::new(Arc::new(TestBackend)));
        let connection = open_connection(device);
        let connection = connection
            .as_any()
            .downcast_ref::<GpuConnection>()
            .expect("GPU open endpoint should be a GpuConnection");
        let mut query = GpuQueryInfo::new();
        query.abi_version = GPU_ABI_VERSION + 1;

        connection.query_info(&mut query);

        assert_eq!(query.result, GPU_RESULT_INVALID_ABI);
    }

    #[test_case]
    fn gpu_connection_rejects_nonzero_reserved_query_fields() {
        let device = Arc::new(GpuControlDevice::new(Arc::new(TestBackend)));
        let connection = open_connection(device);
        let connection = connection
            .as_any()
            .downcast_ref::<GpuConnection>()
            .expect("GPU open endpoint should be a GpuConnection");
        let mut query = GpuQueryInfo::new();
        query.reserved = 1;

        connection.query_info(&mut query);

        assert_eq!(query.result, GPU_RESULT_INVALID_ARGUMENT);
        assert_eq!(query.reserved, 0);
    }

    #[test_case]
    fn gpu_abi_records_are_fixed_width() {
        assert_eq!(core::mem::size_of::<super::GpuCreateBuffer>(), 48);
        assert_eq!(core::mem::size_of::<super::GpuBufferInfo>(), 40);
        assert_eq!(core::mem::size_of::<super::GpuCreateTimeline>(), 40);
        assert_eq!(core::mem::size_of::<super::GpuTimelineInfo>(), 32);
        assert_eq!(core::mem::size_of::<super::GpuTimelineSignal>(), 32);
        assert_eq!(core::mem::size_of::<super::GpuTimelineFail>(), 24);
        assert_eq!(core::mem::size_of::<super::GpuTimelineCreatePoint>(), 32);
        assert_eq!(core::mem::size_of::<super::GpuQueryDialect>(), 296);
        assert_eq!(core::mem::size_of::<super::GpuCreateContext>(), 48);
        assert_eq!(core::mem::size_of::<super::GpuContextInfo>(), 32);
        assert_eq!(core::mem::size_of::<super::GpuCreateQueue>(), 32);
        assert_eq!(core::mem::size_of::<super::GpuQueueInfo>(), 24);
        assert_eq!(core::mem::size_of::<super::GpuQueueSubmit>(), 56);
        assert_eq!(core::mem::size_of::<super::GpuCreateImage>(), 48);
        assert_eq!(core::mem::size_of::<super::GpuImageInfo>(), 40);
        assert_eq!(core::mem::size_of::<super::GpuContextAttachImage>(), 32);
        assert_eq!(core::mem::size_of::<super::GpuContextDetachImage>(), 24);
        assert_eq!(
            core::mem::size_of::<super::GpuCreateImportedImageBgra>(),
            64
        );
        assert_eq!(core::mem::size_of::<super::GpuContextAttachBuffer>(), 32);
        assert_eq!(core::mem::size_of::<super::GpuContextUploadImageBgra>(), 64);
        assert_eq!(
            core::mem::size_of::<super::GpuContextTransferImportedImageBgra>(),
            40
        );
    }

    #[test_case]
    fn gpu_image_format_usage_and_extent_validation_is_strict() {
        let valid = GpuImageCreateInfo::new(
            GPU_IMAGE_FORMAT_BGRA8_UNORM,
            GPU_IMAGE_USAGE_RENDER_TARGET | GPU_IMAGE_USAGE_PRESENTABLE,
            64,
            64,
        );
        assert!(super::resource::image_create_is_valid(valid));
        assert_eq!(
            super::GpuCreateImage::new(64, 64).usage,
            GPU_IMAGE_USAGE_RENDER_TARGET | GPU_IMAGE_USAGE_PRESENTABLE
        );
        assert!(!super::resource::image_create_is_valid(
            GpuImageCreateInfo::new(0, valid.usage, valid.width, valid.height,)
        ));
        assert!(!super::resource::image_create_is_valid(
            GpuImageCreateInfo::new(valid.format, 0, valid.width, valid.height,)
        ));
        assert!(super::resource::image_create_is_valid(
            GpuImageCreateInfo::new(
                valid.format,
                GPU_IMAGE_USAGE_SAMPLED | GPU_IMAGE_USAGE_TRANSFER_DST,
                valid.width,
                valid.height,
            )
        ));
        assert!(!super::resource::image_create_is_valid(
            GpuImageCreateInfo::new(
                valid.format,
                valid.usage | (1 << 31),
                valid.width,
                valid.height,
            )
        ));
        assert!(!super::resource::image_create_is_valid(
            GpuImageCreateInfo::new(valid.format, valid.usage, 0, valid.height,)
        ));
        assert!(super::resource::image_create_is_valid(
            GpuImageCreateInfo::new(
                GPU_IMAGE_FORMAT_DEPTH32_FLOAT,
                GPU_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT,
                valid.width,
                valid.height,
            )
        ));
        assert!(!super::resource::image_create_is_valid(
            GpuImageCreateInfo::new(
                GPU_IMAGE_FORMAT_DEPTH32_FLOAT,
                GPU_IMAGE_USAGE_RENDER_TARGET,
                valid.width,
                valid.height,
            )
        ));
    }

    #[test_case]
    fn gpu_image_upload_layout_validates_strides_bounds_and_source_length() {
        let image = GpuBackendImageInfo::new(
            GpuImageCreateInfo::new(
                GPU_IMAGE_FORMAT_BGRA8_UNORM,
                GPU_IMAGE_USAGE_SAMPLED | GPU_IMAGE_USAGE_TRANSFER_DST,
                8,
                4,
            ),
            7,
            4096,
        );
        let request = GpuContextUploadImageBgra::new(1, 0x1000, 28, 16, 2, 1, 3, 2);
        assert_eq!(request.abi_version, GPU_ABI_VERSION);
        assert_eq!(request.result, GPU_RESULT_SUCCESS);
        assert_eq!(request.reserved, 0);
        assert_eq!(request.reserved2, 0);
        assert!(super::resource::image_upload_layout(&request, image).is_ok());

        let non_transfer_image = GpuBackendImageInfo::new(
            GpuImageCreateInfo::new(GPU_IMAGE_FORMAT_BGRA8_UNORM, GPU_IMAGE_USAGE_SAMPLED, 8, 4),
            7,
            4096,
        );
        assert!(super::resource::image_upload_layout(&request, non_transfer_image).is_err());

        let mut short_source = request;
        short_source.source_length = 27;
        assert!(super::resource::image_upload_layout(&short_source, image).is_err());

        let mut short_stride = request;
        short_stride.source_stride = 11;
        assert!(super::resource::image_upload_layout(&short_stride, image).is_err());

        let mut out_of_bounds = request;
        out_of_bounds.dst_x = 6;
        assert!(super::resource::image_upload_layout(&out_of_bounds, image).is_err());
    }

    #[test_case]
    fn imported_image_layout_validates_offsets_strides_and_damage_rectangles() {
        let create = GpuImageCreateInfo::new(
            GPU_IMAGE_FORMAT_BGRA8_UNORM,
            GPU_IMAGE_USAGE_SAMPLED | GPU_IMAGE_USAGE_TRANSFER_DST,
            8,
            4,
        );
        let layout = super::resource::imported_image_layout(create, 256, 16, 40)
            .expect("valid imported image layout should succeed");
        let image = GpuBackendImageInfo::new(create, 7, 256);
        let transfer =
            super::resource::imported_image_transfer_layout(image, 256, layout, 2, 1, 3, 2)
                .expect("valid imported damage rectangle should succeed");
        assert_eq!(transfer.backing_offset, 64);
        assert_eq!(transfer.backing_stride, 40);
        assert_eq!(transfer.backing_layer_stride, 160);

        assert!(super::resource::imported_image_layout(create, 256, u64::MAX, 40).is_err());
        assert!(super::resource::imported_image_layout(create, 256, 16, 31).is_err());
        assert!(
            super::resource::imported_image_transfer_layout(image, 256, layout, 7, 1, 2, 1)
                .is_err()
        );
        assert!(
            super::resource::imported_image_transfer_layout(image, 115, layout, 2, 1, 3, 2)
                .is_err()
        );
    }

    #[test_case]
    fn imported_image_rejects_write_only_shared_memory_handles() {
        assert!(super::connection::shared_memory_import_access_is_allowed(
            AccessMode::ReadOnly
        ));
        assert!(super::connection::shared_memory_import_access_is_allowed(
            AccessMode::ReadWrite
        ));
        assert!(!super::connection::shared_memory_import_access_is_allowed(
            AccessMode::WriteOnly
        ));
    }

    #[test_case]
    fn gpu_child_handle_has_explicit_metadata_and_optional_capabilities() {
        let buffer: Arc<dyn GpuObject> = Arc::new(
            GpuBuffer::new(Arc::new(TestBackend), 4096, GPU_BUFFER_FLAG_CPU_VISIBLE)
                .expect("GPU buffer allocation should succeed"),
        );
        let table = HandleTable::new();
        let handle = table
            .insert_with_metadata(
                KernelObject::Gpu(buffer),
                HandleMetadata {
                    handle_type: HandleType::Regular,
                    access_mode: AccessMode::ReadWrite,
                    special_semantics: None,
                },
            )
            .expect("GPU child handle should insert");

        let metadata = table.get_metadata(handle).expect("metadata should exist");
        assert_eq!(metadata.handle_type, HandleType::Regular);
        assert_eq!(metadata.access_mode, AccessMode::ReadWrite);
        table
            .with_object(handle, |object| {
                assert!(object.as_gpu().is_some());
                assert!(object.as_control().is_some());
                assert!(object.as_memory_mappable().is_some());
                assert!(object.as_selectable().is_none());
            })
            .expect("GPU child handle should remain valid");
    }

    #[test_case]
    fn gpu_buffer_enforces_visibility_and_mapping_bounds() {
        let visible = GpuBuffer::new(Arc::new(TestBackend), 1, GPU_BUFFER_FLAG_CPU_VISIBLE)
            .expect("GPU buffer allocation should succeed");
        assert!(visible.cpu_visible());
        assert!(visible.supports_mmap());
        assert!(!visible.supports_private_mmap());
        let mapping = visible
            .get_mapping_info(0, 4096)
            .expect("page-sized CPU-visible mapping should succeed");
        assert!(mapping.is_shared);
        assert_eq!(mapping.permissions, 0x3);
        assert!(visible.get_mapping_info_with(0, 4096, true).is_ok());
        assert!(visible.get_mapping_info_with(0, 4096, false).is_err());
        assert!(visible.get_mapping_info(1, 4096).is_err());
        assert!(visible.get_mapping_info(0, 8192).is_err());

        let hidden = GpuBuffer::new(Arc::new(TestBackend), 4096, 0)
            .expect("GPU buffer allocation should succeed");
        assert!(!hidden.cpu_visible());
        assert!(!hidden.supports_mmap());
        assert!(GpuObject::as_memory_mappable(&hidden).is_none());
        assert!(hidden.get_mapping_info(0, 4096).is_err());
    }

    #[test_case]
    fn gpu_child_objects_survive_connection_drop() {
        let backend: Arc<dyn GpuBackend> = Arc::new(TestBackend);
        let backend_lifetime = Arc::downgrade(&backend);
        let connection = GpuConnection::new(Arc::clone(&backend));
        let buffer = GpuBuffer::new(Arc::clone(&backend), 4096, GPU_BUFFER_FLAG_CPU_VISIBLE)
            .expect("GPU buffer allocation should succeed");
        let backend_buffer = buffer.backend_buffer();
        let backend_buffer_lifetime = Arc::downgrade(&backend_buffer);
        drop(backend_buffer);
        let timeline = Arc::new(GpuTimeline::new(Arc::clone(&backend), 0));
        let point = GpuTimelinePoint::new(Arc::clone(&timeline), 1);

        drop(connection);
        drop(backend);
        assert!(backend_lifetime.upgrade().is_some());
        assert!(backend_buffer_lifetime.upgrade().is_some());
        assert_eq!(buffer.backend_info().command_resource_token, 1);
        assert!(buffer.get_mapping_info(0, 4096).is_ok());
        timeline
            .signal(1)
            .expect("timeline should signal after root close");
        assert!(point.current_ready(ReadyInterest::read()).read);

        drop(buffer);
        assert!(backend_buffer_lifetime.upgrade().is_none());
        drop(point);
        drop(timeline);
        assert!(backend_lifetime.upgrade().is_none());
    }

    #[test_case]
    fn gpu_timeline_is_monotonic_with_sticky_failure() {
        let timeline = GpuTimeline::new(Arc::new(TestBackend), 2);
        assert_eq!(timeline.state(), (2, false));
        assert_eq!(timeline.signal(5), Ok(5));
        assert!(timeline.signal(4).is_err());
        assert_eq!(timeline.state(), (5, false));

        timeline.fail();
        assert_eq!(timeline.state(), (5, true));
        assert!(timeline.signal(6).is_err());
        assert_eq!(timeline.state(), (5, true));
    }

    #[test_case]
    fn gpu_timeline_points_are_level_ready_and_capability_limited() {
        let timeline = Arc::new(GpuTimeline::new(Arc::new(TestBackend), 1));
        let point = GpuTimelinePoint::new(Arc::clone(&timeline), 3);
        assert!(!point.current_ready(ReadyInterest::read()).read);
        assert!(GpuObject::as_control_ops(&point).is_none());
        assert!(GpuObject::as_memory_mappable(&point).is_none());
        assert!(GpuObject::as_selectable(&point).is_some());

        timeline
            .signal(2)
            .expect("lower timeline signal should succeed");
        assert!(!point.current_ready(ReadyInterest::read()).read);

        timeline
            .signal(3)
            .expect("timeline should reach point target");
        assert!(point.current_ready(ReadyInterest::read()).read);
        assert!(!point.current_ready(ReadyInterest::write()).write);

        let failed_timeline = Arc::new(GpuTimeline::new(Arc::new(TestBackend), 0));
        let failed_point = GpuTimelinePoint::new(Arc::clone(&failed_timeline), 99);
        failed_timeline.fail();
        assert!(failed_point.current_ready(ReadyInterest::read()).read);
        assert!(
            failed_point
                .current_ready(ReadyInterest {
                    read: false,
                    write: false,
                    except: true,
                })
                .except
        );
        assert!(timeline.control(super::GPU_BUFFER_QUERY_INFO, 0).is_err());
    }
}
