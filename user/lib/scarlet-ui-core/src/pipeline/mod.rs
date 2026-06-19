//! Pipeline module - Rendering pipeline orchestration
//!
//! This module provides the PipelineOwner and RenderingPipeline for orchestrating
//! the build, layout, and paint phases.

mod owner;
mod registry;
mod rendering;

pub(crate) use owner::clear_global_dirty;
pub use owner::{
    DirtyPhase, MountContext, PipelineId, PipelineOwner, mark_element_dirty,
    mark_element_needs_layout, mark_element_needs_paint, mark_element_needs_self_paint,
};
pub use registry::StateRegistry;
pub use rendering::RenderingPipeline;
