//! Backend-neutral GPU rendering facade for Scarlet OS.
//!
//! This crate provides the small rendering API used by applications. The
//! selected driver backend and its command transport remain private so future
//! backends can preserve this application interface.

#![no_std]

extern crate alloc;
extern crate scarlet_std as std;

use alloc::{rc::Rc, vec::Vec};
use std::handle::HandleResult;

mod driver;
mod virgl;

/// Device capabilities expressed in application rendering terms.
#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    rendering: bool,
    presentation: bool,
}

impl Capabilities {
    /// Return whether the device can execute the built-in rendering pipeline.
    ///
    /// # Returns
    ///
    /// `true` when rendering commands are available.
    pub const fn supports_rendering(&self) -> bool {
        self.rendering
    }

    /// Return whether render-target images can be presented to a display.
    ///
    /// # Returns
    ///
    /// `true` when presentation is available.
    pub const fn supports_presentation(&self) -> bool {
        self.presentation
    }
}

/// Backend-neutral graphics device connection.
pub struct Device {
    backend: Rc<driver::Device>,
}

impl Device {
    /// Open a graphics device and select a compatible private backend.
    ///
    /// # Arguments
    ///
    /// * `path` - Device path such as `/dev/gpu0`.
    ///
    /// # Returns
    ///
    /// An opened graphics device or a handle error.
    pub fn open(path: &str) -> HandleResult<Self> {
        Ok(Self {
            backend: Rc::new(driver::Device::open(path)?),
        })
    }

    /// Return the rendering capabilities selected for this device.
    ///
    /// # Returns
    ///
    /// Application-level device capabilities.
    pub fn capabilities(&self) -> Capabilities {
        self.backend.capabilities()
    }

    /// Create an application rendering context.
    ///
    /// # Returns
    ///
    /// A context that owns render targets, pipelines, and queues.
    pub fn create_context(&self) -> HandleResult<Context> {
        Ok(Context {
            backend: self.backend.create_context()?,
        })
    }
}

/// Rendering context that owns application graphics objects.
pub struct Context {
    backend: driver::Context,
}

impl Context {
    /// Create a presentation-capable render-target image.
    ///
    /// # Arguments
    ///
    /// * `width` - Non-zero image width in pixels.
    /// * `height` - Non-zero image height in pixels.
    ///
    /// # Returns
    ///
    /// An image usable by render passes and display presentation.
    pub fn create_image(&self, width: u32, height: u32) -> HandleResult<Image> {
        Ok(Image {
            backend: self.backend.create_image(width, height)?,
            width,
            height,
        })
    }

    /// Create the built-in vertex-color triangle pipeline for one render target.
    ///
    /// # Arguments
    ///
    /// * `image` - Render target that the pipeline will draw into.
    /// * `description` - Pipeline kind and vertex capacity.
    ///
    /// # Returns
    ///
    /// A pipeline compatible with `image` or a handle error.
    pub fn create_pipeline(
        &self,
        image: &Image,
        description: PipelineDesc,
    ) -> HandleResult<Pipeline> {
        Ok(Pipeline {
            backend: Rc::new(self.backend.create_pipeline(&image.backend, description)?),
        })
    }

    /// Create a graphics queue for submitting render passes.
    ///
    /// # Returns
    ///
    /// A queue that executes render passes synchronously.
    pub fn create_queue(&self) -> HandleResult<Queue> {
        Ok(Queue {
            backend: self.backend.create_queue()?,
        })
    }
}

/// Queue that submits complete application render passes.
pub struct Queue {
    backend: driver::Queue,
}

impl Queue {
    /// Submit a render pass and wait for its rendering work to complete.
    ///
    /// # Arguments
    ///
    /// * `render_pass` - Render target, clear color, pipeline, and vertices to execute.
    ///
    /// # Returns
    ///
    /// Success or a handle error.
    pub fn submit(&self, render_pass: &RenderPass<'_>) -> HandleResult<()> {
        let draw = render_pass
            .draw
            .as_ref()
            .ok_or(std::handle::HandleError::InvalidParameter)?;
        self.backend.submit(
            &render_pass.image.backend,
            render_pass.viewport,
            render_pass.clear_color,
            draw.pipeline.as_ref(),
            &draw.vertices,
        )
    }
}

/// Renderable image that can be presented through a display surface.
pub struct Image {
    backend: driver::Image,
    width: u32,
    height: u32,
}

impl Image {
    /// Return the image width in pixels.
    ///
    /// # Returns
    ///
    /// The image width.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Return the image height in pixels.
    ///
    /// # Returns
    ///
    /// The image height.
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Present this image through a Scarlet display surface.
    ///
    /// # Arguments
    ///
    /// * `display` - Destination display surface.
    ///
    /// # Returns
    ///
    /// Success or a handle error.
    pub fn present(&self, display: &framebuffer::DisplaySurface) -> HandleResult<()> {
        self.backend.present(display)
    }
}

/// Built-in application pipeline kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineKind {
    /// Interleaved four-component clip-space position and RGB color vertices.
    ClipSpaceVertexColor,
}

/// Triangle face selection used by rasterization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CullMode {
    /// Rasterize both front-facing and back-facing triangles.
    None,
    /// Discard front-facing triangles.
    Front,
    /// Discard back-facing triangles.
    Back,
}

/// Winding direction that identifies front-facing triangles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontFace {
    /// Clockwise triangle winding is front-facing.
    Clockwise,
    /// Counter-clockwise triangle winding is front-facing.
    CounterClockwise,
}

/// Configuration for a built-in application pipeline.
#[derive(Debug, Clone, Copy)]
pub struct PipelineDesc {
    kind: PipelineKind,
    max_vertices: usize,
    cull_mode: CullMode,
    front_face: FrontFace,
}

impl PipelineDesc {
    /// Construct a clip-space vertex-color triangle pipeline description.
    ///
    /// # Arguments
    ///
    /// * `max_vertices` - Maximum vertices accepted in one render pass.
    ///
    /// # Returns
    ///
    /// A description for the built-in clip-space vertex-color pipeline.
    pub const fn clip_space_vertex_color(max_vertices: usize) -> Self {
        Self {
            kind: PipelineKind::ClipSpaceVertexColor,
            max_vertices,
            cull_mode: CullMode::None,
            front_face: FrontFace::CounterClockwise,
        }
    }

    /// Return the built-in pipeline kind.
    ///
    /// # Returns
    ///
    /// The pipeline kind selected by this description.
    pub const fn kind(&self) -> PipelineKind {
        self.kind
    }

    /// Return the maximum vertices accepted in one render pass.
    ///
    /// # Returns
    ///
    /// The pipeline vertex capacity.
    pub const fn max_vertices(&self) -> usize {
        self.max_vertices
    }

    /// Select the triangle faces discarded by rasterization.
    ///
    /// # Arguments
    ///
    /// * `cull_mode` - Face selection to discard.
    ///
    /// # Returns
    ///
    /// An updated pipeline description.
    pub const fn with_cull_mode(mut self, cull_mode: CullMode) -> Self {
        self.cull_mode = cull_mode;
        self
    }

    /// Select the winding direction treated as front-facing.
    ///
    /// # Arguments
    ///
    /// * `front_face` - Winding direction for visible front faces.
    ///
    /// # Returns
    ///
    /// An updated pipeline description.
    pub const fn with_front_face(mut self, front_face: FrontFace) -> Self {
        self.front_face = front_face;
        self
    }

    pub(crate) const fn cull_mode(&self) -> CullMode {
        self.cull_mode
    }

    pub(crate) const fn front_face(&self) -> FrontFace {
        self.front_face
    }
}

/// Built-in graphics pipeline state.
pub struct Pipeline {
    backend: Rc<driver::Pipeline>,
}

/// RGBA floating-point clear color.
#[derive(Debug, Clone, Copy)]
pub struct Color {
    red: f32,
    green: f32,
    blue: f32,
    alpha: f32,
}

impl Color {
    /// Construct an RGBA color.
    ///
    /// # Arguments
    ///
    /// * `red` - Red component.
    /// * `green` - Green component.
    /// * `blue` - Blue component.
    /// * `alpha` - Alpha component.
    ///
    /// # Returns
    ///
    /// The requested color.
    pub const fn rgba(red: f32, green: f32, blue: f32, alpha: f32) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }
}

/// Pixel dimensions used to map normalized coordinates into a render target.
#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    width: u32,
    height: u32,
}

impl Viewport {
    /// Construct a viewport covering a render target.
    ///
    /// # Arguments
    ///
    /// * `width` - Non-zero viewport width in pixels.
    /// * `height` - Non-zero viewport height in pixels.
    ///
    /// # Returns
    ///
    /// The requested viewport.
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Return the viewport width in pixels.
    ///
    /// # Returns
    ///
    /// The viewport width.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Return the viewport height in pixels.
    ///
    /// # Returns
    ///
    /// The viewport height.
    pub const fn height(&self) -> u32 {
        self.height
    }
}

/// One interleaved homogeneous clip-space position and RGB color vertex.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VertexClip4Color3 {
    clip_position: [f32; 4],
    color: [f32; 3],
}

impl VertexClip4Color3 {
    /// Construct an interleaved clip-space position and color vertex.
    ///
    /// # Arguments
    ///
    /// * `clip_position` - Homogeneous clip-space position before perspective division.
    /// * `color` - RGB vertex color.
    ///
    /// # Returns
    ///
    /// The requested vertex.
    pub const fn new(clip_position: [f32; 4], color: [f32; 3]) -> Self {
        Self {
            clip_position,
            color,
        }
    }
}

/// Complete description of one color render pass.
pub struct RenderPass<'a> {
    image: &'a Image,
    viewport: Viewport,
    clear_color: Color,
    draw: Option<Draw>,
}

struct Draw {
    pipeline: Rc<driver::Pipeline>,
    vertices: Vec<VertexClip4Color3>,
}

impl<'a> RenderPass<'a> {
    /// Begin a pass that clears and renders into one image.
    ///
    /// # Arguments
    ///
    /// * `image` - Render target for the pass.
    /// * `viewport` - Pixel area used for rendering.
    /// * `clear_color` - Background color written before drawing.
    ///
    /// # Returns
    ///
    /// A render pass ready to receive one built-in draw call.
    pub const fn new(image: &'a Image, viewport: Viewport, clear_color: Color) -> Self {
        Self {
            image,
            viewport,
            clear_color,
            draw: None,
        }
    }

    /// Set the clip-space vertex-color triangle draw for this pass.
    ///
    /// # Arguments
    ///
    /// * `pipeline` - Built-in clip-space vertex-color pipeline created for this image.
    /// * `vertices` - Triangle-list vertices to upload and rasterize.
    ///
    /// # Returns
    ///
    /// The updated render pass.
    pub fn draw_clip_space_vertex_color(
        &mut self,
        pipeline: &Pipeline,
        vertices: &[VertexClip4Color3],
    ) {
        self.draw = Some(Draw {
            pipeline: Rc::clone(&pipeline.backend),
            vertices: Vec::from(vertices),
        });
    }
}
