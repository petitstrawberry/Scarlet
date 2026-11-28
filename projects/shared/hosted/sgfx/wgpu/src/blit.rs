//! GPU color mip blits. Each pass reads and writes disjoint subresources;
//! filtering and UNORM conversion occur on the selected graphics device.
use super::*;

#[derive(Default)]
pub(super) struct Cache {
    pipelines: Vec<(raw::TextureFormat, Arc<raw::RenderPipeline>)>,
    samplers: Vec<(FilterMode, Arc<raw::Sampler>)>,
}

impl Resources {
    pub(super) fn encode_mip_blit(
        &mut self,
        encoder: &mut raw::CommandEncoder,
        source: &GpuTexture,
        source_mip: u32,
        destination: &GpuTexture,
        destination_mip: u32,
        filter: FilterMode,
    ) -> Result<()> {
        let pipeline = match self
            .blit
            .pipelines
            .iter()
            .find(|(format, _)| *format == destination.format)
        {
            Some((_, pipeline)) => Arc::clone(pipeline),
            None => {
                let pipeline = Arc::new(create_blit_pipeline(
                    self.context.raw_device(),
                    destination.format,
                    false,
                ));
                self.blit
                    .pipelines
                    .push((destination.format, Arc::clone(&pipeline)));
                pipeline
            }
        };
        let sampler = match self.blit.samplers.iter().find(|(mode, _)| *mode == filter) {
            Some((_, sampler)) => Arc::clone(sampler),
            None => {
                let sampler = Arc::new(self.context.raw_device().create_sampler(
                    &raw::SamplerDescriptor {
                        label: Some("sgfx mip blit sampler"),
                        mag_filter: filter_mode(filter),
                        min_filter: filter_mode(filter),
                        lod_max_clamp: 0.0,
                        ..Default::default()
                    },
                ));
                self.blit.samplers.push((filter, Arc::clone(&sampler)));
                sampler
            }
        };
        let source_view = source.texture.create_view(&raw::TextureViewDescriptor {
            label: Some("sgfx source mip"),
            base_mip_level: source_mip,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let destination_view = destination
            .texture
            .create_view(&raw::TextureViewDescriptor {
                label: Some("sgfx destination mip"),
                base_mip_level: destination_mip,
                mip_level_count: Some(1),
                ..Default::default()
            });
        let group = self
            .context
            .raw_device()
            .create_bind_group(&raw::BindGroupDescriptor {
                label: Some("sgfx mip blit resources"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    raw::BindGroupEntry {
                        binding: 0,
                        resource: raw::BindingResource::TextureView(&source_view),
                    },
                    raw::BindGroupEntry {
                        binding: 1,
                        resource: raw::BindingResource::Sampler(&sampler),
                    },
                ],
            });
        let mut pass = encoder.begin_render_pass(&raw::RenderPassDescriptor {
            label: Some("sgfx GPU mip blit"),
            color_attachments: &[Some(raw::RenderPassColorAttachment {
                view: &destination_view,
                resolve_target: None,
                ops: raw::Operations {
                    load: raw::LoadOp::Clear(raw::Color::TRANSPARENT),
                    store: raw::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
        Ok(())
    }
}
