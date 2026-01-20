mod bridge;

pub use bridge::SurfaceBridge;

use crate::event::{Event, EventDispatcher, FocusManager};
use crate::geometry::Size as UiSize;
use crate::traits::{RenderNode, View};
use std::boxed::Box;

/// App trait that users implement
pub trait App {
    type ViewType: View;

    fn build(&self) -> Self::ViewType;
}

/// Main application structure
pub struct Application<A: App> {
    app: A,
    bridge: SurfaceBridge,
    root_view: Option<A::ViewType>,
    root_node: Option<Box<dyn RenderNode>>,
    focus_manager: FocusManager,
}

impl<A: App> Application<A> {
    pub fn new(app: A) -> Result<Self, &'static str> {
        // Create actual SurfaceBridge
        let bridge = SurfaceBridge::new("com.example.app", "ScarletUI App", "", 800, 600)?;

        Ok(Self {
            app,
            bridge,
            root_view: None,
            root_node: None,
            focus_manager: FocusManager::new(),
        })
    }

    pub fn run(mut self) -> Result<(), &'static str> {
        // Build initial tree
        let root_view = self.app.build();
        self.root_node = Some(root_view.build());
        self.root_view = Some(root_view);

        // Initial layout and render
        let window_size = UiSize { width: 800.0, height: 600.0 };
        let constraints = crate::layout::LayoutConstraints::tight(window_size);
        self.root_node
            .as_mut()
            .unwrap()
            .layout(constraints);
        self.root_node.as_mut().unwrap().render();

        // Main event loop
        loop {
            // 1. Poll for events (with timeout for state-driven updates)
            if let Some(event) = self.bridge.next_event_timeout(16) {
                // 2. Create dispatcher and dispatch event (transient per event)
                {
                    let mut dispatcher =
                        EventDispatcher::new(
                            self.root_node.as_mut().unwrap().as_mut(),
                            &mut self.focus_manager
                        );
                    dispatcher.dispatch(&event);
                }
                // Dispatcher ends here, borrow released
            }

            // 2.5. Check for window resize
            if self.bridge.check_resize_pending() {
                // Mark root as dirty to trigger full relayout
                if let Some(ref mut root) = self.root_node {
                    root.mark_dirty(crate::dirty::DirtyFlags::LAYOUT);
                }
            }

            // 3. Check for dirty nodes
            if self.has_dirty_nodes() {
                // 4. Re-layout if needed (cascade from dirty roots)
                self.relayout_dirty();

                // 5. Render dirty nodes
                self.render_dirty();

                // 6. Present to screen
                if let Some(ref mut node) = self.root_node {
                    self.bridge.present(node.as_mut())?;
                }
            }
        }
    }

    fn has_dirty_nodes(&self) -> bool {
        // Walk tree checking is_dirty()
        self.root_node.as_ref().map(|n| self.check_dirty_recursive(n.as_ref())).unwrap_or(false)
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
                // Root is dirty, relayout everything with current window size
                let window_size = UiSize {
                    width: self.bridge.width as f32,
                    height: self.bridge.height as f32,
                };
                let constraints = crate::layout::LayoutConstraints::tight(window_size);
                root.layout(constraints);
            } else {
                // Check children for dirty nodes
                self.relayout_dirty_recursive(root);
            }
        }
    }

    fn relayout_dirty_recursive(&mut self, node: &mut dyn RenderNode) {
        // Check all children
        for i in 0..node.children().len() {
            if let Some(child) = node.children_mut().get_mut(i) {
                if child.is_dirty() {
                    // Re-layout this child with its current frame constraints
                    let frame = child.frame();
                    let size = UiSize {
                        width: frame.size.width,
                        height: frame.size.height
                    };
                    // Explicitly create constraints to avoid type inference issues
                    use crate::layout::LayoutConstraints;
                    let constraints = LayoutConstraints {
                        min: size,
                        max: size,
                    };
                    child.layout(constraints);
                }
                // Recurse into grandchildren
                self.relayout_dirty_recursive(child.as_mut());
            }
        }
    }

    fn render_dirty(&mut self) {
        // Render only dirty subtrees (cascade down)
        if let Some(ref mut root) = self.root_node {
            self.render_dirty_recursive(root);
        }
    }

    fn render_dirty_recursive(&mut self, node: &mut dyn RenderNode) {
        if node.is_dirty() {
            node.render();
        }

        // Recurse into children
        for child in node.children_mut().iter_mut() {
            self.render_dirty_recursive(child.as_mut());
        }
    }
}
