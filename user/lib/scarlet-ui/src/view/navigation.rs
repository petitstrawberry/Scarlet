//! Navigation views - sidebar navigation with dynamic content

use super::traits::{View, ViewBox, Size};
use super::node::{ViewId, DirtyNotifier};
use super::buffer::ViewBuffer;
use crate::graphics::{Canvas, Rect};
use crate::event::Event;
use crate::color::Color;
use crate::{State, ViewRefreshHandle};
use scarlet_std::boxed::Box;
use scarlet_std::vec::Vec;
use scarlet_std::vec;
use scarlet_std::string::String;
use scarlet_std::format;
use core::cell::UnsafeCell;
use core::option::Option;

/// Navigation item for NavigationView sidebar
#[derive(Clone)]
pub struct NavigationItem {
    pub id: String,
    pub label: String,
    pub icon: Option<char>,
}

impl NavigationItem {
    pub fn new(id: &str, label: &str) -> Self {
        Self {
            id: String::from(id),
            label: String::from(label),
            icon: None,
        }
    }

    pub fn with_icon(mut self, icon: char) -> Self {
        self.icon = Some(icon);
        self
    }
}

/// Page content for NavigationView
pub trait NavigationPage: View {
    fn page_id(&self) -> &str;
}

/// NavigationView - sidebar navigation with dynamic content switching
///
/// Similar to macOS System Settings or Windows Settings sidebar navigation.
/// The sidebar shows navigation items, and clicking an item switches the content area.
///
/// # Example
///
/// ```no_run
/// use scarlet_ui::{NavigationView, NavigationItem, State, Label, VStack};
///
/// let selected_page = State::new(String::from("display"));
///
/// NavigationView::new(selected_page.clone())
///     .sidebar_width(200)
///     .items(&[
///         NavigationItem::new("display", "Display & Theme"),
///         NavigationItem::new("background", "Background"),
///     ])
///     .content(|id| match id {
///         "display" => Label::new("Display Settings").boxed(),
///         "background" => Label::new("Background Settings").boxed(),
///         _ => Label::new("Unknown").boxed(),
///     })
/// ```
pub struct NavigationView {
    selected_id: State<String>,
    items: Vec<NavigationItem>,
    content_builder: Box<dyn Fn(&str) -> ViewBox + Send + Sync>,
    sidebar_width: u32,
    sidebar_bg: Color,
    selected_bg: Color,
    text_color: Color,
    text_dim: Color,
    accent_color: Color,
    cached_size: Size,
    sidebar_frame: Rect,
    content_frame: Rect,
    needs_redraw: bool,
    needs_registry_rebuild: bool,  // Page changed, need to rebuild registry
    // Content cache to preserve state (hover, press, etc.)
    // Using UnsafeCell for interior mutability in draw() which takes &self
    cached_page_id: String,
    cached_content: UnsafeCell<Option<ViewBox>>,
    /// View ID for buffer management
    view_id: Option<ViewId>,
    /// Dirty notifier for buffer management
    dirty_notifier: Option<DirtyNotifier>,
    /// Buffer for sidebar and content rendering
    buffer: Option<ViewBuffer>,
}

impl NavigationView {
    pub fn new(selected_id: State<String>) -> Self {
        selected_id.subscribe_view(&ViewRefreshHandle::new());

        use crate::design::palette::*;

        Self {
            selected_id,
            items: Vec::new(),
            content_builder: Box::new(|_| Box::new(EmptyPage) as ViewBox),
            sidebar_width: 200,
            // macOS light theme using design palette
            sidebar_bg: SIDEBAR_BG,
            selected_bg: PRIMARY,
            text_color: TEXT_MAIN,
            text_dim: TEXT_SUB,
            accent_color: PRIMARY,
            cached_size: Size::ZERO,
            sidebar_frame: Rect::new(0, 0, 0, 0),
            content_frame: Rect::new(0, 0, 0, 0),
            needs_redraw: true,
            needs_registry_rebuild: false,
            cached_page_id: String::new(),
            cached_content: UnsafeCell::new(None),
            view_id: None,
            dirty_notifier: None,
            buffer: None,
        }
    }

    /// Set the sidebar width
    pub fn sidebar_width(mut self, width: u32) -> Self {
        self.sidebar_width = width;
        self
    }

    /// Set the navigation items
    pub fn items(mut self, items: &[NavigationItem]) -> Self {
        self.items = items.to_vec();
        self
    }

    /// Set the content builder function
    ///
    /// The function receives the selected page ID and returns the content view.
    pub fn content<F>(mut self, builder: F) -> Self
    where
        F: Fn(&str) -> ViewBox + Send + Sync + 'static,
    {
        self.content_builder = Box::new(builder);
        self
    }

    /// Set sidebar background color
    pub fn sidebar_bg(mut self, color: Color) -> Self {
        self.sidebar_bg = color;
        self
    }

    /// Set selected item background color
    pub fn selected_bg(mut self, color: Color) -> Self {
        self.selected_bg = color;
        self
    }

    /// Set text color
    pub fn text_color(mut self, color: Color) -> Self {
        self.text_color = color;
        self
    }

    /// Set dimmed text color
    pub fn text_dim(mut self, color: Color) -> Self {
        self.text_dim = color;
        self
    }

    /// Set accent color
    pub fn accent_color(mut self, color: Color) -> Self {
        self.accent_color = color;
        self
    }

    /// Build the current content view
    fn build_content(&self) -> ViewBox {
        let id = self.selected_id.get();
        (self.content_builder)(&id)
    }

    /// Get mutable access to cached content (preserves hover/press state!)
    fn get_content_mut(&mut self) -> &mut dyn View {
        let id = self.selected_id.get();

        // Rebuild if page changed or cache is empty
        let cache_ptr = self.cached_content.get();
        unsafe {
            let cache = &mut *cache_ptr;
            if cache.is_none() || self.cached_page_id != id {
                self.cached_page_id = id.clone();
                *cache = Some((self.content_builder)(&id));
            }
            cache.as_mut().unwrap().as_mut()
        }
    }

    /// Get cached content (mutable, for draw)
    /// Note: layout() requires &mut self, so we return &mut even for draw
    /// Uses UnsafeCell for interior mutability from &self
    fn get_content(&self) -> &mut dyn View {
        let id = self.selected_id.get();

        // Rebuild if page changed or cache is empty
        let cache_ptr = self.cached_content.get();
        unsafe {
            let cache = &mut *cache_ptr;
            if cache.is_none() || self.cached_page_id != id {
                // This is a bit hacky - we need &mut self to rebuild, but we only have &self
                // For now, if cache is empty when called from &self, we'll rebuild it
                // The page_id check won't work correctly from &self, but that's okay
                // because layout() always happens before draw() with &mut self
                *cache = Some((self.content_builder)(&id));
            }
            cache.as_mut().unwrap().as_mut()
        }
    }

    /// Draw sidebar item
    fn draw_item(
        &self,
        canvas: &mut Canvas,
        item: &NavigationItem,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        is_selected: bool,
        palette: &crate::design::Palette,
    ) {
        let text_y = y + (height as i32 - 16) / 2;

        // Fixed starting X position for text
        let mut text_x = x + 8;

        // Draw icon if present
        if let Some(icon) = item.icon {
            let icon_str = format!("{}", icon);
            canvas.draw_text_sized(
                x + 8,
                text_y,
                &icon_str,
                palette.text_main,
                14.0,
            );
            // Move text_x to after the icon (icon width ~16px + 4px spacing)
            text_x += 16;
        }

        // Draw label
        canvas.draw_text_sized(
            text_x,
            text_y,
            &item.label,
            if is_selected {
                palette.primary // Use accent color for selected text
            } else {
                palette.text_sub
            },
            14.0,
        );

        // Draw underline for selected item
        if is_selected {
            canvas.fill_rect(
                x,
                y + height as i32 - 2,
                width,
                2,
                palette.primary,
            );
        }
    }

    /// Handle click on sidebar
    fn handle_sidebar_click(&mut self, x: i32, y: i32) -> bool {
        if !self.sidebar_frame.contains(x, y) {
            return false;
        }

        let relative_y = y - self.sidebar_frame.y;
        let mut current_y = 0i32;
        const ITEM_HEIGHT: u32 = 36;
        const HEADER_HEIGHT: u32 = 40;
        const SIDEBAR_PADDING: u32 = 8;

        current_y += SIDEBAR_PADDING as i32 + HEADER_HEIGHT as i32 + 8;

        for item in &self.items {
            if relative_y >= current_y && relative_y < current_y + ITEM_HEIGHT as i32 {
                // Item clicked
                let old_id = self.selected_id.get();
                if old_id != item.id {
                    // Page is actually changing
                    self.selected_id.set(item.id.clone());
                    self.needs_redraw = true;
                    self.needs_registry_rebuild = true;  // Request registry rebuild

                    // Clear content cache so new page will be built
                    unsafe {
                        *self.cached_content.get() = None;
                    }

                    scarlet_std::println!("[NavigationView] Page changed: {} -> {}, clearing cache", old_id, item.id);
                }
                return true;
            }
            current_y += ITEM_HEIGHT as i32 + 2;
        }

        false
    }

    /// Check if registry needs to be rebuilt (page changed)
    pub fn needs_registry_rebuild(&self) -> bool {
        self.needs_registry_rebuild
    }

    /// Clear the registry rebuild flag
    pub fn clear_registry_rebuild(&mut self) {
        self.needs_registry_rebuild = false;
    }
}

struct EmptyPage;
impl View for EmptyPage {
    fn layout(&mut self, _available: Size) -> Size {
        Size::ZERO
    }
    fn draw(&self, _canvas: &mut Canvas, _frame: Rect) {}
}

impl View for NavigationView {
    fn layout(&mut self, available: Size) -> Size {
        // Sidebar takes fixed width
        let sidebar_width = self.sidebar_width.min(available.width);
        let content_width = available.width.saturating_sub(sidebar_width);

        // Layout sidebar
        self.sidebar_frame = Rect::new(0, 0, sidebar_width, available.height);

        // Layout content
        self.content_frame = Rect::new(
            sidebar_width as i32,
            0,
            content_width,
            available.height,
        );

        // Build and layout content (no padding - content pages handle their own padding)
        let content_available = Size::new(content_width, available.height);
        let mut content = self.build_content();
        let _ = content.layout(content_available);

        self.cached_size = Size::new(available.width, available.height);
        self.cached_size
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        // Get current palette for theme-aware colors
        let palette = crate::design::Palette::current();

        // Draw sidebar background
        canvas.fill_rect(
            frame.x,
            frame.y,
            self.sidebar_frame.width,
            self.sidebar_frame.height,
            palette.sidebar_bg,
        );

        // Draw sidebar header
        let header_text = "Settings";
        canvas.draw_text_sized(
            frame.x + 16,
            frame.y + 16,
            header_text,
            palette.text_main,
            20.0,
        );

        // Draw sidebar items
        let mut current_y = frame.y + 64;
        const ITEM_HEIGHT: u32 = 36;

        let selected_id = self.selected_id.get();

        for item in &self.items {
            let is_selected = item.id == selected_id;
            self.draw_item(
                canvas,
                item,
                frame.x,
                current_y,
                self.sidebar_frame.width,
                ITEM_HEIGHT,
                is_selected,
                palette,
            );
            current_y += ITEM_HEIGHT as i32 + 2;
        }

        // Draw content using cached instance to preserve hover/press state
        let content = self.get_content();

        let content_frame = Rect::new(
            frame.x + self.sidebar_frame.width as i32,
            frame.y,
            self.content_frame.width,
            self.content_frame.height,
        );
        // Layout content before drawing (CRITICAL: without this, cached_sizes are ZERO!)
        let content_size = Size::new(self.content_frame.width, self.content_frame.height);
        let _ = content.layout(content_size);
        content.draw(canvas, content_frame);
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        // Check for sidebar clicks (mouse up on left button)
        let sidebar_handled = if matches!(event.kind, crate::event::EventKind::MouseUp { button: crate::event::MouseButton::Left }) {
            self.handle_sidebar_click(event.x() - frame.x, event.y() - frame.y)
        } else {
            false
        };

        // Extract frame dimensions before mutable borrow
        let content_width = self.content_frame.width;
        let content_height = self.content_frame.height;

        // Forward to content (no padding - content pages handle their own padding)
        let content_frame = Rect::new(
            frame.x + self.sidebar_frame.width as i32,
            frame.y,
            content_width,
            content_height,
        );

        // Always forward events to content (needed for hover state updates)
        // Use get_content_mut() to get cached content with preserved state
        let content = self.get_content_mut();
        // Layout content before handling events (CRITICAL: without this, cached_sizes are ZERO!)
        let content_size = Size::new(content_width, content_height);
        let _ = content.layout(content_size);
        let content_handled = content.on_event(event, content_frame);

        sidebar_handled || content_handled
    }

    // Note: children() and children_mut() return empty because content is dynamically built
    fn children(&self) -> Vec<(&dyn View, Rect)> {
        vec![]
    }

    fn children_mut(&mut self) -> Vec<(&mut dyn View, Rect)> {
        vec![]
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&dyn View, Rect) -> bool) {
        // Extract frame dimensions before mutable borrow
        let sidebar_width = self.sidebar_frame.width;
        let content_width = self.content_frame.width;
        let content_height = self.content_frame.height;

        // Get cached content (uses UnsafeCell internally)
        let content = self.get_content();
        let content_frame = Rect::new(
            sidebar_width as i32,
            0,
            content_width,
            content_height,
        );
        // Layout content before visiting (to ensure cached_sizes are set)
        let content_size = Size::new(content_width, content_height);
        let _ = content.layout(content_size);
        let _ = visitor(content, content_frame);
    }

    fn visit_children_mut(&mut self, visitor: &mut dyn FnMut(&mut dyn View, Rect) -> bool) {
        // Extract frame dimensions before mutable borrow
        let sidebar_width = self.sidebar_frame.width;
        let content_width = self.content_frame.width;
        let content_height = self.content_frame.height;

        // Use get_content_mut() to get cached content with preserved state!
        let content = self.get_content_mut();
        let content_frame = Rect::new(
            sidebar_width as i32,
            0,
            content_width,
            content_height,
        );
        // Layout content before visiting
        let content_size = Size::new(content_width, content_height);
        let _ = content.layout(content_size);
        let _ = visitor(content, content_frame);
    }

    fn needs_draw(&self) -> bool {
        self.needs_redraw
    }

    fn set_needs_draw(&mut self) {
        self.needs_redraw = true;
    }

    fn clear_needs_draw(&mut self) {
        self.needs_redraw = false;
    }

    fn view_id(&self) -> Option<ViewId> {
        self.view_id
    }

    fn set_view_id(&mut self, id: ViewId) {
        self.view_id = Some(id);
    }

    fn set_dirty_notifier(&mut self, notifier: DirtyNotifier) {
        self.dirty_notifier = Some(notifier);
    }

    fn buffer(&self) -> Option<&ViewBuffer> {
        self.buffer.as_ref()
    }

    fn buffer_mut(&mut self) -> Option<&mut ViewBuffer> {
        self.buffer.as_mut()
    }

    fn ensure_buffer(&mut self, width: u32, height: u32) -> Option<&mut ViewBuffer> {
        if self.buffer.is_none() || self.buffer.as_ref().map(|b| (b.width(), b.height())) != Some((width, height)) {
            self.buffer = Some(ViewBuffer::new(width, height));
        }
        self.buffer.as_mut()
    }

    fn draw_to_buffer(&mut self) {
        if let Some(buffer) = self.buffer_mut() {
            // Clear buffer
            buffer.clear();

            let width = buffer.width();
            let height = buffer.height();
            let data = buffer.data_mut();
            let data_ptr = data.as_mut_ptr();
            let len = data.len();

            // Clear buffer
            for i in 0..len {
                unsafe {
                    *data_ptr.add(i) = 0;
                }
            }

            // Create canvas and draw
            let mut canvas = Canvas::new(unsafe { core::slice::from_raw_parts_mut(data_ptr, len) }, width, height);
            let frame = Rect::new(0, 0, width, height);
            self.draw(&mut canvas, frame);
        }
    }
}
