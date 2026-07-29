//! Private backend selection and dispatch for the application facade.

use alloc::rc::Rc;

use framebuffer::DisplaySurface;
use std::handle::HandleResult;

use crate::{Capabilities, Color, PipelineDesc, VertexClip4Color3, Viewport, virgl};

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
}

pub(crate) enum Image {
    Virgl(virgl::Image),
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
