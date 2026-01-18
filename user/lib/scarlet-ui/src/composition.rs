//! Composition and compositing for view rendering
//!
//! This module provides the Compositor, which is responsible for
//! compositing view buffers onto the final screen.

extern crate alloc;
use alloc::vec::Vec;

use crate::graphics::{Canvas, Rect, Point};
use crate::view::id::ViewId;
use crate::view::buffer::ViewBuffer;
use scarlet_std::collections::HashMap;
use scarlet_std::fmt;

/// Compositor for view buffers
///
/// The Compositor is responsible for compositing view buffers onto
/// the final screen. It handles:
/// - Layer ordering (z-index)
/// - Blending modes
/// - Clipping
/// - Transformations
pub struct Compositor {
    /// Buffers to composite, indexed by ViewId
    buffers: HashMap<ViewId, CompositorLayer>,
    /// Target canvas for compositing
    target: Option<Canvas<'static>>,
}

/// A layer in the composition pipeline
#[derive(Clone)]
pub struct CompositorLayer {
    /// View ID
    pub view_id: ViewId,
    /// Buffer to composite
    pub buffer: ViewBuffer,
    /// Position in the composition
    pub position: Point,
    /// Z-index (higher values are drawn on top)
    pub z_index: i32,
    /// Opacity (0.0 = fully transparent, 1.0 = fully opaque)
    pub opacity: f32,
    /// Whether this layer is clipped
    pub clip: Option<Rect>,
}

impl Compositor {
    /// Create a new compositor
    pub fn new() -> Self {
        Self {
            buffers: HashMap::new(),
            target: None,
        }
    }

    /// Set the target canvas
    pub fn set_target(&mut self, target: Canvas<'static>) {
        self.target = Some(target);
    }

    /// Add a layer to the composition
    pub fn add_layer(&mut self, layer: CompositorLayer) {
        self.buffers.insert(layer.view_id, layer);
    }

    /// Remove a layer from the composition
    pub fn remove_layer(&mut self, view_id: ViewId) -> Option<CompositorLayer> {
        self.buffers.remove(&view_id)
    }

    /// Update a layer in the composition
    pub fn update_layer(&mut self, layer: CompositorLayer) {
        self.buffers.insert(layer.view_id, layer);
    }

    /// Get a layer by ViewId
    pub fn get_layer(&self, view_id: ViewId) -> Option<&CompositorLayer> {
        self.buffers.get(&view_id)
    }

    /// Get a mutable layer by ViewId
    pub fn get_layer_mut(&mut self, view_id: ViewId) -> Option<&mut CompositorLayer> {
        self.buffers.get_mut(&view_id)
    }

    /// Composite all layers onto the target canvas
    ///
    /// Layers are composited in z-index order (lowest to highest).
    pub fn composite(&mut self) -> Result<(), CompositorError> {
        let target = self.target.as_mut().ok_or(CompositorError::NoTarget)?;

        // Sort layers by z-index and collect
        let mut layers: Vec<_> = self.buffers.values().cloned().collect();
        layers.sort_by_key(|l| l.z_index);

        // Composite each layer
        for layer in layers {
            composite_layer(target, &layer)?;
        }

        Ok(())
    }

    /// Clear all layers
    pub fn clear_layers(&mut self) {
        self.buffers.clear();
    }

    /// Get the number of layers
    pub fn layer_count(&self) -> usize {
        self.buffers.len()
    }
}

impl Default for Compositor {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Compositor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Compositor")
            .field("layer_count", &self.buffers.len())
            .field("has_target", &self.target.is_some())
            .finish()
    }
}

/// Composite a single layer onto the target canvas
fn composite_layer(
    target: &mut Canvas<'static>,
    layer: &CompositorLayer,
) -> Result<(), CompositorError> {
    // Apply clipping if needed
    if let Some(clip_rect) = layer.clip {
        // TODO: Implement clipping
        let _ = clip_rect;
    }

    // Apply opacity if needed
    if layer.opacity < 1.0 {
        // TODO: Implement alpha blending
    }

    // Composite the buffer
    // TODO: Implement actual blitting
    let _ = (target, layer);

    Ok(())
}

/// Errors that can occur during composition
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CompositorError {
    /// No target canvas has been set
    NoTarget,
    /// Invalid layer configuration
    InvalidLayer,
    /// Buffer size mismatch
    SizeMismatch,
}

impl CompositorLayer {
    /// Create a new compositor layer
    pub fn new(view_id: ViewId, buffer: ViewBuffer, position: Point) -> Self {
        Self {
            view_id,
            buffer,
            position,
            z_index: 0,
            opacity: 1.0,
            clip: None,
        }
    }

    /// Set the z-index
    pub fn with_z_index(mut self, z_index: i32) -> Self {
        self.z_index = z_index;
        self
    }

    /// Set the opacity
    pub fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity.clamp(0.0, 1.0);
        self
    }

    /// Set the clipping region
    pub fn with_clip(mut self, clip: Rect) -> Self {
        self.clip = Some(clip);
        self
    }

    /// Get the frame of this layer
    pub fn frame(&self) -> Rect {
        Rect::new(
            self.position.x,
            self.position.y,
            self.buffer.size().width,
            self.buffer.size().height,
        )
    }
}

impl fmt::Debug for CompositorLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompositorLayer")
            .field("view_id", &self.view_id)
            .field("position", &self.position)
            .field("z_index", &self.z_index)
            .field("opacity", &self.opacity)
            .field("clip", &self.clip)
            .field("size", &self.buffer.size())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compositor_new() {
        let compositor = Compositor::new();
        assert_eq!(compositor.layer_count(), 0);
        assert!(compositor.target.is_none());
    }

    #[test]
    fn test_compositor_add_layer() {
        let mut compositor = Compositor::new();
        let view_id = ViewId::new();
        let buffer = ViewBuffer::new(Size::new(100, 100));
        let layer = CompositorLayer::new(view_id, buffer, Point::new(0, 0));

        compositor.add_layer(layer);
        assert_eq!(compositor.layer_count(), 1);
    }

    #[test]
    fn test_compositor_remove_layer() {
        let mut compositor = Compositor::new();
        let view_id = ViewId::new();
        let buffer = ViewBuffer::new(Size::new(100, 100));
        let layer = CompositorLayer::new(view_id, buffer, Point::new(0, 0));

        compositor.add_layer(layer);
        assert_eq!(compositor.layer_count(), 1);

        compositor.remove_layer(view_id);
        assert_eq!(compositor.layer_count(), 0);
    }

    #[test]
    fn test_compositor_get_layer() {
        let mut compositor = Compositor::new();
        let view_id = ViewId::new();
        let buffer = ViewBuffer::new(Size::new(100, 100));
        let layer = CompositorLayer::new(view_id, buffer, Point::new(0, 0));

        compositor.add_layer(layer.clone());
        let retrieved = compositor.get_layer(view_id).unwrap();
        assert_eq!(retrieved.view_id, view_id);
    }

    #[test]
    fn test_compositor_clear_layers() {
        let mut compositor = Compositor::new();

        for _ in 0..5 {
            let view_id = ViewId::new();
            let buffer = ViewBuffer::new(Size::new(100, 100));
            let layer = CompositorLayer::new(view_id, buffer, Point::new(0, 0));
            compositor.add_layer(layer);
        }

        assert_eq!(compositor.layer_count(), 5);

        compositor.clear_layers();
        assert_eq!(compositor.layer_count(), 0);
    }

    #[test]
    fn test_compositor_layer_builder() {
        let view_id = ViewId::new();
        let buffer = ViewBuffer::new(Size::new(100, 100));

        let layer = CompositorLayer::new(view_id, buffer, Point::new(10, 20))
            .with_z_index(5)
            .with_opacity(0.8)
            .with_clip(Rect::new(0, 0, 50, 50));

        assert_eq!(layer.z_index, 5);
        assert_eq!(layer.opacity, 0.8);
        assert!(layer.clip.is_some());
    }

    #[test]
    fn test_compositor_layer_frame() {
        let view_id = ViewId::new();
        let buffer = ViewBuffer::new(Size::new(100, 50));
        let layer = CompositorLayer::new(view_id, buffer, Point::new(10, 20));

        let frame = layer.frame();
        assert_eq!(frame.x, 10);
        assert_eq!(frame.y, 20);
        assert_eq!(frame.width, 100);
        assert_eq!(frame.height, 50);
    }

    #[test]
    fn test_compositor_opacity_clamp() {
        let view_id = ViewId::new();
        let buffer = ViewBuffer::new(Size::new(100, 100));

        // Test upper bound
        let layer = CompositorLayer::new(view_id, buffer, Point::new(0, 0))
            .with_opacity(1.5);
        assert_eq!(layer.opacity, 1.0);

        // Test lower bound
        let view_id2 = ViewId::new();
        let buffer2 = ViewBuffer::new(Size::new(100, 100));
        let layer2 = CompositorLayer::new(view_id2, buffer2, Point::new(0, 0))
            .with_opacity(-0.5);
        assert_eq!(layer2.opacity, 0.0);
    }
}
