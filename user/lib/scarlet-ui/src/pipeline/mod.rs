//! Pipeline module - Rendering pipeline orchestration
//!
//! This module provides the PipelineOwner and RenderingPipeline for orchestrating
//! the build, layout, and paint phases.

mod owner;
mod rendering;

pub use owner::{PipelineOwner, DirtyPhase};
pub use rendering::RenderingPipeline;
