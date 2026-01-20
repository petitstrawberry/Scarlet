use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, HitResult};
use crate::geometry::{Point, Rect, Size};
use crate::layout::LayoutConstraints;
use crate::node_id::NodeId;
use crate::traits::{RenderNode, UpdateResult, View};
use std::any::Any;
use std::string::String;
use std::sync::Mutex;
use std::vec::Vec;

/// Simple image cache using a global mutex
struct ImageCacheState {
    entries: Vec<ImageEntry>,
    next_evict: usize,
}

struct ImageEntry {
    path: String,
    width: u32,
    height: u32,
    data: Vec<u8>, // RGBA
}

impl ImageCacheState {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_evict: 0,
        }
    }
}

const IMAGE_CACHE_CAP: usize = 32;

static IMAGE_CACHE: Mutex<ImageCacheState> = Mutex::new(ImageCacheState::new());

#[derive(Clone, PartialEq)]
pub struct Image {
    pub path: String,
    pub width: f32,
    pub height: f32,
}

impl Image {
    pub fn new(path: &str) -> Self {
        Self {
            path: String::from(path),
            width: 100.0,
            height: 100.0,
        }
    }

    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.width = width;
        self.height = height;
        self
    }
}

impl View for Image {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }

    fn type_name(&self) -> &'static str {
        "Image"
    }

    fn build(&self) -> std::boxed::Box<dyn RenderNode> {
        std::boxed::Box::new(ImageRenderNode::new(self.clone()))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct ImageRenderNode {
    id: NodeId,
    parent: Option<NodeId>,
    view: Image,
    buffer: Option<Buffer>,
    frame: Rect,
    dirty_flags: DirtyFlags,
}

impl ImageRenderNode {
    pub fn new(view: Image) -> Self {
        Self {
            id: NodeId::new(),
            parent: None,
            view,
            buffer: None,
            frame: Rect::ZERO,
            dirty_flags: DirtyFlags::PAINT | DirtyFlags::LAYOUT,
        }
    }

    fn load_image(&mut self) {
        // Check cache first
        let mut cache = IMAGE_CACHE.lock();
        for entry in &cache.entries {
            if entry.path == self.view.path {
                // Found in cache
                let mut buffer = Buffer::new(Size::new(entry.width as f32, entry.height as f32));
                buffer.as_mut_slice().copy_from_slice(&entry.data);
                self.buffer = Some(buffer);
                return;
            }
        }
        drop(cache);

        // Load from file
        if let Ok(mut file) = std::fs::File::open(&self.view.path) {
            let mut bytes = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = match file.read(&mut buf) {
                    Ok(n) => n,
                    Err(_) => break,
                };
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buf[..n]);
            }

            // Try to decode as PNG (simple implementation)
            // For now, just create a placeholder colored rectangle
            let (width, height) = (self.view.width as usize, self.view.height as usize);
            let mut buffer = Buffer::new(Size::new(width as f32, height as f32));

            // Create a placeholder pattern (checkerboard)
            let checker_size = 8;
            for y in 0..height {
                for x in 0..width {
                    let checker_x = x / checker_size;
                    let checker_y = y / checker_size;
                    let color = if (checker_x + checker_y) % 2 == 0 {
                        [200, 200, 200, 255]
                    } else {
                        [150, 150, 150, 255]
                    };
                    let idx = (y * width + x) * 4;
                    if idx + 3 < buffer.as_mut_slice().len() {
                        buffer.as_mut_slice()[idx..idx + 4].copy_from_slice(&color);
                    }
                }
            }

            self.buffer = Some(buffer);

            // Cache it
            let mut cache = IMAGE_CACHE.lock();
            if cache.entries.len() < IMAGE_CACHE_CAP {
                cache.entries.push(ImageEntry {
                    path: self.view.path.clone(),
                    width: width as u32,
                    height: height as u32,
                    data: buffer.as_slice().to_vec(),
                });
            }
        }
    }
}

impl RenderNode for ImageRenderNode {
    fn id(&self) -> NodeId {
        self.id
    }

    fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    fn set_parent(&mut self, parent: NodeId) {
        self.parent = Some(parent);
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Image>()
    }

    fn type_name(&self) -> &'static str {
        "Image"
    }

    fn try_update(&mut self, new_view: &dyn View) -> Option<UpdateResult> {
        new_view
            .as_any()
            .downcast_ref::<Image>()
            .map(|new_image| {
                if self.view != *new_image {
                    self.view = new_image.clone();
                    Some(UpdateResult::Changed(DirtyFlags::PAINT))
                } else {
                    Some(UpdateResult::Unchanged)
                }
            })
            .flatten()
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let size = Size::new(
            self.view.width.clamp(constraints.min.width, constraints.max.width),
            self.view.height.clamp(constraints.min.height, constraints.max.height),
        );
        self.frame = Rect::new(Point::ZERO, size);
        size
    }

    fn set_frame(&mut self, frame: Rect) {
        self.frame = frame;
    }

    fn frame(&self) -> Rect {
        self.frame
    }

    fn render(&mut self) {
        if !self.is_dirty() {
            return;
        }

        // Load or get cached image
        if self.buffer.is_none() {
            self.load_image();
        }

        // Resize buffer if needed
        if let Some(ref mut buffer) = self.buffer {
            if buffer.size() != self.frame.size {
                // For simplicity, just recreate at the right size
                self.load_image();
            }
        }

        self.clear_dirty();
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn hit_test(&self, point: Point) -> HitResult {
        if self.frame.contains(point) {
            HitResult::Handled(self.id)
        } else {
            HitResult::Passthrough
        }
    }

    fn handle_event(&mut self, _event: &Event, _ctx: &mut EventContext) {
        // Image doesn't handle events
    }

    fn mark_dirty(&mut self, flags: DirtyFlags) {
        self.dirty_flags |= flags;
    }

    fn is_dirty(&self) -> bool {
        !self.dirty_flags.is_empty()
    }

    fn clear_dirty(&mut self) {
        self.dirty_flags = DirtyFlags::empty();
    }
}
