//! Core View trait and related types

use crate::graphics::{Canvas, Rect};
use crate::event::Event;
use crate::view::node::{ViewId, DirtyNotifier};
use crate::view::buffer::ViewBuffer;
use scarlet_std::boxed::Box;
use scarlet_std::vec::Vec;

/// Size constraints for layout
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub const ZERO: Size = Size::new(0, 0);
}

/// Trait for views that can receive and manage focus
///
/// Views like TextField implement this to handle focus gain/loss.
pub trait Focus {
    /// Called when the view gains focus
    fn on_focus_gain(&mut self) -> bool {
        false
    }

    /// Called when the view loses focus
    fn on_focus_loss(&mut self) -> bool {
        false
    }

    /// Check if this view currently has focus
    fn is_focused(&self) -> bool {
        false
    }
}

/// Trait for views that track hover state
///
/// Views like Button implement this for visual feedback.
///
/// This is separate from `View::update_hover_state` - views implement
/// this trait to advertise hover capability, and the framework calls
/// `update_hover_state` automatically on MouseMove.
pub trait Hoverable {
    /// Check if mouse is currently over the view
    fn is_hovered(&self) -> bool {
        false
    }
}

/// The core trait for all UI components
///
/// Views form a tree structure where each view can contain child views.
/// The framework handles layout, drawing, and event dispatch automatically.
///
/// # Event Propagation
///
/// Events follow a two-phase model:
/// 1. **Capture phase**: `on_event_capture()` called from root to target
/// 2. **Bubble phase**: `on_event()` called from target to root
///
/// # Lifecycle
///
/// 1. `layout()` - Called to determine size and position
/// 2. `draw()` - Called to render the view
/// 3. `on_event_capture()` / `on_event()` - Called during event dispatch
///
/// # Example
///
/// ```no_run
/// use scarlet_ui::{View, Canvas, Rect, Size, Event, Color};
///
/// struct MyView {
///     text: &'static str,
/// }
///
/// impl View for MyView {
///     fn layout(&mut self, available: Size) -> Size {
///         Size::new(100, 20) // Fixed size
///     }
///
///     fn draw(&self, canvas: &mut Canvas, frame: Rect) {
///         canvas.draw_text(frame.x, frame.y, self.text, Color::BLACK);
///     }
/// }
/// ```
pub trait View {
    /// Calculate the desired size given available space
    ///
    /// Returns the size this view wants to be.
    fn layout(&mut self, available: Size) -> Size;

    /// Draw the view within the given frame
    ///
    /// The frame is the actual rectangle allocated by the parent.
    fn draw(&self, canvas: &mut Canvas, frame: Rect);

    /// Called during capture phase (root → target)
    ///
    /// Return `true` to stop propagation and consume the event.
    /// This is useful for parent views that want to intercept events
    /// before they reach children.
    fn on_event_capture(&mut self, _event: &mut Event, _frame: Rect) -> bool {
        false
    }

    /// Called during bubble phase (target → root)
    ///
    /// Return `true` to stop propagation and consume the event.
    /// This is the primary event handling method for most views.
    fn on_event(&mut self, _event: &mut Event, _frame: Rect) -> bool {
        false
    }

    /// Get child views with their frames for hit-testing
    ///
    /// Returns an empty slice by default (leaf views).
    /// Container views should override this to return their children.
    fn children(&self) -> Vec<(&dyn View, Rect)> {
        Vec::new()
    }

    /// Get mutable child views with their frames
    fn children_mut(&mut self) -> Vec<(&mut dyn View, Rect)> {
        Vec::new()
    }

    /// Visit children without allocating.
    ///
    /// The visitor returns `true` to stop iteration early.
    ///
    /// Containers should override this to avoid per-call allocations.
    fn visit_children(&self, visitor: &mut dyn FnMut(&dyn View, Rect) -> bool) {
        for (child, frame) in self.children() {
            if visitor(child, frame) {
                break;
            }
        }
    }

    /// Visit mutable children without allocating.
    ///
    /// The visitor returns `true` to stop iteration early.
    ///
    /// Containers should override this to avoid per-call allocations.
    fn visit_children_mut(&mut self, visitor: &mut dyn FnMut(&mut dyn View, Rect) -> bool) {
        for (child, frame) in self.children_mut() {
            if visitor(child, frame) {
                break;
            }
        }
    }

    /// Flex factor for main-axis space distribution in stacks.
    ///
    /// - `0` (default): the view is laid out at its natural size.
    /// - `>0`: the view participates in distributing remaining space in `VStack`/`HStack`.
    fn flex_factor(&self) -> u32 {
        0
    }

    /// Check if this view needs to be redrawn
    fn needs_draw(&self) -> bool {
        false
    }

    /// Mark this view as needing redraw
    fn set_needs_draw(&mut self) {
        // Default: no-op (views can track their own dirty state)
    }

    /// Clear the needs_draw flag after drawing
    fn clear_needs_draw(&mut self) {
        // Default: no-op
    }

    /// Update hover state based on mouse position
    ///
    /// Called by the framework during MouseMove events to update hover state.
    /// Returns `true` if the hover state changed.
    ///
    /// Default implementation does nothing. Views that track hover state
    /// (like Button) should override this to update their internal state.
    fn update_hover_state(&mut self, _mouse_in_frame: bool) -> bool {
        false
    }

    /// Get this view's buffer (immutable)
    ///
    /// Returns None if the view doesn't have a buffer yet.
    fn buffer(&self) -> Option<&ViewBuffer> {
        None
    }

    /// Get this view's buffer (mutable)
    ///
    /// Returns None if the view doesn't have a buffer yet.
    fn buffer_mut(&mut self) -> Option<&mut ViewBuffer> {
        None
    }

    /// Ensure the view has a buffer, creating it if needed
    ///
    /// Returns Some(buffer) if the view has a buffer, None otherwise.
    /// Container views that don't need buffers return None.
    fn ensure_buffer(&mut self, width: u32, height: u32) -> Option<&mut ViewBuffer> {
        // Default implementation: return None (containers don't have buffers)
        // Views with buffers should override this to handle buffer creation/resizing
        self.buffer_mut()
    }

    /// Draw the view to its own buffer
    ///
    /// This is called by the framework to render the view.
    /// Default implementation calls the old draw() method.
    fn draw_to_buffer(&mut self) {
        if let Some(buffer) = self.buffer_mut() {
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

    /// Get this view's registry ID
    fn view_id(&self) -> Option<ViewId> {
        None
    }

    /// Set this view's registry ID
    fn set_view_id(&mut self, _id: ViewId) {}

    /// Set dirty notifier for communication with Window
    fn set_dirty_notifier(&mut self, _notifier: DirtyNotifier) {}
}

/// Type-erased boxed View for dynamic dispatch
pub type ViewBox = Box<dyn View>;

/// Extension trait for converting views into boxes
pub trait IntoViewBox {
    fn into_view_box(self) -> ViewBox;
}

impl<V: View + 'static> IntoViewBox for V {
    fn into_view_box(self) -> ViewBox {
        Box::new(self)
    }
}
