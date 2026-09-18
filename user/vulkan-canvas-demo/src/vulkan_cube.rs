use std::error::Error;
use std::ffi::CStr;

use ash::{Entry, vk};

pub const IMAGE_WIDTH: u32 = 512;
pub const IMAGE_HEIGHT: u32 = 512;

const CLEAR: [f32; 4] = [0.025, 0.035, 0.06, 1.0];
const SCARLET_IMAGE_EXTENSION: &CStr = c"VK_SGFX_scarlet_image";
const SCARLET_IMAGE_EXPORT: &CStr = c"vkGetImageScarletHandleSGFX";

type ExportImageFn = unsafe extern "system" fn(vk::Device, vk::Image, *mut i32) -> vk::Result;

pub struct VulkanCube {
    resources: Resources,
    _entry: Entry,
    queue: vk::Queue,
    command_buffer: vk::CommandBuffer,
    fence: vk::Fence,
    uniform_memory: vk::DeviceMemory,
    color_image: vk::Image,
    submitted: bool,
}

impl VulkanCube {
    pub fn new() -> Result<(Self, i32), Box<dyn Error>> {
        let vertex_words =
            shader_words(include_bytes!(concat!(env!("OUT_DIR"), "/cube.vert.spv")))?;
        let fragment_words =
            shader_words(include_bytes!(concat!(env!("OUT_DIR"), "/cube.frag.spv")))?;
        let entry = vulkan_sgfx::linked_entry();

        unsafe {
            let application = vk::ApplicationInfo::default()
                .application_name(c"vulkan-canvas-demo")
                .api_version(vk::API_VERSION_1_0);
            let instance = entry.create_instance(
                &vk::InstanceCreateInfo::default().application_info(&application),
                None,
            )?;
            let mut resources = Resources::new(instance.clone());
            let (physical_device, queue_family) = select_device(&instance)?
                .ok_or("no SGFX Vulkan graphics device supports Scarlet image sharing")?;
            let properties = instance.get_physical_device_properties(physical_device);
            let device_name = CStr::from_ptr(properties.device_name.as_ptr()).to_string_lossy();
            println!("Vulkan physical device: {device_name}");

            let priorities = [1.0];
            let queue_infos = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(queue_family)
                .queue_priorities(&priorities)];
            let extension_names = [SCARLET_IMAGE_EXTENSION.as_ptr()];
            let device = instance
                .create_device(
                    physical_device,
                    &vk::DeviceCreateInfo::default()
                        .queue_create_infos(&queue_infos)
                        .enabled_extension_names(&extension_names),
                    None,
                )
                .map_err(|error| format!("vkCreateDevice failed: {error:?}"))?;
            resources.device = Some(device.clone());
            let queue = device.get_device_queue(queue_family, 0);
            let memory_properties = instance.get_physical_device_memory_properties(physical_device);
            let extent = vk::Extent3D {
                width: IMAGE_WIDTH,
                height: IMAGE_HEIGHT,
                depth: 1,
            };

            let color_image = device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::B8G8R8A8_UNORM)
                    .extent(extent)
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(
                        vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
                    )
                    .sharing_mode(vk::SharingMode::EXCLUSIVE)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )?;
            resources.images.push(color_image);
            let color_requirements = device.get_image_memory_requirements(color_image);
            let color_memory = device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(color_requirements.size)
                    .memory_type_index(memory_type(
                        &memory_properties,
                        color_requirements,
                        vk::MemoryPropertyFlags::DEVICE_LOCAL,
                    )?),
                None,
            )?;
            resources.memories.push(color_memory);
            device.bind_image_memory(color_image, color_memory, 0)?;
            let color_range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1);
            let color_view = device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(color_image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(vk::Format::B8G8R8A8_UNORM)
                    .subresource_range(color_range),
                None,
            )?;
            resources.views.push(color_view);

            let depth_image = device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::D32_SFLOAT)
                    .extent(extent)
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )?;
            resources.images.push(depth_image);
            let depth_requirements = device.get_image_memory_requirements(depth_image);
            let depth_memory = device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(depth_requirements.size)
                    .memory_type_index(memory_type(
                        &memory_properties,
                        depth_requirements,
                        vk::MemoryPropertyFlags::DEVICE_LOCAL,
                    )?),
                None,
            )?;
            resources.memories.push(depth_memory);
            device.bind_image_memory(depth_image, depth_memory, 0)?;
            let depth_view = device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(depth_image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(vk::Format::D32_SFLOAT)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::DEPTH)
                            .level_count(1)
                            .layer_count(1),
                    ),
                None,
            )?;
            resources.views.push(depth_view);

            let (vertex_buffer, _) = upload_buffer(
                &device,
                &memory_properties,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                &textured_vertices(),
                &mut resources,
            )?;
            let (index_buffer, _) = upload_buffer(
                &device,
                &memory_properties,
                vk::BufferUsageFlags::INDEX_BUFFER,
                &cube_indices(),
                &mut resources,
            )?;
            let (uniform_buffer, uniform_memory) = upload_buffer(
                &device,
                &memory_properties,
                vk::BufferUsageFlags::UNIFORM_BUFFER,
                &[vec![0; 256], transform(0.58)].concat(),
                &mut resources,
            )?;

            let attachments = [
                vk::AttachmentDescription::default()
                    .format(vk::Format::B8G8R8A8_UNORM)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .load_op(vk::AttachmentLoadOp::CLEAR)
                    .store_op(vk::AttachmentStoreOp::STORE)
                    .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                    .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .initial_layout(vk::ImageLayout::UNDEFINED)
                    .final_layout(vk::ImageLayout::GENERAL),
                vk::AttachmentDescription::default()
                    .format(vk::Format::D32_SFLOAT)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .load_op(vk::AttachmentLoadOp::CLEAR)
                    .store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                    .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .initial_layout(vk::ImageLayout::UNDEFINED)
                    .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
            ];
            let color_references = [vk::AttachmentReference::default()
                .attachment(0)
                .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
            let depth_reference = vk::AttachmentReference::default()
                .attachment(1)
                .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
            let subpasses = [vk::SubpassDescription::default()
                .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
                .color_attachments(&color_references)
                .depth_stencil_attachment(&depth_reference)];
            let dependencies = [vk::SubpassDependency::default()
                .src_subpass(vk::SUBPASS_EXTERNAL)
                .dst_subpass(0)
                .src_stage_mask(vk::PipelineStageFlags::TOP_OF_PIPE)
                .dst_stage_mask(
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                        | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
                )
                .dst_access_mask(
                    vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                        | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
                )];
            let render_pass = device.create_render_pass(
                &vk::RenderPassCreateInfo::default()
                    .attachments(&attachments)
                    .subpasses(&subpasses)
                    .dependencies(&dependencies),
                None,
            )?;
            resources.render_passes.push(render_pass);
            let framebuffer_attachments = [color_view, depth_view];
            let framebuffer = device.create_framebuffer(
                &vk::FramebufferCreateInfo::default()
                    .render_pass(render_pass)
                    .attachments(&framebuffer_attachments)
                    .width(IMAGE_WIDTH)
                    .height(IMAGE_HEIGHT)
                    .layers(1),
                None,
            )?;
            resources.framebuffers.push(framebuffer);

            let vertex_shader = device.create_shader_module(
                &vk::ShaderModuleCreateInfo::default().code(&vertex_words),
                None,
            )?;
            resources.shaders.push(vertex_shader);
            let fragment_shader = device.create_shader_module(
                &vk::ShaderModuleCreateInfo::default().code(&fragment_words),
                None,
            )?;
            resources.shaders.push(fragment_shader);

            let (texture_image, texture_view, sampler, staging) =
                create_texture(&device, &memory_properties, &mut resources)?;
            let mut bindings = vec![
                vk::DescriptorSetLayoutBinding::default()
                    .binding(0)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::VERTEX),
            ];
            bindings.push(
                vk::DescriptorSetLayoutBinding::default()
                    .binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            );
            bindings.push(
                vk::DescriptorSetLayoutBinding::default()
                    .binding(2)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            );
            let descriptor_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )?;
            resources.descriptor_layouts.push(descriptor_layout);
            let set_layouts = [descriptor_layout];
            let pipeline_layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts),
                None,
            )?;
            resources.pipeline_layouts.push(pipeline_layout);
            let pool_sizes = [
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC)
                    .descriptor_count(1),
            ];
            let descriptor_pool = device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&pool_sizes),
                None,
            )?;
            resources.descriptor_pools.push(descriptor_pool);
            let descriptor_set = device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&set_layouts),
            )?[0];
            let uniform_info = [vk::DescriptorBufferInfo::default()
                .buffer(uniform_buffer)
                .offset(0)
                .range(64)];
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(descriptor_set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC)
                    .buffer_info(&uniform_info)],
                &[],
            );

            let image_info = [vk::DescriptorImageInfo::default()
                .image_view(texture_view)
                .sampler(sampler)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            device.update_descriptor_sets(
                &[
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(1)
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(&image_info),
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(2)
                        .descriptor_type(vk::DescriptorType::SAMPLER)
                        .image_info(&image_info),
                ],
                &[],
            );
            let stages = [
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::VERTEX)
                    .module(vertex_shader)
                    .name(c"vs_main"),
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::FRAGMENT)
                    .module(fragment_shader)
                    .name(c"fs_main"),
            ];
            let vertex_bindings = [vk::VertexInputBindingDescription::default()
                .binding(0)
                .stride(32)
                .input_rate(vk::VertexInputRate::VERTEX)];
            let mut vertex_attributes = vec![
                vk::VertexInputAttributeDescription::default()
                    .location(0)
                    .binding(0)
                    .format(vk::Format::R32G32B32_SFLOAT)
                    .offset(0),
                vk::VertexInputAttributeDescription::default()
                    .location(1)
                    .binding(0)
                    .format(vk::Format::R32G32B32_SFLOAT)
                    .offset(12),
            ];
            vertex_attributes.push(
                vk::VertexInputAttributeDescription::default()
                    .location(2)
                    .binding(0)
                    .format(vk::Format::R32G32_SFLOAT)
                    .offset(24),
            );
            let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
                .vertex_binding_descriptions(&vertex_bindings)
                .vertex_attribute_descriptions(&vertex_attributes);
            let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
                .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
            let viewports = [vk::Viewport::default()
                .width(IMAGE_WIDTH as f32)
                .height(IMAGE_HEIGHT as f32)
                .min_depth(0.0)
                .max_depth(1.0)];
            let render_area = vk::Rect2D::default().extent(vk::Extent2D {
                width: IMAGE_WIDTH,
                height: IMAGE_HEIGHT,
            });
            let scissors = [render_area];
            let viewport = vk::PipelineViewportStateCreateInfo::default()
                .viewports(&viewports)
                .scissors(&scissors);
            let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
                .polygon_mode(vk::PolygonMode::FILL)
                .cull_mode(vk::CullModeFlags::NONE)
                .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
                .line_width(1.0);
            let multisample = vk::PipelineMultisampleStateCreateInfo::default()
                .rasterization_samples(vk::SampleCountFlags::TYPE_1);
            let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
                .blend_enable(false)
                .color_write_mask(
                    vk::ColorComponentFlags::R
                        | vk::ColorComponentFlags::G
                        | vk::ColorComponentFlags::B
                        | vk::ColorComponentFlags::A,
                )];
            let blend =
                vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
            let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
                .depth_test_enable(true)
                .depth_write_enable(true)
                .depth_compare_op(vk::CompareOp::LESS);
            let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
            let dynamic =
                vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
            let pipeline_infos = [vk::GraphicsPipelineCreateInfo::default()
                .dynamic_state(&dynamic)
                .stages(&stages)
                .vertex_input_state(&vertex_input)
                .input_assembly_state(&input_assembly)
                .viewport_state(&viewport)
                .rasterization_state(&rasterization)
                .multisample_state(&multisample)
                .color_blend_state(&blend)
                .depth_stencil_state(&depth_stencil)
                .layout(pipeline_layout)
                .render_pass(render_pass)
                .subpass(0)];
            let pipeline = device
                .create_graphics_pipelines(vk::PipelineCache::null(), &pipeline_infos, None)
                .map_err(|(_, error)| error)?[0];
            resources.pipelines.push(pipeline);

            let command_pool = device.create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(queue_family),
                None,
            )?;
            resources.command_pools.push(command_pool);
            let command_buffer = device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )?[0];
            let upload_command = device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )?[0];
            device.begin_command_buffer(
                upload_command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            upload_texture(&device, upload_command, texture_image, staging);
            device.end_command_buffer(upload_command)?;
            device
                .queue_submit(
                    queue,
                    &[vk::SubmitInfo::default().command_buffers(&[upload_command])],
                    vk::Fence::null(),
                )
                .map_err(|error| format!("texture upload vkQueueSubmit failed: {error:?}"))?;
            device
                .queue_wait_idle(queue)
                .map_err(|error| format!("texture upload vkQueueWaitIdle failed: {error:?}"))?;
            device.free_command_buffers(command_pool, &[upload_command]);
            device.begin_command_buffer(command_buffer, &vk::CommandBufferBeginInfo::default())?;
            device.cmd_bind_pipeline(command_buffer, vk::PipelineBindPoint::GRAPHICS, pipeline);
            device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline_layout,
                0,
                &[descriptor_set],
                &[256],
            );
            device.cmd_bind_vertex_buffers(command_buffer, 0, &[vertex_buffer], &[0]);
            device.cmd_bind_index_buffer(command_buffer, index_buffer, 0, vk::IndexType::UINT16);
            let clear_values = [
                vk::ClearValue {
                    color: vk::ClearColorValue { float32: CLEAR },
                },
                vk::ClearValue {
                    depth_stencil: vk::ClearDepthStencilValue {
                        depth: 1.0,
                        stencil: 0,
                    },
                },
            ];
            device.cmd_begin_render_pass(
                command_buffer,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(render_pass)
                    .framebuffer(framebuffer)
                    .render_area(render_area)
                    .clear_values(&clear_values),
                vk::SubpassContents::INLINE,
            );
            device.cmd_set_viewport(command_buffer, 0, &viewports);
            device.cmd_set_scissor(command_buffer, 0, &scissors);
            device.cmd_draw_indexed(command_buffer, 36, 1, 0, 0, 0);
            device.cmd_end_render_pass(command_buffer);
            device.end_command_buffer(command_buffer)?;
            let fence = device.create_fence(&vk::FenceCreateInfo::default(), None)?;
            resources.fences.push(fence);

            // Export is deliberately the final initialization operation. The
            // application adopts this owning raw handle immediately.
            let export = instance
                .get_device_proc_addr(device.handle(), SCARLET_IMAGE_EXPORT.as_ptr())
                .ok_or("vkGetImageScarletHandleSGFX is unavailable")?;
            let export: ExportImageFn = std::mem::transmute(export);
            let mut raw_handle = -1;
            let result = export(device.handle(), color_image, &mut raw_handle);
            if result != vk::Result::SUCCESS {
                return Err(format!("vkGetImageScarletHandleSGFX failed: {result:?}").into());
            }
            if raw_handle < 0 {
                return Err("vkGetImageScarletHandleSGFX returned an invalid handle".into());
            }

            Ok((
                Self {
                    resources,
                    _entry: entry,
                    queue,
                    command_buffer,
                    fence,
                    uniform_memory,
                    color_image,
                    submitted: false,
                },
                raw_handle,
            ))
        }
    }

    /// Validate actual GPU pixels once, before starting the shared-image UI loop.
    pub fn verify_readback(&mut self) -> Result<(), Box<dyn Error>> {
        let device = self
            .resources
            .device
            .as_ref()
            .ok_or("Vulkan device is unavailable")?
            .clone();
        unsafe {
            let physical = self.resources.instance.enumerate_physical_devices()?[0];
            let properties = self
                .resources
                .instance
                .get_physical_device_memory_properties(physical);
            let length = IMAGE_WIDTH as usize * IMAGE_HEIGHT as usize * 4;
            let (buffer, memory) = upload_buffer(
                &device,
                &properties,
                vk::BufferUsageFlags::TRANSFER_DST,
                &vec![0; length],
                &mut self.resources,
            )?;
            let pool = self.resources.command_pools[0];
            let command = device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )?[0];
            let range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1);
            device.begin_command_buffer(
                command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                    .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(self.color_image)
                    .subresource_range(range)],
            );
            device.cmd_copy_image_to_buffer(
                command,
                self.color_image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer,
                &[vk::BufferImageCopy::default()
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width: IMAGE_WIDTH,
                        height: IMAGE_HEIGHT,
                        depth: 1,
                    })],
            );
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                    .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                    .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(self.color_image)
                    .subresource_range(range)],
            );
            device.end_command_buffer(command)?;
            device.queue_submit(
                self.queue,
                &[vk::SubmitInfo::default().command_buffers(&[command])],
                vk::Fence::null(),
            )?;
            device.queue_wait_idle(self.queue)?;
            let mapped =
                device.map_memory(memory, 0, length as u64, vk::MemoryMapFlags::empty())?;
            let pixels = std::slice::from_raw_parts(mapped.cast::<u8>(), length).to_vec();
            device.unmap_memory(memory);
            device.free_command_buffers(pool, &[command]);
            let clear = |pixel: &[u8]| {
                pixel
                    .iter()
                    .zip([15u8, 9, 6, 255])
                    .all(|(&a, b)| a.abs_diff(b) <= 1)
            };
            let foreground = pixels.chunks_exact(4).filter(|pixel| !clear(pixel)).count();
            let colors = pixels
                .chunks_exact(4)
                .map(|pixel| [pixel[0], pixel[1], pixel[2]])
                .collect::<std::collections::BTreeSet<_>>();
            if pixels.chunks_exact(4).any(|pixel| pixel[3] != 255)
                || foreground < length / 40
                || foreground > length * 3 / 16
                || colors.len() < 64
                || !clear(&pixels[..4])
                || !clear(&pixels[length - 4..])
            {
                return Err(format!(
                    "textured GPU readback failed: {foreground} foreground pixels, {} colors",
                    colors.len()
                )
                .into());
            }
            println!(
                "PASS: Scarlet Vulkan textured cube GPU readback: {foreground} foreground pixels, {} colors; dynamic viewport/scissor, BGRA transfer",
                colors.len()
            );
            let mut tga = vec![0; 18];
            tga[2] = 2;
            tga[12..14].copy_from_slice(&(IMAGE_WIDTH as u16).to_le_bytes());
            tga[14..16].copy_from_slice(&(IMAGE_HEIGHT as u16).to_le_bytes());
            tga[16] = 32;
            tga[17] = 0x28;
            tga.extend_from_slice(&pixels);
            if let Err(error) = std::fs::write("/tmp/vulkan-textured-cube.tga", tga) {
                eprintln!("GPU capture could not be saved: {error}");
            }
        }
        Ok(())
    }

    pub fn render(&mut self, angle: f32) -> Result<(), Box<dyn Error>> {
        if !angle.is_finite() {
            return Err("cube angle must be finite".into());
        }
        let device = self
            .resources
            .device
            .as_ref()
            .ok_or("Vulkan device is unavailable")?;
        unsafe {
            let bytes = transform(angle);
            let mapped = device.map_memory(
                self.uniform_memory,
                256,
                bytes.len() as u64,
                vk::MemoryMapFlags::empty(),
            )?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.cast::<u8>(), bytes.len());
            device.unmap_memory(self.uniform_memory);

            if self.submitted {
                device.reset_fences(&[self.fence])?;
            }
            let command_buffers = [self.command_buffer];
            let submits = [vk::SubmitInfo::default().command_buffers(&command_buffers)];
            device.queue_submit(self.queue, &submits, self.fence)?;
            self.submitted = true;
            device.wait_for_fences(&[self.fence], true, u64::MAX)?;
        }
        Ok(())
    }
}

unsafe fn select_device(
    instance: &ash::Instance,
) -> Result<Option<(vk::PhysicalDevice, u32)>, vk::Result> {
    for physical_device in unsafe { instance.enumerate_physical_devices()? } {
        let extensions =
            unsafe { instance.enumerate_device_extension_properties(physical_device)? };
        let shares_scarlet_images = extensions.iter().any(|extension| unsafe {
            CStr::from_ptr(extension.extension_name.as_ptr()) == SCARLET_IMAGE_EXTENSION
        });
        if !shares_scarlet_images {
            continue;
        }
        if let Some((index, _)) =
            unsafe { instance.get_physical_device_queue_family_properties(physical_device) }
                .iter()
                .enumerate()
                .find(|(_, properties)| {
                    properties.queue_count > 0
                        && properties.queue_flags.contains(vk::QueueFlags::GRAPHICS)
                })
        {
            return Ok(Some((physical_device, index as u32)));
        }
    }
    Ok(None)
}

fn shader_words(bytes: &[u8]) -> Result<Vec<u32>, Box<dyn Error>> {
    if !bytes.len().is_multiple_of(4) {
        return Err("SPIR-V byte length is not word-aligned".into());
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|word| u32::from_le_bytes(word.try_into().expect("four-byte SPIR-V word")))
        .collect())
}

fn memory_type(
    properties: &vk::PhysicalDeviceMemoryProperties,
    requirements: vk::MemoryRequirements,
    flags: vk::MemoryPropertyFlags,
) -> Result<u32, Box<dyn Error>> {
    properties.memory_types[..properties.memory_type_count as usize]
        .iter()
        .enumerate()
        .find(|(index, memory)| {
            requirements.memory_type_bits & (1u32 << index) != 0
                && memory.property_flags.contains(flags)
        })
        .map(|(index, _)| index as u32)
        .ok_or_else(|| format!("no compatible Vulkan memory type with flags {flags:?}").into())
}

unsafe fn upload_buffer(
    device: &ash::Device,
    properties: &vk::PhysicalDeviceMemoryProperties,
    usage: vk::BufferUsageFlags,
    bytes: &[u8],
    resources: &mut Resources,
) -> Result<(vk::Buffer, vk::DeviceMemory), Box<dyn Error>> {
    let buffer = unsafe {
        device.create_buffer(
            &vk::BufferCreateInfo::default()
                .size(bytes.len() as u64)
                .usage(usage)
                .sharing_mode(vk::SharingMode::EXCLUSIVE),
            None,
        )?
    };
    resources.buffers.push(buffer);
    let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
    let memory = unsafe {
        device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(memory_type(
                    properties,
                    requirements,
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                )?),
            None,
        )?
    };
    resources.memories.push(memory);
    unsafe {
        device.bind_buffer_memory(buffer, memory, 0)?;
        let mapped =
            device.map_memory(memory, 0, bytes.len() as u64, vk::MemoryMapFlags::empty())?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.cast::<u8>(), bytes.len());
        device.unmap_memory(memory);
    }
    Ok((buffer, memory))
}

fn cube_vertices() -> Vec<u8> {
    let faces = [
        (
            [0.95f32, 0.24, 0.32],
            [
                [-1., -1., -1.],
                [1., -1., -1.],
                [1., 1., -1.],
                [-1., 1., -1.],
            ],
        ),
        (
            [0.18, 0.75, 0.98],
            [[1., -1., -1.], [1., -1., 1.], [1., 1., 1.], [1., 1., -1.]],
        ),
        (
            [0.64, 0.28, 0.92],
            [[1., -1., 1.], [-1., -1., 1.], [-1., 1., 1.], [1., 1., 1.]],
        ),
        (
            [0.13, 0.78, 0.49],
            [
                [-1., -1., 1.],
                [-1., -1., -1.],
                [-1., 1., -1.],
                [-1., 1., 1.],
            ],
        ),
        (
            [1.0, 0.72, 0.17],
            [[-1., 1., -1.], [1., 1., -1.], [1., 1., 1.], [-1., 1., 1.]],
        ),
        (
            [0.23, 0.38, 0.90],
            [
                [-1., -1., 1.],
                [1., -1., 1.],
                [1., -1., -1.],
                [-1., -1., -1.],
            ],
        ),
    ];
    let mut bytes = Vec::with_capacity(24 * 24);
    for (color, positions) in faces {
        for position in positions {
            for component in position.into_iter().chain(color) {
                bytes.extend_from_slice(&component.to_le_bytes());
            }
        }
    }
    bytes
}

unsafe fn create_texture(
    device: &ash::Device,
    properties: &vk::PhysicalDeviceMemoryProperties,
    resources: &mut Resources,
) -> Result<(vk::Image, vk::ImageView, vk::Sampler, vk::Buffer), Box<dyn Error>> {
    unsafe {
        let image = device.create_image(
            &vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(vk::Format::R8G8B8A8_UNORM)
                .extent(vk::Extent3D {
                    width: 32,
                    height: 32,
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
                .sharing_mode(vk::SharingMode::EXCLUSIVE),
            None,
        )?;
        resources.images.push(image);
        let requirements = device.get_image_memory_requirements(image);
        let memory = device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(memory_type(
                    properties,
                    requirements,
                    vk::MemoryPropertyFlags::DEVICE_LOCAL,
                )?),
            None,
        )?;
        resources.memories.push(memory);
        device.bind_image_memory(image, memory, 0)?;
        let view = device.create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(vk::Format::R8G8B8A8_UNORM)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .level_count(1)
                        .layer_count(1),
                ),
            None,
        )?;
        resources.views.push(view);
        let sampler = device.create_sampler(
            &vk::SamplerCreateInfo::default()
                .min_filter(vk::Filter::NEAREST)
                .mag_filter(vk::Filter::NEAREST)
                .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                .address_mode_u(vk::SamplerAddressMode::REPEAT)
                .address_mode_v(vk::SamplerAddressMode::REPEAT)
                .address_mode_w(vk::SamplerAddressMode::REPEAT)
                .min_lod(0.0)
                .max_lod(0.0),
            None,
        )?;
        resources.samplers.push(sampler);
        let (staging, _) = upload_buffer(
            device,
            properties,
            vk::BufferUsageFlags::TRANSFER_SRC,
            &checkerboard(),
            resources,
        )?;
        Ok((image, view, sampler, staging))
    }
}

unsafe fn upload_texture(
    device: &ash::Device,
    command: vk::CommandBuffer,
    image: vk::Image,
    staging: vk::Buffer,
) {
    unsafe {
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        device.cmd_pipeline_barrier(
            command,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .image(image)
                .subresource_range(range)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)],
        );
        device.cmd_copy_buffer_to_image(
            command,
            staging,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[vk::BufferImageCopy::default()
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width: 32,
                    height: 32,
                    depth: 1,
                })],
        );
        device.cmd_pipeline_barrier(
            command,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .image(image)
                .subresource_range(range)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)],
        );
    }
}

fn textured_vertices() -> Vec<u8> {
    cube_vertices()
        .chunks_exact(24)
        .enumerate()
        .flat_map(|(index, vertex)| {
            let uv = [[0.0f32, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]][index % 4];
            vertex
                .iter()
                .copied()
                .chain(uv.into_iter().flat_map(f32::to_le_bytes))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn checkerboard() -> Vec<u8> {
    (0..32)
        .flat_map(|y| {
            (0..32).flat_map(move |x| {
                if (x / 4 + y / 4) % 2 == 0 {
                    [240, 240, 240, 255]
                } else {
                    [24, (32 + y * 3) as u8, (64 + x * 3) as u8, 255]
                }
            })
        })
        .collect()
}

fn cube_indices() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(36 * 2);
    for face in 0..6u16 {
        let base = face * 4;
        for index in [base, base + 1, base + 2, base, base + 2, base + 3] {
            bytes.extend_from_slice(&index.to_le_bytes());
        }
    }
    bytes
}

fn transform(angle: f32) -> Vec<u8> {
    let (sin_y, cos_y) = angle.sin_cos();
    let (sin_x, cos_x) = (-0.42f32).sin_cos();
    let focal = 1.8;
    let near = 0.1;
    let far = 20.0;
    let depth_scale = far / (far - near);
    let model_columns = [
        [cos_y, sin_x * sin_y, -cos_x * sin_y, 0.0],
        [0.0, cos_x, sin_x, 0.0],
        [sin_y, -sin_x * cos_y, cos_x * cos_y, 0.0],
        [0.0, 0.0, 4.5, 1.0],
    ];
    model_columns
        .into_iter()
        .flat_map(|[x, y, z, w]| {
            [
                focal * x,
                focal * y,
                depth_scale * z - near * depth_scale * w,
                z,
            ]
        })
        .flat_map(f32::to_le_bytes)
        .collect()
}

struct Resources {
    instance: ash::Instance,
    device: Option<ash::Device>,
    buffers: Vec<vk::Buffer>,
    memories: Vec<vk::DeviceMemory>,
    images: Vec<vk::Image>,
    views: Vec<vk::ImageView>,
    shaders: Vec<vk::ShaderModule>,
    samplers: Vec<vk::Sampler>,
    descriptor_layouts: Vec<vk::DescriptorSetLayout>,
    descriptor_pools: Vec<vk::DescriptorPool>,
    pipeline_layouts: Vec<vk::PipelineLayout>,
    pipelines: Vec<vk::Pipeline>,
    render_passes: Vec<vk::RenderPass>,
    framebuffers: Vec<vk::Framebuffer>,
    command_pools: Vec<vk::CommandPool>,
    fences: Vec<vk::Fence>,
}

impl Resources {
    fn new(instance: ash::Instance) -> Self {
        Self {
            instance,
            device: None,
            buffers: Vec::new(),
            memories: Vec::new(),
            images: Vec::new(),
            views: Vec::new(),
            shaders: Vec::new(),
            samplers: Vec::new(),
            descriptor_layouts: Vec::new(),
            descriptor_pools: Vec::new(),
            pipeline_layouts: Vec::new(),
            pipelines: Vec::new(),
            render_passes: Vec::new(),
            framebuffers: Vec::new(),
            command_pools: Vec::new(),
            fences: Vec::new(),
        }
    }
}

impl Drop for Resources {
    fn drop(&mut self) {
        unsafe {
            if let Some(device) = &self.device {
                let _ = device.device_wait_idle();
                for &fence in &self.fences {
                    device.destroy_fence(fence, None);
                }
                for &pool in &self.command_pools {
                    device.destroy_command_pool(pool, None);
                }
                for &pipeline in &self.pipelines {
                    device.destroy_pipeline(pipeline, None);
                }
                for &framebuffer in &self.framebuffers {
                    device.destroy_framebuffer(framebuffer, None);
                }
                for &render_pass in &self.render_passes {
                    device.destroy_render_pass(render_pass, None);
                }
                for &pool in &self.descriptor_pools {
                    device.destroy_descriptor_pool(pool, None);
                }
                for &layout in &self.pipeline_layouts {
                    device.destroy_pipeline_layout(layout, None);
                }
                for &layout in &self.descriptor_layouts {
                    device.destroy_descriptor_set_layout(layout, None);
                }
                for &sampler in &self.samplers {
                    device.destroy_sampler(sampler, None);
                }
                for &shader in &self.shaders {
                    device.destroy_shader_module(shader, None);
                }
                for &view in &self.views {
                    device.destroy_image_view(view, None);
                }
                for &image in &self.images {
                    device.destroy_image(image, None);
                }
                for &buffer in &self.buffers {
                    device.destroy_buffer(buffer, None);
                }
                for &memory in &self.memories {
                    device.free_memory(memory, None);
                }
                device.destroy_device(None);
            }
            self.instance.destroy_instance(None);
        }
    }
}
