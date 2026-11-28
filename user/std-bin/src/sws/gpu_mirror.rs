//! A retained, sampleable copy of the source's completed GPU swapchain image.
//! Some backends use tiled scanout images which cannot be sampled directly.
use crate::compositor::scanout::mirror_viewport;
use crate::sgfx_ir_support::{Quad, SampledRect, define_bgra_texture};
use sgfx::ir::{self, Color, CommandBuffer, CommandEncoder, PixelRect, ResourceTable, TextureId};

pub(super) struct MirrorImage {
    pub source: usize,
    pub extent: (u32, u32),
    pub valid: bool,
    texture: TextureId,
}

impl MirrorImage {
    pub fn new(resources: &ResourceTable, source: usize, extent: (u32, u32)) -> ir::Result<Self> {
        Ok(Self {
            source,
            extent,
            valid: false,
            texture: define_bgra_texture(resources, extent.0, extent.1)?,
        })
    }

    pub fn copy_from<'a>(
        &self,
        resources: &'a ResourceTable,
        scanout: TextureId,
    ) -> ir::Result<CommandBuffer<'a, 'static>> {
        let area = PixelRect::new(0, 0, self.extent.0, self.extent.1)?;
        let mut encoder = CommandEncoder::new(resources);
        encoder.copy_texture_to_texture(
            resources.texture_ref(scanout)?,
            area,
            resources.texture_ref(self.texture)?,
            area,
        )?;
        encoder.finish()
    }

    pub fn quad(&self, target: (u32, u32)) -> ir::Result<Quad> {
        let (x, y, w, h) = mirror_viewport(self.extent, target);
        Ok(Quad::Sampled(SampledRect {
            texture: self.texture,
            texture_width: self.extent.0,
            texture_height: self.extent.1,
            destination: PixelRect::new(x, y, w, h)?,
            source: PixelRect::new(0, 0, self.extent.0, self.extent.1)?,
            tint: Color::rgba(1.0, 1.0, 1.0, 1.0)?,
            ignore_source_alpha: true,
            clip: None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sgfx::ir::{Command, Extent2D, TextureDesc, TextureFormat, TextureUsage};

    #[test]
    fn mirror_samples_the_completed_scanout_copy_including_its_last_pixel() {
        let resources = ResourceTable::new();
        let scanout = resources
            .define_texture(
                TextureDesc::new(
                    TextureFormat::Bgra8Unorm,
                    Extent2D::new(9, 5).unwrap(),
                    TextureUsage::RENDER_ATTACHMENT
                        | TextureUsage::PRESENT
                        | TextureUsage::COPY_SRC,
                )
                .unwrap(),
            )
            .unwrap()
            .id();
        let mirror = MirrorImage::new(&resources, 1, (9, 5)).unwrap();
        let copy = mirror.copy_from(&resources, scanout).unwrap();
        let [
            Command::CopyTextureToTexture {
                source,
                source_rect,
                destination,
                destination_rect,
            },
        ] = copy.commands()
        else {
            panic!("expected one GPU-to-GPU scanout copy")
        };
        let Quad::Sampled(quad) = mirror.quad((9, 9)).unwrap() else {
            panic!("expected a sampled mirror")
        };
        assert_eq!(source.id(), scanout);
        assert_eq!(destination.id(), quad.texture);
        assert_eq!(*source_rect, PixelRect::new(0, 0, 9, 5).unwrap());
        assert_eq!(destination_rect, source_rect);
        assert_eq!(quad.source, *source_rect);
        assert_eq!(quad.destination, PixelRect::new(0, 2, 9, 5).unwrap());
        assert!(quad.ignore_source_alpha);
    }
}
