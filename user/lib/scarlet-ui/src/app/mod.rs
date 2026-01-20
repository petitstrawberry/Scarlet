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
        // Re-layout from dirty roots (cascade down)
        if let Some(ref mut root) = self.root_node {
            if root.is_dirty() {
                // Root is dirty, relayout everything
                let window_size = Size::new(800.0, 600.0);
                root.layout(LayoutConstraints::tight(window_size));
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
                    let constraints = LayoutConstraints::tight(child.frame().size);
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
                // Focus changed - we're only interested if we gained focus
                // For now, assume we always get focus events for our surface
                Some(Event::Focus(true))
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
                        // Keyboard event - map key codes to KeyEvent
                        if input.value != 0 {
                            use sws_client::key_code;
                            let key_event = match input.code {
                                key_code::KEY_ESC => Some(KeyEvent::Escape),
                                key_code::KEY_ENTER => Some(KeyEvent::Enter),
                                key_code::KEY_BACKSPACE => Some(KeyEvent::Backspace),
                                key_code::KEY_TAB => Some(KeyEvent::Tab),
                                key_code::KEY_DELETE => Some(KeyEvent::Delete),
                                key_code::KEY_HOME => Some(KeyEvent::Home),
                                key_code::KEY_END => Some(KeyEvent::End),
                                key_code::KEY_PAGEUP => Some(KeyEvent::PageUp),
                                key_code::KEY_PAGEDOWN => Some(KeyEvent::PageDown),
                                key_code::KEY_UP => Some(KeyEvent::Up),
                                key_code::KEY_DOWN => Some(KeyEvent::Down),
                                key_code::KEY_LEFT => Some(KeyEvent::Left),
                                key_code::KEY_RIGHT => Some(KeyEvent::Right),
                                // Printable characters
                                key_code::KEY_SPACE => Some(KeyEvent::Char(' ')),
                                key_code::KEY_1 => Some(KeyEvent::Char('1')),
                                key_code::KEY_2 => Some(KeyEvent::Char('2')),
                                key_code::KEY_3 => Some(KeyEvent::Char('3')),
                                key_code::KEY_4 => Some(KeyEvent::Char('4')),
                                key_code::KEY_5 => Some(KeyEvent::Char('5')),
                                key_code::KEY_6 => Some(KeyEvent::Char('6')),
                                key_code::KEY_7 => Some(KeyEvent::Char('7')),
                                key_code::KEY_8 => Some(KeyEvent::Char('8')),
                                key_code::KEY_9 => Some(KeyEvent::Char('9')),
                                key_code::KEY_0 => Some(KeyEvent::Char('0')),
                                key_code::KEY_Q => Some(KeyEvent::Char('q')),
                                key_code::KEY_W => Some(KeyEvent::Char('w')),
                                key_code::KEY_E => Some(KeyEvent::Char('e')),
                                key_code::KEY_R => Some(KeyEvent::Char('r')),
                                key_code::KEY_T => Some(KeyEvent::Char('t')),
                                key_code::KEY_Y => Some(KeyEvent::Char('y')),
                                key_code::KEY_U => Some(KeyEvent::Char('u')),
                                key_code::KEY_I => Some(KeyEvent::Char('i')),
                                key_code::KEY_O => Some(KeyEvent::Char('o')),
                                key_code::KEY_P => Some(KeyEvent::Char('p')),
                                key_code::KEY_A => Some(KeyEvent::Char('a')),
                                key_code::KEY_S => Some(KeyEvent::Char('s')),
                                key_code::KEY_D => Some(KeyEvent::Char('d')),
                                key_code::KEY_F => Some(KeyEvent::Char('f')),
                                key_code::KEY_G => Some(KeyEvent::Char('g')),
                                key_code::KEY_H => Some(KeyEvent::Char('h')),
                                key_code::KEY_J => Some(KeyEvent::Char('j')),
                                key_code::KEY_K => Some(KeyEvent::Char('k')),
                                key_code::KEY_L => Some(KeyEvent::Char('l')),
                                key_code::KEY_Z => Some(KeyEvent::Char('z')),
                                key_code::KEY_X => Some(KeyEvent::Char('x')),
                                key_code::KEY_C => Some(KeyEvent::Char('c')),
                                key_code::KEY_V => Some(KeyEvent::Char('v')),
                                key_code::KEY_B => Some(KeyEvent::Char('b')),
                                key_code::KEY_N => Some(KeyEvent::Char('n')),
                                key_code::KEY_M => Some(KeyEvent::Char('m')),
                                key_code::KEY_COMMA => Some(KeyEvent::Char(',')),
                                key_code::KEY_DOT => Some(KeyEvent::Char('.')),
                                key_code::KEY_SLASH => Some(KeyEvent::Char('/')),
                                key_code::KEY_SEMICOLON => Some(KeyEvent::Char(';')),
                                key_code::KEY_APOSTROPHE => Some(KeyEvent::Char('\'')),
                                key_code::KEY_LEFTBRACE => Some(KeyEvent::Char('[')),
                                key_code::KEY_RIGHTBRACE => Some(KeyEvent::Char(']')),
                                key_code::KEY_BACKSLASH => Some(KeyEvent::Char('\\')),
                                key_code::KEY_MINUS => Some(KeyEvent::Char('-')),
                                key_code::KEY_EQUAL => Some(KeyEvent::Char('=')),
                                _ => None,
                            };

                            key_event.map(|k| Event::Key(k))
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
