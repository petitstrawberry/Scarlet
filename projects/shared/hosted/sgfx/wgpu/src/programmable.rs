//! Programmable lowering uses explicit layouts and WGPU's automatic single-queue
//! resource transitions. IR barriers only occur between passes/copies, so pass
//! boundaries and ordered staging copies provide their availability/visibility.

use super::*;
use ir::{BindGroupDesc, BindGroupLayoutDesc, BindingResource, BindingType, ShaderSource};

#[derive(Default)]
pub(super) struct Cache {
    shaders: Vec<(ir::ShaderModuleId, Arc<raw::ShaderModule>)>,
    layouts: Vec<(BindGroupLayoutDesc, Arc<raw::BindGroupLayout>)>,
    render: Vec<(
        ir::ProgrammableRenderPipelineId,
        raw::TextureFormat,
        bool,
        Arc<RenderPipeline>,
    )>,
    compute: Vec<(ir::ComputePipelineId, Arc<ComputePipeline>)>,
    pub(super) groups: Vec<(ir::BindGroupId, Arc<raw::BindGroup>)>,
}

#[cfg(test)]
impl Cache {
    pub(super) fn cached_program_counts(&self) -> (usize, usize, usize, usize) {
        (
            self.shaders.len(),
            self.render.len(),
            self.compute.len(),
            self.layouts.len(),
        )
    }
}

pub(super) struct RenderPipeline {
    pub pipeline: raw::RenderPipeline,
    pub vertex_buffer_count: usize,
    pub has_depth: bool,
    pub empty_groups: Vec<Option<Arc<raw::BindGroup>>>,
    pub topology: ir::PrimitiveTopology,
}

pub(super) struct ComputePipeline {
    pipeline: raw::ComputePipeline,
    empty_groups: Vec<Option<Arc<raw::BindGroup>>>,
}

/// Native WGPU records validation synchronously; popping this scope is an
/// immediately-ready future, and does not wait for submitted GPU work.
pub(super) fn validated<T>(device: &raw::Device, create: impl FnOnce() -> Result<T>) -> Result<T> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        for filter in [
            raw::ErrorFilter::OutOfMemory,
            raw::ErrorFilter::Internal,
            raw::ErrorFilter::Validation,
        ] {
            device.push_error_scope(filter);
        }
        let result = create();
        let mut failure = None;
        for _ in 0..3 {
            let error = pollster::block_on(device.pop_error_scope());
            if failure.is_none() {
                failure = error;
            }
        }
        match failure {
            Some(error) => Err(Error::Validation(error.to_string())),
            None => result,
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = device;
        create()
    }
}

impl Resources {
    /// Release cached shader module objects; submitted WGPU work owns its resources.
    pub fn release_shader_module(&mut self, id: ir::ShaderModuleId) -> Result<()> {
        self.resources.shader_module_ref(id)?;
        self.programmable
            .shaders
            .retain(|(candidate, _)| *candidate != id);
        Ok(())
    }

    /// Release cached compute pipeline objects; submitted WGPU work owns its resources.
    pub fn release_compute_pipeline(&mut self, id: ir::ComputePipelineId) -> Result<()> {
        self.resources.compute_pipeline_ref(id)?;
        self.programmable
            .compute
            .retain(|(candidate, _)| *candidate != id);
        self.programmable.layouts.clear();
        Ok(())
    }

    /// Release cached programmable render pipeline objects; submitted WGPU work owns its resources.
    pub fn release_programmable_render_pipeline(
        &mut self,
        id: ir::ProgrammableRenderPipelineId,
    ) -> Result<()> {
        self.resources.programmable_render_pipeline_ref(id)?;
        self.programmable
            .render
            .retain(|(candidate, _, _, _)| *candidate != id);
        self.programmable.layouts.clear();
        Ok(())
    }

    /// Validate and materialize a shader module without submitting GPU work.
    /// Shader compilation failures are returned as [`Error::Validation`].
    pub fn validate_shader_module(&mut self, id: ir::ShaderModuleId) -> Result<()> {
        let validation = Arc::clone(&self.context.device.validation);
        let _guard = validation.lock().map_err(|_| Error::InvalidState)?;
        self.shader(id).map(|_| ())
    }

    /// Validate and cache a compute pipeline without submitting GPU work.
    pub fn validate_compute_pipeline(&mut self, id: ir::ComputePipelineId) -> Result<()> {
        let validation = Arc::clone(&self.context.device.validation);
        let _guard = validation.lock().map_err(|_| Error::InvalidState)?;
        self.compute_pipeline(id).map(|_| ())
    }

    /// Validate and cache a graphics pipeline using its logical target format.
    /// A remapped surface with a different physical format is validated again at use.
    pub fn validate_programmable_render_pipeline(
        &mut self,
        id: ir::ProgrammableRenderPipelineId,
    ) -> Result<()> {
        let validation = Arc::clone(&self.context.device.validation);
        let _guard = validation.lock().map_err(|_| Error::InvalidState)?;
        let desc = self
            .resources
            .programmable_render_pipeline(self.resources.programmable_render_pipeline_ref(id)?)?;
        let format = raw_format(desc.target_format())
            .ok_or(Error::Unsupported(UnsupportedFeature::TextureFormat))?;
        self.programmable_pipeline(id, format, desc.depth_state().is_some())
            .map(|_| ())
    }

    fn empty_groups(
        &mut self,
        desc: &ir::PipelineLayoutDesc,
    ) -> Result<Vec<Option<Arc<raw::BindGroup>>>> {
        desc.bind_groups()
            .iter()
            .map(|desc| {
                if !desc.entries().is_empty() {
                    return Ok(None);
                }
                let layout = self.programmable_layout(desc)?;
                validated(self.context.raw_device(), || {
                    Ok(Some(Arc::new(self.context.raw_device().create_bind_group(
                        &raw::BindGroupDescriptor {
                            label: Some("sgfx implicit empty bind group"),
                            layout: &layout,
                            entries: &[],
                        },
                    ))))
                })
            })
            .collect()
    }

    fn shader(&mut self, id: ir::ShaderModuleId) -> Result<Arc<raw::ShaderModule>> {
        self.resources.shader_module_ref(id)?;
        #[cfg(target_arch = "wasm32")]
        return Err(Error::Unsupported(UnsupportedFeature::ProgrammableOnWeb));
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some((_, shader)) = self
                .programmable
                .shaders
                .iter()
                .find(|(candidate, _)| *candidate == id)
            {
                return Ok(Arc::clone(shader));
            }
            let desc = self
                .resources
                .shader_module(self.resources.shader_module_ref(id)?)?;
            let source = match desc.source() {
                ShaderSource::Wgsl(source) => raw::ShaderSource::Wgsl(Cow::Borrowed(source)),
                ShaderSource::SpirV(source) => raw::ShaderSource::SpirV(Cow::Borrowed(source)),
            };
            let shader = Arc::new(validated(self.context.raw_device(), || {
                Ok(self
                    .context
                    .raw_device()
                    .create_shader_module(raw::ShaderModuleDescriptor {
                        label: Some("sgfx programmable shader"),
                        source,
                    }))
            })?);
            self.programmable.shaders.push((id, Arc::clone(&shader)));
            Ok(shader)
        }
    }

    fn programmable_layout(
        &mut self,
        desc: &BindGroupLayoutDesc,
    ) -> Result<Arc<raw::BindGroupLayout>> {
        if let Some((_, layout)) = self
            .programmable
            .layouts
            .iter()
            .find(|(candidate, _)| candidate == desc)
        {
            return Ok(Arc::clone(layout));
        }
        let entries = desc
            .entries()
            .iter()
            .map(|entry| {
                let mut visibility = raw::ShaderStages::empty();
                for (logical, physical) in [
                    (ir::ShaderStages::VERTEX, raw::ShaderStages::VERTEX),
                    (ir::ShaderStages::FRAGMENT, raw::ShaderStages::FRAGMENT),
                    (ir::ShaderStages::COMPUTE, raw::ShaderStages::COMPUTE),
                ] {
                    if entry.visibility().contains(logical) {
                        visibility |= physical;
                    }
                }
                let ty = match entry.ty() {
                    BindingType::UniformBuffer => raw::BindingType::Buffer {
                        ty: raw::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    BindingType::StorageBuffer { read_only } => raw::BindingType::Buffer {
                        ty: raw::BufferBindingType::Storage { read_only },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    BindingType::SampledTexture => raw::BindingType::Texture {
                        sample_type: raw::TextureSampleType::Float { filterable: true },
                        view_dimension: raw::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    BindingType::SampledTextureView { dimension, depth } => {
                        raw::BindingType::Texture {
                            sample_type: if depth {
                                raw::TextureSampleType::Depth
                            } else {
                                raw::TextureSampleType::Float { filterable: true }
                            },
                            view_dimension: texture_view_dimension(dimension),
                            multisampled: false,
                        }
                    }
                    BindingType::Sampler => {
                        raw::BindingType::Sampler(raw::SamplerBindingType::Filtering)
                    }
                    BindingType::ComparisonSampler => {
                        raw::BindingType::Sampler(raw::SamplerBindingType::Comparison)
                    }
                    BindingType::StorageTexture {
                        format,
                        access: ir::StorageTextureAccess::WriteOnly,
                    } => raw::BindingType::StorageTexture {
                        access: raw::StorageTextureAccess::WriteOnly,
                        format: raw_format(format)
                            .ok_or(Error::Unsupported(UnsupportedFeature::TextureFormat))?,
                        view_dimension: raw::TextureViewDimension::D2,
                    },
                };
                Ok(raw::BindGroupLayoutEntry {
                    binding: entry.binding(),
                    visibility,
                    ty,
                    count: None,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let layout = Arc::new(validated(self.context.raw_device(), || {
            Ok(self.context.raw_device().create_bind_group_layout(
                &raw::BindGroupLayoutDescriptor {
                    label: Some("sgfx programmable bind group layout"),
                    entries: &entries,
                },
            ))
        })?);
        self.programmable
            .layouts
            .push((desc.clone(), Arc::clone(&layout)));
        Ok(layout)
    }

    fn pipeline_layout(
        &mut self,
        desc: &ir::PipelineLayoutDesc,
        active: raw::ShaderStages,
    ) -> Result<raw::PipelineLayout> {
        let layouts = desc
            .bind_groups()
            .iter()
            .map(|desc| self.programmable_layout(desc))
            .collect::<Result<Vec<_>>>()?;
        let refs = layouts.iter().map(Arc::as_ref).collect::<Vec<_>>();
        let push_constant_ranges = desc
            .push_constant_ranges()
            .iter()
            .filter_map(|range| {
                let stages = shader_stages(range.stages()) & active;
                (!stages.is_empty()).then_some(raw::PushConstantRange {
                    stages,
                    range: range.offset()..range.offset() + range.size(),
                })
            })
            .collect::<Vec<_>>();
        validated(self.context.raw_device(), || {
            Ok(self
                .context
                .raw_device()
                .create_pipeline_layout(&raw::PipelineLayoutDescriptor {
                    label: Some("sgfx programmable pipeline layout"),
                    bind_group_layouts: &refs,
                    push_constant_ranges: &push_constant_ranges,
                }))
        })
    }

    pub(super) fn programmable_pipeline(
        &mut self,
        id: ir::ProgrammableRenderPipelineId,
        format: raw::TextureFormat,
        has_depth: bool,
    ) -> Result<Arc<RenderPipeline>> {
        self.resources.programmable_render_pipeline_ref(id)?;
        if let Some((_, _, _, pipeline)) =
            self.programmable
                .render
                .iter()
                .find(|(candidate, target, depth, _)| {
                    *candidate == id && *target == format && *depth == has_depth
                })
        {
            return Ok(Arc::clone(pipeline));
        }
        let desc = self
            .resources
            .programmable_render_pipeline(self.resources.programmable_render_pipeline_ref(id)?)?;
        validate_depth_bias(desc.raster())?;
        let locations = desc
            .vertex_buffers()
            .iter()
            .flat_map(|buffer| buffer.attributes())
            .filter(|attribute| attribute.format() == VertexFormat::Snorm10_10_10_2)
            .map(|attribute| attribute.location())
            .collect::<Vec<_>>();
        let vertex = if locations.is_empty() {
            self.shader(desc.vertex().module())?
        } else {
            let shader = self
                .resources
                .shader_module(self.resources.shader_module_ref(desc.vertex().module())?)?;
            let module = vertex_input::lower_packed_inputs(
                shader.source(),
                desc.vertex().entry_point(),
                &locations,
            )?;
            Arc::new(validated(self.context.raw_device(), || {
                Ok(self
                    .context
                    .raw_device()
                    .create_shader_module(raw::ShaderModuleDescriptor {
                        label: Some("sgfx packed vertex fetch"),
                        source: raw::ShaderSource::Naga(Cow::Owned(module)),
                    }))
            })?)
        };
        let fragment = self.shader(desc.fragment().module())?;
        let layout = self.pipeline_layout(
            desc.layout(),
            raw::ShaderStages::VERTEX | raw::ShaderStages::FRAGMENT,
        )?;
        if desc
            .vertex_buffers()
            .iter()
            .any(|layout| layout.instance_divisor() != 1)
        {
            return Err(Error::Unsupported(UnsupportedFeature::VertexDivisor));
        }
        let attributes = desc
            .vertex_buffers()
            .iter()
            .map(|layout| {
                layout
                    .attributes()
                    .iter()
                    .map(raw_vertex_attribute)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let vertex_buffers = desc
            .vertex_buffers()
            .iter()
            .zip(&attributes)
            .map(|(layout, attributes)| raw::VertexBufferLayout {
                array_stride: u64::from(layout.stride()),
                step_mode: match layout.step_mode() {
                    ir::VertexStepMode::Vertex => raw::VertexStepMode::Vertex,
                    ir::VertexStepMode::Instance => raw::VertexStepMode::Instance,
                },
                attributes,
            })
            .collect::<Vec<_>>();
        let targets = desc
            .color_targets()
            .enumerate()
            .map(|(slot, target)| {
                Ok(Some(raw::ColorTargetState {
                    format: if slot == 0 {
                        format
                    } else {
                        raw_format(target.format())
                            .ok_or(Error::Unsupported(UnsupportedFeature::TextureFormat))?
                    },
                    blend: Some(raw::BlendState {
                        color: blend_component(target.blend().color()),
                        alpha: blend_component(target.blend().alpha()),
                    }),
                    write_mask: raw::ColorWrites::from_bits_truncate(u32::from(
                        target.write_mask().bits(),
                    )),
                }))
            })
            .collect::<Result<Vec<_>>>()?;
        let pipeline = validated(self.context.raw_device(), || {
            Ok(self
                .context
                .raw_device()
                .create_render_pipeline(&raw::RenderPipelineDescriptor {
                    label: Some("sgfx programmable render pipeline"),
                    layout: Some(&layout),
                    vertex: raw::VertexState {
                        module: &vertex,
                        entry_point: Some(desc.vertex().entry_point()),
                        compilation_options: Default::default(),
                        buffers: &vertex_buffers,
                    },
                    fragment: Some(raw::FragmentState {
                        module: &fragment,
                        entry_point: Some(desc.fragment().entry_point()),
                        compilation_options: Default::default(),
                        targets: &targets,
                    }),
                    primitive: raw::PrimitiveState {
                        topology: match desc.topology() {
                            ir::PrimitiveTopology::TriangleList => {
                                raw::PrimitiveTopology::TriangleList
                            }
                            ir::PrimitiveTopology::TriangleStrip => {
                                raw::PrimitiveTopology::TriangleStrip
                            }
                            ir::PrimitiveTopology::LineList => raw::PrimitiveTopology::LineList,
                            ir::PrimitiveTopology::LineStrip => raw::PrimitiveTopology::LineStrip,
                            ir::PrimitiveTopology::PointList => raw::PrimitiveTopology::PointList,
                            ir::PrimitiveTopology::TriangleFan => {
                                return Err(Error::Unsupported(UnsupportedFeature::TriangleFan));
                            }
                        },
                        front_face: match desc.raster().front_face() {
                            ir::FrontFace::Clockwise => raw::FrontFace::Cw,
                            ir::FrontFace::CounterClockwise => raw::FrontFace::Ccw,
                        },
                        cull_mode: match desc.raster().cull_mode() {
                            ir::CullMode::None => None,
                            ir::CullMode::Front => Some(raw::Face::Front),
                            ir::CullMode::Back => Some(raw::Face::Back),
                        },
                        ..Default::default()
                    },
                    depth_stencil: desc
                        .depth_state()
                        .map(|depth| raw::DepthStencilState {
                            format: raw::TextureFormat::Depth32Float,
                            depth_write_enabled: depth.write_enabled(),
                            depth_compare: compare_function(depth.compare()),
                            stencil: Default::default(),
                            bias: raw_depth_bias(desc.raster()),
                        })
                        .or_else(|| {
                            has_depth.then_some(raw::DepthStencilState {
                                format: raw::TextureFormat::Depth32Float,
                                depth_write_enabled: false,
                                depth_compare: raw::CompareFunction::Always,
                                stencil: Default::default(),
                                bias: Default::default(),
                            })
                        }),
                    multisample: Default::default(),
                    multiview: None,
                    cache: None,
                }))
        })?;
        let pipeline = Arc::new(RenderPipeline {
            pipeline,
            vertex_buffer_count: desc.vertex_buffers().len(),
            has_depth,
            empty_groups: self.empty_groups(desc.layout())?,
            topology: desc.topology(),
        });
        self.programmable
            .render
            .push((id, format, has_depth, Arc::clone(&pipeline)));
        Ok(pipeline)
    }

    pub(super) fn compute_pipeline(
        &mut self,
        id: ir::ComputePipelineId,
    ) -> Result<Arc<ComputePipeline>> {
        self.resources.compute_pipeline_ref(id)?;
        if let Some((_, pipeline)) = self
            .programmable
            .compute
            .iter()
            .find(|(candidate, _)| *candidate == id)
        {
            return Ok(Arc::clone(pipeline));
        }
        let desc = self
            .resources
            .compute_pipeline(self.resources.compute_pipeline_ref(id)?)?;
        let shader = self.shader(desc.shader().module())?;
        let layout = self.pipeline_layout(desc.layout(), raw::ShaderStages::COMPUTE)?;
        let pipeline = validated(self.context.raw_device(), || {
            Ok(self
                .context
                .raw_device()
                .create_compute_pipeline(&raw::ComputePipelineDescriptor {
                    label: Some("sgfx compute pipeline"),
                    layout: Some(&layout),
                    module: &shader,
                    entry_point: Some(desc.shader().entry_point()),
                    compilation_options: Default::default(),
                    cache: None,
                }))
        })?;
        let pipeline = Arc::new(ComputePipeline {
            pipeline,
            empty_groups: self.empty_groups(desc.layout())?,
        });
        self.programmable.compute.push((id, Arc::clone(&pipeline)));
        Ok(pipeline)
    }

    pub(super) fn programmable_bind_group(
        &mut self,
        reference: ir::BindGroupRef<'_>,
    ) -> Result<Arc<raw::BindGroup>> {
        let desc: BindGroupDesc = self.resources.bind_group(reference)?;
        if let Some((_, group)) = self
            .programmable
            .groups
            .iter()
            .find(|(id, _)| *id == reference.id())
        {
            return Ok(Arc::clone(group));
        }
        let table = Rc::clone(&self.resources);
        let layout = self.programmable_layout(desc.layout())?;
        enum Resource {
            Buffer(Arc<GpuBuffer>, u64, u64),
            Texture(Arc<GpuTexture>),
            TextureView(raw::TextureView),
            Sampler(Arc<raw::Sampler>),
        }
        let mut resources = Vec::new();
        for entry in desc.entries() {
            let resource = match entry.resource() {
                BindingResource::Buffer {
                    buffer,
                    offset,
                    size,
                } => {
                    let ty = desc
                        .layout()
                        .entries()
                        .iter()
                        .find(|layout| layout.binding() == entry.binding())
                        .ok_or(Error::InvalidState)?
                        .ty();
                    let limits = self.context.raw_device().limits();
                    let (alignment, max_size) = match ty {
                        BindingType::UniformBuffer => (
                            limits.min_uniform_buffer_offset_alignment,
                            limits.max_uniform_buffer_binding_size,
                        ),
                        BindingType::StorageBuffer { .. } => (
                            limits.min_storage_buffer_offset_alignment,
                            limits.max_storage_buffer_binding_size,
                        ),
                        _ => return Err(Error::InvalidState),
                    };
                    if !offset.is_multiple_of(u64::from(alignment)) || size > u64::from(max_size) {
                        return Err(Error::Unsupported(UnsupportedFeature::BindingLimits));
                    }
                    Resource::Buffer(self.buffer(table.buffer_ref(buffer)?)?, offset, size)
                }
                BindingResource::Texture(id) => {
                    Resource::Texture(self.texture(table.texture_ref(id)?)?)
                }
                BindingResource::TextureView { texture, view } => {
                    if view.format() == ir::TextureFormat::Depth32Float
                        && desc.layout().entries().iter().any(|layout| {
                            layout.binding() == entry.binding()
                                && matches!(
                                    layout.ty(),
                                    BindingType::SampledTextureView { depth: false, .. }
                                )
                        })
                    {
                        return Err(Error::Unsupported(UnsupportedFeature::TextureFormat));
                    }
                    let texture = self.texture(table.texture_ref(texture)?)?;
                    Resource::TextureView(validated(self.context.raw_device(), || {
                        Ok(texture.texture.create_view(&raw::TextureViewDescriptor {
                            label: Some("sgfx sampled texture view"),
                            format: raw_format(view.format()),
                            dimension: Some(texture_view_dimension(view.dimension())),
                            base_mip_level: view.base_mip_level(),
                            mip_level_count: Some(view.mip_level_count()),
                            base_array_layer: view.base_array_layer(),
                            array_layer_count: Some(view.array_layer_count()),
                            ..Default::default()
                        }))
                    })?)
                }
                BindingResource::Sampler(id) => {
                    Resource::Sampler(self.sampler(table.sampler_ref(id)?)?)
                }
            };
            resources.push((entry.binding(), resource));
        }
        let entries = resources
            .iter()
            .map(|(binding, resource)| raw::BindGroupEntry {
                binding: *binding,
                resource: match resource {
                    Resource::Buffer(buffer, offset, size) => {
                        raw::BindingResource::Buffer(raw::BufferBinding {
                            buffer: &buffer.buffer,
                            offset: *offset,
                            size: core::num::NonZeroU64::new(*size),
                        })
                    }
                    Resource::Texture(texture) => {
                        raw::BindingResource::TextureView(&texture.sampled_view)
                    }
                    Resource::TextureView(view) => raw::BindingResource::TextureView(view),
                    Resource::Sampler(sampler) => raw::BindingResource::Sampler(sampler),
                },
            })
            .collect::<Vec<_>>();
        // Immutable descriptors retain their physical group until a presentation
        // mapping changes. map_image/unmap_image invalidate this cache.
        let group = validated(self.context.raw_device(), || {
            Ok(Arc::new(self.context.raw_device().create_bind_group(
                &raw::BindGroupDescriptor {
                    label: Some("sgfx programmable bind group"),
                    layout: &layout,
                    entries: &entries,
                },
            )))
        })?;
        self.programmable
            .groups
            .push((reference.id(), Arc::clone(&group)));
        Ok(group)
    }
}

impl Queue {
    pub(super) fn encode_compute_pass(
        &self,
        resources: &mut Resources,
        encoder: &mut raw::CommandEncoder,
        commands: &[Command<'_, '_>],
    ) -> Result<()> {
        let query_sets = self.query_sets(true)?;
        let mut pass = encoder.begin_compute_pass(&raw::ComputePassDescriptor {
            label: Some("sgfx compute pass"),
            timestamp_writes: None,
        });
        if let Some(set) = &query_sets.statistics {
            pass.begin_pipeline_statistics_query(set, 0);
        }
        let mut current_pipeline: Option<Arc<ComputePipeline>> = None;
        let mut bind_groups: Vec<(u32, Arc<raw::BindGroup>)> = Vec::new();
        for command in commands {
            match command {
                Command::SetComputePipeline(pipeline) => {
                    let pipeline = resources.compute_pipeline(pipeline.id())?;
                    pass.set_pipeline(&pipeline.pipeline);
                    current_pipeline = Some(pipeline);
                }
                Command::SetBindGroup { index, bind_group } => {
                    let group = resources.programmable_bind_group(*bind_group)?;
                    if let Some((_, current)) =
                        bind_groups.iter_mut().find(|(slot, _)| slot == index)
                    {
                        *current = group;
                    } else {
                        bind_groups.push((*index, group));
                    }
                }
                Command::SetPushConstants {
                    stages,
                    offset,
                    data,
                } => {
                    if stages.contains(ir::ShaderStages::COMPUTE) {
                        pass.set_push_constants(*offset, data);
                    }
                }
                Command::Dispatch { x, y, z } => {
                    let limit = self
                        .context
                        .raw_device()
                        .limits()
                        .max_compute_workgroups_per_dimension;
                    if [*x, *y, *z].into_iter().any(|value| value > limit) {
                        return Err(Error::Unsupported(UnsupportedFeature::DispatchLimits));
                    }
                    let pipeline = current_pipeline.as_ref().ok_or(Error::InvalidState)?;
                    for (index, empty) in pipeline.empty_groups.iter().enumerate() {
                        let group = bind_groups
                            .iter()
                            .find(|(slot, _)| *slot == index as u32)
                            .map(|(_, group)| group)
                            .or(empty.as_ref())
                            .ok_or(Error::InvalidState)?;
                        pass.set_bind_group(index as u32, group.as_ref(), &[]);
                    }
                    pass.dispatch_workgroups(*x, *y, *z);
                }
                _ => return Err(Error::InvalidState),
            }
        }
        if query_sets.statistics.is_some() {
            pass.end_pipeline_statistics_query();
        }
        drop(pass);
        self.resolve_queries(encoder, query_sets)?;
        Ok(())
    }
}

pub(super) fn shader_stages(stages: ir::ShaderStages) -> raw::ShaderStages {
    let mut raw_stages = raw::ShaderStages::empty();
    for (logical, raw_stage) in [
        (ir::ShaderStages::VERTEX, raw::ShaderStages::VERTEX),
        (ir::ShaderStages::FRAGMENT, raw::ShaderStages::FRAGMENT),
        (ir::ShaderStages::COMPUTE, raw::ShaderStages::COMPUTE),
    ] {
        if stages.contains(logical) {
            raw_stages |= raw_stage;
        }
    }
    raw_stages
}

fn texture_view_dimension(dimension: ir::TextureViewDimension) -> raw::TextureViewDimension {
    match dimension {
        ir::TextureViewDimension::D1 => raw::TextureViewDimension::D1,
        // D1-array allocations are explicitly rejected by texture materialization.
        ir::TextureViewDimension::D1Array => raw::TextureViewDimension::D2Array,
        ir::TextureViewDimension::D2 => raw::TextureViewDimension::D2,
        ir::TextureViewDimension::D2Array => raw::TextureViewDimension::D2Array,
        ir::TextureViewDimension::Cube => raw::TextureViewDimension::Cube,
    }
}
