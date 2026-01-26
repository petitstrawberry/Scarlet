//! Pipeline module - Rendering pipeline orchestration
//!
//! This module provides the PipelineOwner and RenderingPipeline for orchestrating
//! the build, layout, and paint phases.

mod owner;
mod rendering;
mod registry;

pub use owner::{PipelineOwner, DirtyPhase, mark_element_dirty, mark_element_needs_paint};
pub use rendering::RenderingPipeline;
pub use registry::StateRegistry;
