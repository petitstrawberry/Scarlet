//! Pipeline module - Rendering pipeline orchestration
//!
//! This module provides the PipelineOwner and RenderingPipeline for orchestrating
//! the build, layout, and paint phases.

mod owner;
mod registry;
mod rendering;

pub use owner::{
    DirtyPhase, PipelineOwner, mark_element_dirty, mark_element_needs_paint,
    mark_element_needs_self_paint,
};
pub use registry::StateRegistry;
pub use rendering::RenderingPipeline;
