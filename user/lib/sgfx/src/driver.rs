//! Private backend selection and dispatch for the application facade.

use alloc::{rc::Rc, vec::Vec};

use framebuffer::DisplaySurface;
#[cfg(feature = "std")]
use scarlet_os::handle::{HandleError, HandleResult};
#[cfg(not(feature = "std"))]
use std::handle::{HandleError, HandleResult};

use crate::{
    Capabilities, Color, PipelineDesc, PixelRect, SourceAlpha, VertexClip4Color3, Viewport, virgl,
};
#[cfg(feature = "std")]
use scarlet_os::ipc::SharedMemory;
#[cfg(not(feature = "std"))]
use std::ipc::SharedMemory;

pub(crate) enum Device {
    Virgl(Rc<virgl::Device>),
}

impl Device {
    pub(crate) fn open(path: &str) -> HandleResult<Self> {
        let backend = Rc::new(virgl::Device::open(path)?);
        Ok(Self::Virgl(backend))
    }

    pub(crate) fn capabilities(&self) -> Capabilities {
        match self {
            Self::Virgl(device) => device.capabilities(),
        }
    }

    pub(crate) fn create_context(&self) -> HandleResult<Context> {
        match self {
            Self::Virgl(device) => Ok(Context::Virgl(device.create_context()?)),
        }
    }
}

pub(crate) enum Context {
    Virgl(virgl::Context),
}

impl Context {
    pub(crate) fn create_image(&self, width: u32, height: u32) -> HandleResult<Image> {
        match self {
            Self::Virgl(context) => Ok(Image::Virgl(context.create_image(width, height)?)),
        }
    }

    pub(crate) fn create_sampled_bgra_texture(
        &self,
        width: u32,
        height: u32,
    ) -> HandleResult<Texture> {
        match self {
            Self::Virgl(context) => Ok(Texture::Virgl(
                context.create_sampled_bgra_texture(width, height)?,
            )),
        }
    }

    pub(crate) fn create_imported_bgra_texture(
        &self,
        shared_memory: &SharedMemory,
        width: u32,
        height: u32,
        shm_offset: usize,
        source_stride: u32,
    ) -> HandleResult<Texture> {
        match self {
            Self::Virgl(context) => Ok(Texture::Virgl(context.create_imported_bgra_texture(
                shared_memory,
                width,
                height,
                shm_offset,
                source_stride,
            )?)),
        }
    }

    pub(crate) fn upload_texture_bgra(
        &self,
        texture: &Texture,
        pixels: &[u8],
        source_stride: u32,
        damage: PixelRect,
    ) -> HandleResult<()> {
        match (self, texture) {
            (Self::Virgl(context), Texture::Virgl(texture)) => {
                context.upload_texture_bgra(texture, pixels, source_stride, damage)
            }
        }
    }

    pub(crate) fn transfer_imported_bgra_rect(
        &self,
        texture: &Texture,
        damage: PixelRect,
    ) -> HandleResult<()> {
        match (self, texture) {
            (Self::Virgl(context), Texture::Virgl(texture)) => {
                context.transfer_imported_bgra_rect(texture, damage)
            }
        }
    }

    pub(crate) fn release_texture(&self, texture: Texture) -> HandleResult<()> {
        match (self, texture) {
            (Self::Virgl(context), Texture::Virgl(texture)) => context.release_texture(texture),
        }
    }

    pub(crate) fn release_image(&self, image: Image) -> HandleResult<()> {
        match (self, image) {
            (Self::Virgl(context), Image::Virgl(image)) => context.release_image(image),
        }
    }

    pub(crate) fn create_pipeline(
        &self,
        image: &Image,
        description: PipelineDesc,
    ) -> HandleResult<Pipeline> {
        match (self, image) {
            (Self::Virgl(context), Image::Virgl(image)) => Ok(Pipeline::Virgl(
                context.create_pipeline(image, description)?,
            )),
        }
    }

    pub(crate) fn create_queue(&self) -> HandleResult<Queue> {
        match self {
            Self::Virgl(context) => Ok(Queue::Virgl(context.create_queue()?)),
        }
    }
}

pub(crate) enum Queue {
    Virgl(virgl::Queue),
}

impl Queue {
    pub(crate) fn submit(
        &self,
        image: &Image,
        viewport: Viewport,
        clear_color: Color,
        pipeline: &Pipeline,
        vertices: &[VertexClip4Color3],
    ) -> HandleResult<()> {
        match (self, image, pipeline) {
            (Self::Virgl(queue), Image::Virgl(image), Pipeline::Virgl(pipeline)) => {
                queue.submit(image, viewport, clear_color, pipeline, vertices)
            }
        }
    }

    pub(crate) fn submit_composition(
        &self,
        image: &Image,
        clear_color: Color,
        operations: &[CompositionOperation<'_>],
    ) -> HandleResult<()> {
        match (self, image) {
            (Self::Virgl(queue), Image::Virgl(image)) => {
                let mut backend_operations = Vec::new();
                backend_operations
                    .try_reserve(operations.len())
                    .map_err(|_| HandleError::OutOfResources)?;
                for operation in operations {
                    match operation {
                        CompositionOperation::Textured {
                            texture: Texture::Virgl(texture),
                            destination,
                            source,
                            opacity,
                            source_alpha,
                            clip,
                        } => backend_operations.push(virgl::CompositionOperation::Textured {
                            texture,
                            destination: *destination,
                            source: *source,
                            opacity: *opacity,
                            source_alpha: *source_alpha,
                            clip: *clip,
                        }),
                        CompositionOperation::Solid {
                            destination,
                            color,
                            clip,
                        } => backend_operations.push(virgl::CompositionOperation::Solid {
                            destination: *destination,
                            color: *color,
                            clip: *clip,
                        }),
                    }
                }
                queue.submit_composition(image, clear_color, &backend_operations)
            }
        }
    }
}

pub(crate) enum Image {
    Virgl(virgl::Image),
}

pub(crate) enum Texture {
    Virgl(virgl::Texture),
}

pub(crate) enum CompositionOperation<'a> {
    Textured {
        texture: &'a Texture,
        destination: PixelRect,
        source: PixelRect,
        opacity: f32,
        source_alpha: SourceAlpha,
        clip: Option<PixelRect>,
    },
    Solid {
        destination: PixelRect,
        color: Color,
        clip: Option<PixelRect>,
    },
}

impl Image {
    pub(crate) fn present(&self, display: &DisplaySurface) -> HandleResult<()> {
        match self {
            Self::Virgl(image) => image.present(display),
        }
    }
}

pub(crate) enum Pipeline {
    Virgl(virgl::Pipeline),
}
