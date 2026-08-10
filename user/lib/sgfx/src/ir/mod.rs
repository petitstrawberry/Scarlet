//! Validated, backend-neutral logical graphics intermediate representation.
//!
//! Resources are descriptors held by [`ResourceTable`]. Commands recorded by
//! [`CommandEncoder`] borrow both that table and upload data, so a future
//! backend can lower the finished [`CommandBuffer`] without accepting forged
//! resource identities.

pub(crate) mod command;
mod pipeline;
pub(crate) mod resource;
mod types;

pub use command::{
    Command, CommandBuffer, CommandEncoder, DepthAttachment, DepthLoadOp, LoadOp, MAX_COMMANDS,
    RenderPassDesc, RenderPassEncoder, StoreOp,
};
pub use pipeline::{
    BlendComponent, BlendFactor, BlendOp, BlendState, CompareFunction, CullMode, DepthState,
    DrawUniforms, FragmentProgram, FrontFace, IndexFormat, MAX_VERTEX_ATTRIBUTES,
    PrimitiveTopology, RasterState, RenderPipelineDesc, TextureSampleMode, VertexAttribute,
    VertexBufferLayout, VertexFormat,
};
pub use resource::{
    AddressMode, BufferDesc, BufferId, BufferRef, BufferUsage, FilterMode, MAX_BUFFERS,
    MAX_RENDER_PIPELINES, MAX_SAMPLERS, MAX_TEXTURES, RenderPipelineId, RenderPipelineRef,
    ResourceTable, SamplerDesc, SamplerId, SamplerRef, TextureDesc, TextureFormat, TextureId,
    TextureRef, TextureUsage, TextureWrite,
};
pub use types::{Color, Error, Extent2D, PixelRect, Result, Transform};
