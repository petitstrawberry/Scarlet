extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::any::Any;
use core::cell::RefCell;
use core::num::NonZeroU32;
use core::sync::atomic::{AtomicU32, Ordering};
use scarlet_ui_core::buffer::Buffer;
use scarlet_ui_core::compositor::DamageRect;
use scarlet_ui_core::element::TextInputElementState;
use scarlet_ui_core::error::{Error, Result};
use scarlet_ui_core::event::{Event, KeyCode, KeyEvent, MouseButton, MouseEvent};
use scarlet_ui_core::geometry::{Point, Size};
use scarlet_ui_core::platform::{PlatformBackend, PlatformWindow, WindowCreateRequest};
use std::time::Duration;

use ::winit::application::ApplicationHandler;
use ::winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition};
use ::winit::event::{
    ElementState as WinitElementState, Ime, MouseButton as WinitMouseButton, MouseScrollDelta,
    WindowEvent,
};
use ::winit::event_loop::{ActiveEventLoop, EventLoop};
use ::winit::keyboard::{Key, NamedKey};
use ::winit::platform::pump_events::EventLoopExtPumpEvents;
use ::winit::window::{Window as WinitWindow, WindowAttributes, WindowId};

type SoftbufferContext = softbuffer::Context<::winit::event_loop::OwnedDisplayHandle>;
type SoftbufferSurface =
    softbuffer::Surface<::winit::event_loop::OwnedDisplayHandle, Rc<WinitWindow>>;

pub struct WinitBackend {
    shared: Rc<WinitSharedState>,
}

impl WinitBackend {
    /// Create a winit backend.
    ///
    /// # Returns
    ///
    /// A backend that creates native desktop windows.
    pub fn new() -> Self {
        Self {
            shared: Rc::new(WinitSharedState::new()),
        }
    }
}

impl PlatformBackend for WinitBackend {
    fn output_scale_milli(&mut self) -> u32 {
        1000
    }

    fn create_window(&mut self, request: WindowCreateRequest) -> Result<Box<dyn PlatformWindow>> {
        Ok(Box::new(WinitPlatformWindow::create(
            self.shared.clone(),
            request,
        )?))
    }
}

impl Default for WinitBackend {
    fn default() -> Self {
        Self::new()
    }
}

struct WinitEventState {
    scale_factor: f64,
    cursor_physical_x: f64,
    cursor_physical_y: f64,
    cursor_x: i32,
    cursor_y: i32,
    window_focused: bool,
    manual_move_active: bool,
    manual_move_origin_outer_x: i32,
    manual_move_origin_outer_y: i32,
    manual_move_origin_global_x: f64,
    manual_move_origin_global_y: f64,
    ime_preedit_active: bool,
    text_input_context_id: u32,
    text_input_serial: u32,
    pending_empty_preedit: Option<(u32, u32)>,
    queue: VecDeque<Event>,
}

impl WinitEventState {
    fn new(scale_factor: f64) -> Self {
        Self {
            scale_factor,
            cursor_physical_x: 0.0,
            cursor_physical_y: 0.0,
            cursor_x: 0,
            cursor_y: 0,
            window_focused: true,
            manual_move_active: false,
            manual_move_origin_outer_x: 0,
            manual_move_origin_outer_y: 0,
            manual_move_origin_global_x: 0.0,
            manual_move_origin_global_y: 0.0,
            ime_preedit_active: false,
            text_input_context_id: 1,
            text_input_serial: 1,
            pending_empty_preedit: None,
            queue: VecDeque::new(),
        }
    }

    fn push(&mut self, event: Event) {
        self.queue.push_back(event);
    }

    fn next_text_input_serial(&mut self) -> u32 {
        let serial = self.text_input_serial;
        self.text_input_serial = self.text_input_serial.saturating_add(1);
        serial
    }

    fn defer_empty_preedit(&mut self) {
        let context_id = self.text_input_context_id;
        let serial = self.next_text_input_serial();
        self.pending_empty_preedit = Some((context_id, serial));
    }

    fn discard_pending_empty_preedit(&mut self) {
        self.pending_empty_preedit = None;
    }

    fn flush_pending_empty_preedit(&mut self) {
        let Some((context_id, serial)) = self.pending_empty_preedit.take() else {
            return;
        };
        self.push(Event::TextInputPreedit {
            context_id,
            serial,
            cursor_byte: 0,
            anchor_byte: 0,
            text: alloc::string::String::new(),
            spans: Vec::new(),
        });
    }
}

struct WinitWindowEntry {
    state: Rc<RefCell<WinitEventState>>,
    window: Rc<WinitWindow>,
}

struct WinitSharedState {
    event_loop: RefCell<EventLoop<()>>,
    windows: RefCell<BTreeMap<WindowId, WinitWindowEntry>>,
}

impl WinitSharedState {
    fn new() -> Self {
        let event_loop = EventLoop::new().expect("winit event loop creation must succeed");
        Self {
            event_loop: RefCell::new(event_loop),
            windows: RefCell::new(BTreeMap::new()),
        }
    }

    fn window_entry(
        &self,
        window_id: WindowId,
    ) -> Option<(Rc<RefCell<WinitEventState>>, Rc<WinitWindow>)> {
        self.windows
            .borrow()
            .get(&window_id)
            .map(|entry| (entry.state.clone(), entry.window.clone()))
    }

    fn remove_window(&self, window_id: WindowId) {
        self.windows.borrow_mut().remove(&window_id);
    }
}

struct WinitPumpHandler {
    shared: Rc<WinitSharedState>,
}

impl ApplicationHandler for WinitPumpHandler {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some((state, window)) = self.shared.window_entry(window_id) else {
            return;
        };
        let mut state = state.borrow_mut();

        match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => {
                state.push(Event::Quit);
            }
            WindowEvent::Resized(size) => {
                let logical_width = physical_to_logical_len(size.width, state.scale_factor);
                let logical_height = physical_to_logical_len(size.height, state.scale_factor);
                state.push(Event::Resize {
                    width: logical_width,
                    height: logical_height,
                });
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                state.scale_factor = scale_factor;
                let size = window.inner_size();
                state.push(Event::Resize {
                    width: physical_to_logical_len(size.width, scale_factor),
                    height: physical_to_logical_len(size.height, scale_factor),
                });
            }
            WindowEvent::Focused(focused) => {
                state.window_focused = focused;
                if !focused {
                    state.manual_move_active = false;
                    state.ime_preedit_active = false;
                    state.discard_pending_empty_preedit();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let new_x = physical_to_logical_pos(position.x, state.scale_factor);
                let new_y = physical_to_logical_pos(position.y, state.scale_factor);
                if state.manual_move_active {
                    if let Ok(outer) = window.outer_position() {
                        let global_x = outer.x as f64 + position.x;
                        let global_y = outer.y as f64 + position.y;
                        let new_outer_x = state.manual_move_origin_outer_x as f64 + global_x
                            - state.manual_move_origin_global_x;
                        let new_outer_y = state.manual_move_origin_outer_y as f64 + global_y
                            - state.manual_move_origin_global_y;
                        window.set_outer_position(PhysicalPosition::new(
                            f64_to_i32_saturated(new_outer_x.round()),
                            f64_to_i32_saturated(new_outer_y.round()),
                        ));
                    }
                    state.cursor_physical_x = position.x;
                    state.cursor_physical_y = position.y;
                    state.cursor_x = new_x;
                    state.cursor_y = new_y;
                    return;
                }
                state.cursor_physical_x = position.x;
                state.cursor_physical_y = position.y;
                state.cursor_x = new_x;
                state.cursor_y = new_y;
                let x = state.cursor_x;
                let y = state.cursor_y;
                state.push(Event::Mouse(MouseEvent::Moved { x, y }));
            }
            WindowEvent::CursorEntered { .. } => {
                let x = state.cursor_x;
                let y = state.cursor_y;
                state.push(Event::Mouse(MouseEvent::Entered { x, y }));
            }
            WindowEvent::CursorLeft { .. } => {
                let x = state.cursor_x;
                let y = state.cursor_y;
                state.push(Event::Mouse(MouseEvent::Exited { x, y }));
            }
            WindowEvent::MouseInput {
                state: button_state,
                button,
                ..
            } => {
                let Some(button) = map_mouse_button(button) else {
                    return;
                };
                let x = state.cursor_x;
                let y = state.cursor_y;
                if state.manual_move_active {
                    if button == MouseButton::Left && button_state == WinitElementState::Released {
                        state.manual_move_active = false;
                        state.push(Event::Mouse(MouseEvent::ButtonReleased {
                            button: MouseButton::Left,
                            x,
                            y,
                        }));
                    }
                    return;
                }
                let event = if button_state == WinitElementState::Pressed {
                    MouseEvent::ButtonPressed { button, x, y }
                } else {
                    MouseEvent::ButtonReleased { button, x, y }
                };
                state.push(Event::Mouse(event));
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (delta_x, delta_y) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => ((x * 32.0) as i32, (y * 32.0) as i32),
                    MouseScrollDelta::PixelDelta(delta) => (
                        physical_to_logical_pos(delta.x, state.scale_factor),
                        physical_to_logical_pos(delta.y, state.scale_factor),
                    ),
                };
                state.push(Event::Mouse(MouseEvent::Wheel { delta_x, delta_y }));
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let keycode = map_key(&event.logical_key);
                if event.state == WinitElementState::Pressed {
                    state.push(Event::Keyboard(KeyEvent::Pressed { keycode }));
                    if !state.ime_preedit_active
                        && let Key::Character(text) = &event.logical_key
                    {
                        for c in text.chars() {
                            if !c.is_control() {
                                state.push(Event::Keyboard(KeyEvent::Char { c }));
                            }
                        }
                    }
                } else {
                    state.push(Event::Keyboard(KeyEvent::Released { keycode }));
                }
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                state.discard_pending_empty_preedit();
                state.ime_preedit_active = false;
                let context_id = state.text_input_context_id;
                let serial = state.next_text_input_serial();
                state.push(Event::TextInputCommit {
                    context_id,
                    serial,
                    text,
                });
            }
            WindowEvent::Ime(Ime::Preedit(text, cursor)) => {
                state.discard_pending_empty_preedit();
                if text.is_empty() {
                    state.ime_preedit_active = false;
                    state.defer_empty_preedit();
                    return;
                }
                state.ime_preedit_active = true;
                let (cursor_byte, anchor_byte) = cursor
                    .map_or((text.len() as u32, text.len() as u32), |(start, end)| {
                        (end as u32, start as u32)
                    });
                let context_id = state.text_input_context_id;
                let serial = state.next_text_input_serial();
                state.push(Event::TextInputPreedit {
                    context_id,
                    serial,
                    cursor_byte,
                    anchor_byte,
                    text,
                    spans: Vec::new(),
                });
            }
            WindowEvent::Ime(Ime::Disabled) => {
                state.discard_pending_empty_preedit();
                state.ime_preedit_active = false;
                let context_id = state.text_input_context_id;
                let serial = state.next_text_input_serial();
                state.push(Event::TextInputPreedit {
                    context_id,
                    serial,
                    cursor_byte: 0,
                    anchor_byte: 0,
                    text: alloc::string::String::new(),
                    spans: Vec::new(),
                });
            }
            _ => {}
        }
    }
}

pub struct WinitPlatformWindow {
    shared: Rc<WinitSharedState>,
    window: Rc<WinitWindow>,
    surface: SoftbufferSurface,
    state: Rc<RefCell<WinitEventState>>,
    current_size: Size,
    ime_allowed: bool,
    surface_id: u32,
}

impl WinitPlatformWindow {
    fn create(shared: Rc<WinitSharedState>, request: WindowCreateRequest) -> Result<Self> {
        let attributes = WindowAttributes::default()
            .with_title(request.title)
            .with_decorations(false)
            .with_inner_size(LogicalSize::new(request.size.width, request.size.height));
        let context = {
            let event_loop = shared.event_loop.borrow();
            SoftbufferContext::new(event_loop.owned_display_handle()).map_err(|_| Error::IoError)?
        };
        #[allow(deprecated)]
        let window = {
            let event_loop = shared.event_loop.borrow();
            Rc::new(
                event_loop
                    .create_window(attributes)
                    .map_err(|_| Error::WindowCreationFailed)?,
            )
        };
        window.set_ime_allowed(false);
        let scale_factor = window.scale_factor();
        let inner_size = window.inner_size();
        let surface =
            SoftbufferSurface::new(&context, window.clone()).map_err(|_| Error::IoError)?;
        let surface_id = next_surface_id();
        let state = Rc::new(RefCell::new(WinitEventState::new(scale_factor)));
        shared.windows.borrow_mut().insert(
            window.id(),
            WinitWindowEntry {
                state: state.clone(),
                window: window.clone(),
            },
        );
        Ok(Self {
            shared,
            window,
            surface,
            state,
            current_size: physical_to_logical_size(
                inner_size.width,
                inner_size.height,
                scale_factor,
            ),
            ime_allowed: false,
            surface_id,
        })
    }

    fn pump_events(&mut self) {
        let mut handler = WinitPumpHandler {
            shared: self.shared.clone(),
        };
        let _ = self
            .shared
            .event_loop
            .borrow_mut()
            .pump_app_events(Some(Duration::ZERO), &mut handler);
        self.state.borrow_mut().flush_pending_empty_preedit();
    }

    fn resize_surface(&mut self, width: u32, height: u32) -> Result<()> {
        let width = NonZeroU32::new(width.max(1)).ok_or(Error::InvalidSize { width, height })?;
        let height = NonZeroU32::new(height.max(1)).ok_or(Error::InvalidSize {
            width: width.get(),
            height,
        })?;
        self.surface
            .resize(width, height)
            .map_err(|_| Error::RenderError)
    }

    pub(crate) fn set_observed_logical_size(&mut self, size: Size) {
        self.current_size = size;
    }

    fn observed_logical_size(&self) -> Size {
        let scale_factor = self.window.scale_factor();
        let inner_size = self.window.inner_size();
        physical_to_logical_size(inner_size.width, inner_size.height, scale_factor)
    }
}

impl PlatformWindow for WinitPlatformWindow {
    fn new(app_id: &str, title: &str, size: Size) -> Result<Self> {
        let backend = WinitBackend::new();
        Self::create(
            backend.shared.clone(),
            WindowCreateRequest {
                app_id: alloc::string::String::from(app_id),
                title: alloc::string::String::from(title),
                size,
                window_type: 0,
                menu_titles: alloc::string::String::new(),
                focus_on_create: true,
                active_on_focus: true,
                opaque: true,
            },
        )
    }

    fn poll_event(&mut self) -> Option<Event> {
        self.pump_events();
        let event = self.state.borrow_mut().queue.pop_front();
        if let Some(Event::Resize { width, height }) = event {
            let size = Size::new(width as f32, height as f32);
            self.set_observed_logical_size(size);
            Some(Event::Resize { width, height })
        } else {
            event
        }
    }

    fn wait_for_event(&mut self, timeout: Duration) {
        let mut handler = WinitPumpHandler {
            shared: self.shared.clone(),
        };
        let _ = self
            .shared
            .event_loop
            .borrow_mut()
            .pump_app_events(Some(timeout), &mut handler);
        self.state.borrow_mut().flush_pending_empty_preedit();
    }

    fn output_scale_milli(&self) -> u32 {
        scale_factor_to_milli(self.window.scale_factor())
    }

    fn present(&mut self, buffer: &Buffer) {
        let _ = self.present_buffer(buffer);
    }

    fn present_with_damage(&mut self, buffer: &Buffer, _damage: Option<&[DamageRect]>) {
        let _ = self.present_buffer(buffer);
    }

    fn set_title(&mut self, title: &str) {
        self.window.set_title(title);
    }

    fn size(&self) -> Size {
        self.observed_logical_size()
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        let _ = self
            .window
            .request_inner_size(LogicalSize::new(width as f32, height as f32));
        self.current_size = Size::new(width as f32, height as f32);
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        self.window.set_visible(false);
        Ok(())
    }

    fn minimize(&mut self) -> Result<()> {
        self.window.set_minimized(true);
        Ok(())
    }

    fn maximize(&mut self) -> Result<()> {
        self.window.set_maximized(true);
        Ok(())
    }

    fn restore(&mut self) -> Result<()> {
        self.window.set_minimized(false);
        self.window.set_maximized(false);
        Ok(())
    }

    fn request_move(&mut self) -> Result<()> {
        let outer = self
            .window
            .outer_position()
            .map_err(|_| Error::EventDispatchError)?;
        let mut state = self.state.borrow_mut();
        state.manual_move_active = true;
        state.manual_move_origin_outer_x = outer.x;
        state.manual_move_origin_outer_y = outer.y;
        state.manual_move_origin_global_x = outer.x as f64 + state.cursor_physical_x;
        state.manual_move_origin_global_y = outer.y as f64 + state.cursor_physical_y;
        Ok(())
    }

    fn create_popup(&mut self, _position: Point, _size: Size) -> Result<u32> {
        Ok(0)
    }

    fn destroy_popup(&mut self, _surface_id: u32) -> Result<()> {
        Ok(())
    }

    fn set_workarea(&mut self, _x: i32, _y: i32, _width: u32, _height: u32) -> Result<()> {
        Ok(())
    }

    fn create_window_with_type(
        &mut self,
        app_id: &str,
        title: &str,
        size: Size,
        window_type: u32,
    ) -> Result<Self>
    where
        Self: Sized,
    {
        let _ = app_id;
        Self::create(
            self.shared.clone(),
            WindowCreateRequest {
                app_id: alloc::string::String::from(app_id),
                title: alloc::string::String::from(title),
                size,
                window_type,
                menu_titles: alloc::string::String::new(),
                focus_on_create: true,
                active_on_focus: true,
                opaque: true,
            },
        )
    }

    fn move_window(&mut self, x: i32, y: i32) -> Result<()> {
        self.window
            .set_outer_position(LogicalPosition::new(x as f32, y as f32));
        Ok(())
    }

    fn set_window_type(&mut self, _surface_id: u32, _window_type: u32) -> Result<()> {
        Ok(())
    }

    fn get_screen_size(&mut self) -> Result<(u32, u32)> {
        let Some(monitor) = self.window.current_monitor() else {
            return Ok((
                self.current_size.width as u32,
                self.current_size.height as u32,
            ));
        };
        let size = monitor.size();
        Ok((size.width, size.height))
    }

    fn surface_id(&self) -> u32 {
        self.surface_id
    }

    fn platform_window_id(&self) -> u64 {
        self.surface_id as u64
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn set_resizable(&mut self, resizable: bool) -> Result<()> {
        self.window.set_resizable(resizable);
        Ok(())
    }

    fn set_opaque(&mut self, _opaque: bool) -> Result<()> {
        Ok(())
    }

    fn set_menu_titles(&mut self, _menu_titles: &str) -> Result<()> {
        Ok(())
    }

    fn sync_text_input(&mut self, state: Option<&TextInputElementState>) {
        let Some(state) = state else {
            let mut event_state = self.state.borrow_mut();
            event_state.ime_preedit_active = false;
            event_state.discard_pending_empty_preedit();
            if self.ime_allowed {
                self.window.set_ime_allowed(false);
                self.ime_allowed = false;
            }
            return;
        };
        if !self.ime_allowed {
            self.window.set_ime_allowed(true);
            self.ime_allowed = true;
        }
        self.window.set_ime_cursor_area(
            LogicalPosition::new(state.cursor_rect.origin.x, state.cursor_rect.origin.y),
            LogicalSize::new(state.cursor_rect.size.width, state.cursor_rect.size.height),
        );
    }
}

impl Drop for WinitPlatformWindow {
    fn drop(&mut self) {
        self.shared.remove_window(self.window.id());
    }
}

impl WinitPlatformWindow {
    fn present_buffer(&mut self, buffer: &Buffer) -> Result<()> {
        let physical_size = self.window.inner_size();
        let width = physical_size.width.max(1);
        let height = physical_size.height.max(1);
        self.resize_surface(width, height)?;
        let mut surface_buffer = self.surface.buffer_mut().map_err(|_| Error::RenderError)?;
        copy_scaled(buffer, &mut surface_buffer, width, height);
        surface_buffer.present().map_err(|_| Error::RenderError)
    }
}

fn physical_to_logical_len(value: u32, scale_factor: f64) -> u32 {
    ((value as f64 / scale_factor.max(0.001)).round() as u32).max(1)
}

fn scale_factor_to_milli(scale_factor: f64) -> u32 {
    (scale_factor.max(0.001) * 1000.0).round() as u32
}

fn physical_to_logical_pos(value: f64, scale_factor: f64) -> i32 {
    (value / scale_factor.max(0.001)).round() as i32
}

fn f64_to_i32_saturated(value: f64) -> i32 {
    if value <= i32::MIN as f64 {
        i32::MIN
    } else if value >= i32::MAX as f64 {
        i32::MAX
    } else {
        value as i32
    }
}

fn physical_to_logical_size(width: u32, height: u32, scale_factor: f64) -> Size {
    Size::new(
        physical_to_logical_len(width, scale_factor) as f32,
        physical_to_logical_len(height, scale_factor) as f32,
    )
}

fn next_surface_id() -> u32 {
    static NEXT_SURFACE_ID: AtomicU32 = AtomicU32::new(1);
    NEXT_SURFACE_ID.fetch_add(1, Ordering::Relaxed).max(1)
}

fn copy_scaled(buffer: &Buffer, dst: &mut [u32], dst_width: u32, dst_height: u32) {
    let src_width = buffer.width().max(1);
    let src_height = buffer.height().max(1);
    let src = buffer.as_slice();
    if src_width == dst_width && src_height == dst_height {
        let len = dst.len().min(src.len());
        dst[..len].copy_from_slice(&src[..len]);
        return;
    }

    for y in 0..dst_height {
        let src_y = (y as u64 * src_height as u64 / dst_height as u64) as u32;
        for x in 0..dst_width {
            let src_x = (x as u64 * src_width as u64 / dst_width as u64) as u32;
            let src_index = (src_y * src_width + src_x) as usize;
            let dst_index = (y * dst_width + x) as usize;
            if let (Some(dst_px), Some(src_px)) = (dst.get_mut(dst_index), src.get(src_index)) {
                *dst_px = *src_px;
            }
        }
    }
}

fn map_mouse_button(button: WinitMouseButton) -> Option<MouseButton> {
    match button {
        WinitMouseButton::Left => Some(MouseButton::Left),
        WinitMouseButton::Middle => Some(MouseButton::Middle),
        WinitMouseButton::Right => Some(MouseButton::Right),
        _ => None,
    }
}

fn map_key(key: &Key) -> KeyCode {
    match key {
        Key::Named(NamedKey::Escape) => KeyCode::Escape,
        Key::Named(NamedKey::Enter) => KeyCode::Enter,
        Key::Named(NamedKey::Tab) => KeyCode::Tab,
        Key::Named(NamedKey::Backspace) => KeyCode::Backspace,
        Key::Named(NamedKey::Space) => KeyCode::Space,
        Key::Named(NamedKey::ArrowLeft) => KeyCode::Left,
        Key::Named(NamedKey::ArrowRight) => KeyCode::Right,
        Key::Named(NamedKey::ArrowUp) => KeyCode::Up,
        Key::Named(NamedKey::ArrowDown) => KeyCode::Down,
        Key::Named(NamedKey::Home) => KeyCode::Home,
        Key::Named(NamedKey::End) => KeyCode::End,
        Key::Named(NamedKey::PageUp) => KeyCode::PageUp,
        Key::Named(NamedKey::PageDown) => KeyCode::PageDown,
        Key::Named(NamedKey::Insert) => KeyCode::Insert,
        Key::Named(NamedKey::Delete) => KeyCode::Delete,
        Key::Named(NamedKey::F1) => KeyCode::F(1),
        Key::Named(NamedKey::F2) => KeyCode::F(2),
        Key::Named(NamedKey::F3) => KeyCode::F(3),
        Key::Named(NamedKey::F4) => KeyCode::F(4),
        Key::Named(NamedKey::F5) => KeyCode::F(5),
        Key::Named(NamedKey::F6) => KeyCode::F(6),
        Key::Named(NamedKey::F7) => KeyCode::F(7),
        Key::Named(NamedKey::F8) => KeyCode::F(8),
        Key::Named(NamedKey::F9) => KeyCode::F(9),
        Key::Named(NamedKey::F10) => KeyCode::F(10),
        Key::Named(NamedKey::F11) => KeyCode::F(11),
        Key::Named(NamedKey::F12) => KeyCode::F(12),
        Key::Character(text) => text.chars().next().map_or(KeyCode::Unknown, KeyCode::Char),
        _ => KeyCode::Unknown,
    }
}
