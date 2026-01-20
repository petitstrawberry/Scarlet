use crate::event::{Event, EventDispatcher, MouseEvent, MouseEventKind, KeyEvent, MouseButtons};
use crate::geometry::{Point, Size};
use crate::layout::LayoutConstraints;
use crate::traits::{RenderNode, View};
use sws_client::{Connection as SwsConnection, Event as SwsEvent};
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
        // Create actual SurfaceBridge
        let bridge = SurfaceBridge::new("com.example.app", "ScarletUI App", "", 800, 600)?;

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

/// Bridge to the window server (SWS)
pub struct SurfaceBridge {
    connection: SwsConnection,
    surface_id: u32,
    width: u32,
    height: u32,
    mouse_pos: Point,
    mouse_buttons: u8,
}

impl SurfaceBridge {
    pub fn new(app_id: &str, app_name: &str, menu_titles: &str, width: u32, height: u32) -> Result<Self, &'static str> {
        let mut connection = SwsConnection::connect_default()
            .map_err(|_| "Failed to connect to window server")?;

        let surface_id = connection
            .create_surface(app_id, app_name, menu_titles, width, height)
            .map_err(|_| "Failed to create surface")?;

        Ok(Self {
            connection,
            surface_id,
            width,
            height,
            mouse_pos: Point::ZERO,
            mouse_buttons: 0,
        })
    }

    pub fn next_event_timeout(&mut self, _timeout_ms: u64) -> Option<Event> {
        // Dispatch events from server
        let _ = self.connection.dispatch();

        // Convert SWS events to ScarletUI events
        loop {
            match self.connection.poll_event() {
                None => return None,
                Some(sws_event) => {
                    // Convert to ScarletUI event
                    if let Some(event) = self.convert_event(sws_event) {
                        return Some(event);
                    }
                    // Continue polling if conversion returned None
                }
            }
        }
    }

    pub fn present(&mut self, root: &mut dyn RenderNode) -> Result<(), &'static str> {
        // Get root buffer and copy to surface
        if let Some(buffer) = root.get_buffer() {
            let surface = self.connection
                .surface_mut(self.surface_id)
                .ok_or("Surface not found")?;

            let src_data = buffer.data();
            let dst_data = surface.buffer_mut();

            // Copy BGRA to BGRA (direct copy)
            let src_len = src_data.len().min(dst_data.len());
            dst_data[..src_len].copy_from_slice(&src_data[..src_len]);

            // Commit to server
            self.connection
                .commit(self.surface_id)
                .map_err(|_| "Failed to commit")?;
        }

        Ok(())
    }

    pub fn set_root(&mut self, _node: &mut dyn RenderNode) {
        // Nothing to do here, root is managed by Application
    }

    fn convert_event(&mut self, sws_event: SwsEvent) -> Option<Event> {
        match sws_event {
            SwsEvent::Input(input) => self.convert_input_event(input),
            SwsEvent::SurfaceConfigure { width, height, .. } => {
                self.width = width;
                self.height = height;
                None  // Configure events don't generate ScarletUI events
            }
            SwsEvent::SurfaceDestroyed { .. } => {
                // Window was closed
                None
            }
            SwsEvent::FocusChanged { .. } => {
                // TODO: Handle focus events
                None
            }
            SwsEvent::Error { .. } => {
                None
            }
        }
    }

    fn convert_input_event(&mut self, input: sws_client::InputEvent) -> Option<Event> {
        use sws_client::event_type::{EV_ABS, EV_KEY, EV_REL};
        use sws_client::abs_code::{ABS_X, ABS_Y};
        use sws_client::key_code::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
        use sws_client::rel_code::{REL_X, REL_Y};

        match input.type_ {
            EV_ABS => {
                // Absolute position (mouse)
                match input.code {
                    ABS_X => {
                        self.mouse_pos.x = input.value as f32;
                    }
                    ABS_Y => {
                        self.mouse_pos.y = input.value as f32;
                    }
                    _ => {}
                }
                None
            }
            EV_REL => {
                // Relative position (mouse movement)
                match input.code {
                    REL_X => {
                        self.mouse_pos.x += input.value as f32;
                    }
                    REL_Y => {
                        self.mouse_pos.y += input.value as f32;
                    }
                    _ => {}
                }
                None
            }
            EV_KEY => {
                // Key or button event
                match input.code {
                    BTN_LEFT => {
                        if input.value != 0 {
                            self.mouse_buttons |= 0x01;
                            Some(Event::Mouse(MouseEvent {
                                position: self.mouse_pos,
                                buttons: MouseButtons(self.mouse_buttons),
                                kind: MouseEventKind::Press,
                            }))
                        } else {
                            self.mouse_buttons &= !0x01;
                            Some(Event::Mouse(MouseEvent {
                                position: self.mouse_pos,
                                buttons: MouseButtons(self.mouse_buttons),
                                kind: MouseEventKind::Release,
                            }))
                        }
                    }
                    BTN_MIDDLE => {
                        if input.value != 0 {
                            self.mouse_buttons |= 0x04;
                            Some(Event::Mouse(MouseEvent {
                                position: self.mouse_pos,
                                buttons: MouseButtons(self.mouse_buttons),
                                kind: MouseEventKind::Press,
                            }))
                        } else {
                            self.mouse_buttons &= !0x04;
                            Some(Event::Mouse(MouseEvent {
                                position: self.mouse_pos,
                                buttons: MouseButtons(self.mouse_buttons),
                                kind: MouseEventKind::Release,
                            }))
                        }
                    }
                    BTN_RIGHT => {
                        if input.value != 0 {
                            self.mouse_buttons |= 0x02;
                            Some(Event::Mouse(MouseEvent {
                                position: self.mouse_pos,
                                buttons: MouseButtons(self.mouse_buttons),
                                kind: MouseEventKind::Press,
                            }))
                        } else {
                            self.mouse_buttons &= !0x02;
                            Some(Event::Mouse(MouseEvent {
                                position: self.mouse_pos,
                                buttons: MouseButtons(self.mouse_buttons),
                                kind: MouseEventKind::Release,
                            }))
                        }
                    }
                    _ => {
                        // Keyboard event - TODO: implement proper key mapping
                        // Only send events on press (value != 0)
                        if input.value != 0 {
                            Some(Event::Key(KeyEvent::Char(input.value as u8 as char)))
                        } else {
                            None
                        }
                    }
                }
            }
            _ => None,
        }
    }
}
