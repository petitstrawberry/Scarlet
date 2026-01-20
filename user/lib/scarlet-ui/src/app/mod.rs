use crate::event::{Event, EventDispatcher};
use crate::geometry::Size;
use crate::layout::LayoutConstraints;
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
}

impl<A: App> Application<A> {
    pub fn new(app: A) -> Result<Self, &'static str> {
        // TODO: Create actual SurfaceBridge
        // For now, use placeholder
        let bridge = SurfaceBridge::placeholder();

        Ok(Self {
            app,
            bridge,
            root_view: None,
            root_node: None,
        })
    }

    pub fn run(mut self) -> Result<(), &'static str> {
        // Build initial tree
        let root_view = self.app.build();
        self.root_node = Some(root_view.build());
        self.root_view = Some(root_view);

        // Initial layout and render
        let window_size = Size::new(800.0, 600.0);
        self.root_node
            .as_mut()
            .unwrap()
            .layout(LayoutConstraints::tight(window_size));
        self.root_node.as_mut().unwrap().render();

        // Main event loop
        loop {
            // 1. Poll for events (with timeout for state-driven updates)
            if let Some(event) = self.bridge.next_event_timeout(16) {
                // 2. Create dispatcher and dispatch event (transient per event)
                {
                    let mut dispatcher =
                        EventDispatcher::new(self.root_node.as_mut().unwrap().as_mut());
                    dispatcher.dispatch(&event);
                }
                // Dispatcher ends here, borrow released
            }

            // 3. Check for dirty nodes
            if self.has_dirty_nodes() {
                // 4. Re-layout if needed (cascade from dirty roots)
                self.relayout_dirty();

                // 5. Render dirty nodes
                self.render_dirty();

                // 6. Present to screen
                self.bridge.present()?;
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
        // Re-layout from dirty roots
        // TODO: Implement cascade
        if let Some(ref mut node) = self.root_node {
            if node.is_dirty() {
                let window_size = Size::new(800.0, 600.0);
                node.layout(LayoutConstraints::tight(window_size));
            }
        }
    }

    fn render_dirty(&mut self) {
        // Render only dirty subtrees
        if let Some(ref mut node) = self.root_node {
            if node.is_dirty() {
                node.render();
            }
        }
    }
}

/// Placeholder for SurfaceBridge
/// TODO: Implement actual connection to window server
pub struct SurfaceBridge {
    _private: (),
}

impl SurfaceBridge {
    pub fn placeholder() -> Self {
        Self { _private: () }
    }

    pub fn next_event_timeout(&self, _timeout_ms: u64) -> Option<Event> {
        // TODO: Connect to actual window server
        // For now, return None (no events)
        None
    }

    pub fn present(&mut self) -> Result<(), &'static str> {
        // TODO: Send buffer to window server
        Ok(())
    }

    pub fn set_root(&mut self, _node: &mut dyn RenderNode) {
        // TODO: Register root node with window server
    }
}
