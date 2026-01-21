mod bridge;

pub use bridge::SurfaceBridge;

use crate::event::{Event, EventDispatcher, FocusManager, HoverManager, PressedManager};
use crate::geometry::{Color, Size as UiSize};
use crate::traits::{RenderNode, View};
use std::boxed::Box;
use std::vec::Vec;

/// App trait that users implement
pub trait App {
    type ViewType: View;

    fn build(&self) -> Self::ViewType;

    /// Called when window close is requested (e.g., close button clicked).
    /// Return true to allow the close, false to cancel it.
    /// This is called before the window is actually destroyed.
    fn on_request_close(&mut self) -> bool {
        true  // Default: allow close
    }
}

/// Main application structure
pub struct Application<A: App> {
    app: A,
    bridge: SurfaceBridge,
    root_view: Option<A::ViewType>,
    root_node: Option<Box<dyn RenderNode>>,
    focus_manager: FocusManager,
    hover_manager: HoverManager,
    pressed_manager: PressedManager,
    rebuild_requested: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Show debug colored borders around each View frame
    pub debug_frames: bool,
}

impl<A: App> Application<A> {
    pub fn new(app: A) -> Result<Self, &'static str> {
        // Create actual SurfaceBridge
        let bridge = SurfaceBridge::new("com.example.app", "ScarletUI App", "", 800, 600)?;

        let rebuild_requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        Ok(Self {
            app,
            bridge,
            root_view: None,
            root_node: None,
            focus_manager: FocusManager::new(),
            hover_manager: HoverManager::new(),
            pressed_manager: PressedManager::new(),
            rebuild_requested,
            debug_frames: false,
        })
    }

    pub fn run(mut self) -> Result<(), &'static str> {
        use std::println;  // For logging in no_std environment

        // Initialize theme automatically
        crate::theme::init();

        println!("[scarlet-ui] Application::run() starting");

        // Build initial tree
        let root_view = self.app.build();
        println!("[scarlet-ui] root_view built");
        self.root_node = Some(root_view.build());
        self.root_view = Some(root_view);

        // Subscribe to state changes for automatic rebuilding
        let rebuild_flag = self.rebuild_requested.clone();
        self.root_view.as_ref().unwrap().subscribe_states(std::sync::Arc::new(move || {
            println!("[app] State changed, requesting rebuild");
            rebuild_flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }));

        println!("[scarlet-ui] root_node built: {:?}", self.root_node.as_ref().map(|n| n.type_name()));

        // Initial layout and render
        let window_size = UiSize { width: 800.0, height: 600.0 };
        let constraints = crate::layout::LayoutConstraints::tight(window_size);

        println!("[scarlet-ui] calling layout()");
        self.root_node
            .as_mut()
            .unwrap()
            .layout(constraints);
        println!("[scarlet-ui] layout() complete, frame: {:?}", self.root_node.as_ref().map(|n| n.frame()));

        println!("[scarlet-ui] calling render()");
        self.root_node.as_mut().unwrap().render();
        println!("[scarlet-ui] render() complete");

        println!("[scarlet-ui] buffer: {:?}", self.root_node.as_ref().and_then(|n| n.get_buffer()).map(|b| (b.width(), b.height())));

        // Draw debug frames if enabled (initial render)
        if self.debug_frames {
            if let Some(ref mut node) = self.root_node {
                Self::draw_debug_frames_static(node.as_mut());
            }
        }

        // Present initial state to screen
        println!("[scarlet-ui] calling present()");
        if let Some(ref mut node) = self.root_node {
            self.bridge.present(node.as_mut())?;
        }
        println!("[scarlet-ui] present() complete");

        // Main event loop
        loop {
            // 1. Poll for events (with timeout for state-driven updates)
            if let Some(event) = self.bridge.next_event_timeout(16) {
                // 2. Create dispatcher and dispatch event (transient per event)
                {
                    let mut dispatcher =
                        EventDispatcher::new(
                            self.root_node.as_mut().unwrap().as_mut(),
                            &mut self.focus_manager,
                            &mut self.hover_manager,
                            &mut self.pressed_manager
                        );
                    dispatcher.dispatch(&event);
                }
                // Dispatcher ends here, borrow released
            }

            // 2.4. Check for Window requests (move, minimize, maximize, close)
            if self.handle_window_requests() {
                // Application requested exit (e.g., window closed)
                break Ok(());
            }

            // 2.5. Check for window resize
            if self.bridge.check_resize_pending() {
                // Mark root as dirty to trigger full relayout
                if let Some(ref mut root) = self.root_node {
                    root.mark_dirty(crate::dirty::DirtyFlags::LAYOUT);
                }
            }

            // 2.6. Check if window was destroyed
            if self.bridge.check_surface_destroyed() {
                std::println!("[app] Window destroyed, exiting application");
                break Ok(());  // Exit the event loop
            }

            // 2.7. Check for state changes and rebuild view if needed
            if self.rebuild_requested.load(std::sync::atomic::Ordering::Relaxed) {
                println!("[app] Rebuild requested, rebuilding view");
                self.rebuild_requested.store(false, std::sync::atomic::Ordering::Relaxed);

                // Rebuild view
                let new_view = self.app.build();
                self.root_view = Some(new_view);
                self.root_node = Some(self.root_view.as_ref().unwrap().build());

                // Re-subscribe to states
                let rebuild_flag = self.rebuild_requested.clone();
                self.root_view.as_ref().unwrap().subscribe_states(std::sync::Arc::new(move || {
                    println!("[app] State changed, requesting rebuild");
                    rebuild_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                }));

                // Trigger full layout and render
                let window_size = UiSize {
                    width: self.bridge.width as f32,
                    height: self.bridge.height as f32,
                };
                let constraints = crate::layout::LayoutConstraints::tight(window_size);
                self.root_node.as_mut().unwrap().layout(constraints);
                self.root_node.as_mut().unwrap().render();

                // Draw debug frames if enabled (after rebuild)
                if self.debug_frames {
                    if let Some(ref mut node) = self.root_node {
                        Self::draw_debug_frames_static(node.as_mut());
                    }
                }

                // Present immediately after rebuild
                if let Some(ref mut node) = self.root_node {
                    self.bridge.present(node.as_mut())?;
                }
            }

            // 3. Check for dirty nodes
            if self.has_dirty_nodes() {
                // 4. Re-layout if needed (cascade from dirty roots)
                self.relayout_dirty();

                // 5. Render dirty nodes
                self.render_dirty();

                // 5.5. Draw debug frames if enabled
                if self.debug_frames {
                    if let Some(ref mut node) = self.root_node {
                        Self::draw_debug_frames_static(node.as_mut());
                    }
                }

                // 6. Present to screen
                if let Some(ref mut node) = self.root_node {
                    self.bridge.present(node.as_mut())?;
                }
            }
        }
    }

    fn has_dirty_nodes(&self) -> bool {
        // Walk tree checking is_dirty()
        let dirty = self.root_node.as_ref().map(|n| self.check_dirty_recursive(n.as_ref())).unwrap_or(false);
        if dirty {
            std::println!("[app] has_dirty_nodes() = TRUE");
        }
        dirty
    }

    fn check_dirty_recursive(&self, node: &dyn RenderNode) -> bool {
        if node.is_dirty() {
            return true;
        }
        for child in node.children() {
            if self.check_dirty_recursive(child.as_ref()) {
                return true;
            }
        }
        false
    }

    fn relayout_dirty(&mut self) {
        // Re-layout from dirty roots (cascade down)
        if let Some(ref mut root) = self.root_node {
            if root.is_dirty() {
                // Root is dirty (includes window resize), relayout everything
                // Uses current window size from bridge (updated by SurfaceConfigure events)
                let window_size = UiSize {
                    width: self.bridge.width as f32,
                    height: self.bridge.height as f32,
                };
                let constraints = crate::layout::LayoutConstraints::tight(window_size);
                root.layout(constraints);
                // Root's layout() will recursively layout children with proper constraints
            } else {
                // Only children are dirty (not a resize), relayout with existing frames
                Self::relayout_dirty_recursive(root.as_mut());
            }
        }
    }

    fn relayout_dirty_recursive(node: &mut dyn RenderNode) {
        // Check all children
        for i in 0..node.children().len() {
            if let Some(child) = node.children_mut().get_mut(i) {
                if child.is_dirty() {
                    // Re-layout this child:
                    // First, measure with loose constraints to get natural size
                    use crate::layout::LayoutConstraints;
                    use crate::geometry::Size;

                    let frame = child.frame();
                    let loose = LayoutConstraints::loose(frame.size);
                    let natural_size = child.layout(loose);

                    // Then, layout with tight constraints using the natural size
                    let tight = LayoutConstraints::tight(natural_size);
                    child.layout(tight);
                }
                // Recurse into grandchildren
                Self::relayout_dirty_recursive(child.as_mut());
            }
        }
    }

    fn render_dirty(&mut self) {
        // Render dirty subtrees (post-order: children first, then parents)
        std::println!("[app] render_dirty() called");
        if let Some(ref mut root) = self.root_node {
            Self::render_dirty_recursive(root.as_mut());
        }
    }

    fn render_dirty_recursive(node: &mut dyn RenderNode) -> bool {
        // Returns true if this node or any descendant is dirty

        // First, recurse into children (post-order)
        let mut has_dirty_child = false;
        for child in node.children_mut().iter_mut() {
            if Self::render_dirty_recursive(child.as_mut()) {
                has_dirty_child = true;
            }
        }

        // Then render self if dirty or has dirty child
        let node_is_dirty = node.is_dirty();
        if node_is_dirty || has_dirty_child {
            std::println!("[app]   rendering node: {} (self_dirty={}, has_dirty_child={})",
                node.type_name(), node_is_dirty, has_dirty_child);
            node.render();
            return true;  // This node was rendered (dirty or dirty descendant)
        }

        false
    }

    fn draw_debug_frames_static(root: &mut dyn RenderNode) {
        use crate::geometry::{Point, Rect, Size};

        // Collect all frames with their depth (converting to root coordinates)
        let mut frames: Vec<(Rect, usize)> = Vec::new();
        Self::collect_frames_recursive(root, &mut frames, 0, Point::ZERO);

        // Get root buffer to draw on
        let buffer = match root.get_buffer_mut() {
            Some(b) => b,
            None => return,
        };

        // Draw borders for all collected frames
        let border_width = 1.0_f32;
        for (frame, depth) in frames.iter() {
            // Use color based on depth (cycle through colors)
            let border_color = match depth % 6 {
                0 => Color::RED,
                1 => Color::GREEN,
                2 => Color::BLUE,
                3 => Color::rgb(255, 255, 0),  // Yellow
                4 => Color::rgb(255, 0, 255),  // Magenta
                _ => Color::rgb(0, 255, 255),  // Cyan
            };

            // Draw borders on all four sides
            // Top
            buffer.fill_rect(Rect::new(
                Point::new(frame.origin.x, frame.origin.y),
                Size::new(frame.size.width, border_width),
            ), border_color.as_bgra());

            // Bottom
            buffer.fill_rect(Rect::new(
                Point::new(frame.origin.x, frame.origin.y + frame.size.height - border_width),
                Size::new(frame.size.width, border_width),
            ), border_color.as_bgra());

            // Left
            buffer.fill_rect(Rect::new(
                Point::new(frame.origin.x, frame.origin.y),
                Size::new(border_width, frame.size.height),
            ), border_color.as_bgra());

            // Right
            buffer.fill_rect(Rect::new(
                Point::new(frame.origin.x + frame.size.width - border_width, frame.origin.y),
                Size::new(border_width, frame.size.height),
            ), border_color.as_bgra());
        }
    }

    fn collect_frames_recursive(
        node: &dyn RenderNode,
        frames: &mut Vec<(crate::geometry::Rect, usize)>,
        depth: usize,
        parent_offset: crate::geometry::Point,
    ) {
        use crate::geometry::Point;

        let frame = node.frame();
        if frame.size.width > 0.0 && frame.size.height > 0.0 {
            // Convert frame to root coordinates by adding parent offset
            let root_frame = crate::geometry::Rect::new(
                Point::new(frame.origin.x + parent_offset.x, frame.origin.y + parent_offset.y),
                frame.size,
            );
            frames.push((root_frame, depth));

            // For children, add this node's origin to the offset
            let child_offset = Point::new(
                parent_offset.x + frame.origin.x,
                parent_offset.y + frame.origin.y,
            );

            // Recurse into children
            for child in node.children().iter() {
                Self::collect_frames_recursive(child.as_ref(), frames, depth + 1, child_offset);
            }
        }
    }

    fn handle_window_requests(&mut self) -> bool {
        use crate::containers::window::WindowRenderNode;

        // Handle requests for root Window node (if it's a Window)
        let root_ptr = match self.root_node.as_mut() {
            Some(r) => r.as_mut() as *mut dyn RenderNode,
            None => return false,
        };

        unsafe {
            let root_node = &mut *root_ptr;

            // Check if this node is a Window with pending requests
            if root_node.type_name() == "Window" {
                let window_node = &mut *(root_node as *mut dyn RenderNode as *mut WindowRenderNode);

                // Handle close first (with hook)
                if window_node.request_close {
                    window_node.request_close = false;
                    std::println!("[app] Window close requested, calling on_request_close() hook");

                    // Call user's hook - if it returns false, cancel the close
                    if self.app.on_request_close() {
                        std::println!("[app] on_request_close() returned true, proceeding with close");
                        let _ = self.bridge.close_window();
                        // Exit immediately after closing - don't wait for SurfaceDestroyed event
                        // The event may not arrive if the window server destroys the surface
                        std::println!("[app] Exiting application after close");
                        return true;  // Signal to exit event loop
                    } else {
                        std::println!("[app] on_request_close() returned false, canceling close");
                    }
                    return false;
                }

                // Handle move request
                if window_node.request_move {
                    window_node.request_move = false;
                    std::println!("[app] Starting window move");
                    let _ = self.bridge.request_move_window();
                }

                // Handle minimize
                if window_node.request_minimize {
                    window_node.request_minimize = false;
                    std::println!("[app] Minimizing window");
                    let _ = self.bridge.minimize_window();
                }

                // Handle maximize
                if window_node.request_maximize {
                    window_node.request_maximize = false;
                    std::println!("[app] Maximizing window");
                    let _ = self.bridge.maximize_window();
                }
            }
        }

        false
    }
}
