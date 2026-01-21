use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, HitResult};
use crate::geometry::{Point, Rect, Size};
use crate::layout::LayoutConstraints;
use crate::node_id::NodeId;
use crate::traits::{RenderNode, UpdateResult, View};
use std::any::Any;
use std::io::Read;
use std::string::String;
use std::sync::{Arc, Mutex};
use std::vec;
use std::vec::Vec;

/// Image handle representing a loaded image in the cache
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageHandle(u64);

impl ImageHandle {
    fn new() -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// Cached image data (rendering layer only)
struct CachedImage {
    handle: ImageHandle,
    width: u32,
    height: u32,
    data: Vec<u8>, // RGBA
}

/// Image cache (rendering layer only - no I/O)
struct ImageCacheState {
    entries: Vec<CachedImage>,
    next_evict: usize,
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

/// Image loader (I/O layer) - separate from rendering
pub struct ImageLoader;

impl ImageLoader {
    /// Load image from file and cache it (I/O operation)
    /// Call this during initialization/updates, NOT during render
    pub fn load(path: &str) -> Option<ImageHandle> {
        // Check if already cached
        let handle = Self::get_handle_for_path(path)?;
        if Self::is_cached(handle) {
            return Some(handle);
        }

        // Load from file (I/O)
        let mut file = std::fs::File::open(path).ok()?;
        let mut bytes = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = file.read(&mut buf).ok()?;
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buf[..n]);
        }

        // Decode image (placeholder - creates checkerboard)
        let (width, height) = (100, 100); // TODO: actual image decoding
        let mut data = vec![0u8; width * height * 4];

        // Create checkerboard placeholder
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
                data[idx..idx + 4].copy_from_slice(&color);
            }
        }

        // Add to cache
        Self::cache_image(handle, width as u32, height as u32, data);
        Some(handle)
    }

    fn get_handle_for_path(path: &str) -> Option<ImageHandle> {
        // Simple path-based handle lookup
        // In a real implementation, this would be a proper map
        let cache = IMAGE_CACHE.lock();
        for entry in &cache.entries {
            // TODO: store path in entry for lookup
        }
        Some(ImageHandle::new()) // Fallback: always create new handle
    }

    fn is_cached(handle: ImageHandle) -> bool {
        let cache = IMAGE_CACHE.lock();
        cache.entries.iter().any(|e| e.handle == handle)
    }

    fn cache_image(handle: ImageHandle, width: u32, height: u32, data: Vec<u8>) {
        let mut cache = IMAGE_CACHE.lock();
        if cache.entries.len() < IMAGE_CACHE_CAP {
            cache.entries.push(CachedImage {
                handle,
                width,
                height,
                data,
            });
        } else {
            // Evict oldest
            let idx = cache.next_evict % IMAGE_CACHE_CAP;
            cache.next_evict = cache.next_evict.wrapping_add(1);
            cache.entries[idx] = CachedImage {
                handle,
                width,
                height,
                data,
            };
        }
    }

    /// Get cached image data (rendering layer only - no I/O)
    pub fn get(handle: ImageHandle) -> Option<(u32, u32, Vec<u8>)> {
        let cache = IMAGE_CACHE.lock();
        cache.entries
            .iter()
            .find(|e| e.handle == handle)
            .map(|e| (e.width, e.height, e.data.clone()))
    }
}

#[derive(Clone, PartialEq)]
pub struct Image {
    pub handle: Option<ImageHandle>,  // Pre-loaded image handle (no I/O in render)
    pub width: f32,
    pub height: f32,
}

impl Image {
    /// Create a new Image with a pre-loaded handle
    /// Use ImageLoader::load() to get the handle first (I/O layer)
    pub fn with_handle(handle: ImageHandle) -> Self {
        Self {
            handle: Some(handle),
            width: 100.0,
            height: 100.0,
        }
    }

    /// Create a new Image that will be loaded later
    pub fn new(path: &str) -> Self {
        // Attempt to load immediately (I/O layer)
        let handle = ImageLoader::load(path);
        Self {
            handle,
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

    /// Render from cached data only (no I/O)
    fn render_from_cache(&mut self) -> bool {
        let handle = match self.view.handle {
            Some(h) => h,
            None => return false,
        };

        let (width, height, data) = match ImageLoader::get(handle) {
            Some(data) => data,
            None => return false,
        };

        let mut buffer = Buffer::new(Size::new(width as f32, height as f32));
        buffer.as_mut_slice().copy_from_slice(&data);
        self.buffer = Some(buffer);
        true
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

        // Render from cache only (no I/O during render)
        if self.buffer.is_none() {
            if !self.render_from_cache() {
                // Image not loaded yet - create placeholder
                let (width, height) = (self.view.width as usize, self.view.height as usize);
                let mut buffer = Buffer::new(Size::new(width as f32, height as f32));

                // Create placeholder pattern
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
                        let rect = Rect::new(
                            Point::new(x as f32, y as f32),
                            Size::new(1.0, 1.0)
                        );
                        buffer.fill_rect(rect, color);
                    }
                }

                self.buffer = Some(buffer);
            }
        }

        self.clear_dirty();
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn get_buffer_mut(&mut self) -> Option<&mut Buffer> {
        self.buffer.as_mut()
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
