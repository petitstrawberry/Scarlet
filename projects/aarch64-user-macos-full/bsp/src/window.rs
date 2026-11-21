//! The main thread owns AppKit, the Metal surface and presentation resources.
mod drawable;
mod gpu;
mod input;
mod presenter;
mod scanout;

use input::{Input, InputSender, PointerButton};
use presenter::Presenter;
use scanout::{Frames, Output, Scanout, dimensions};
use scarlet::device::{
    graphics::{FramebufferConfig, PixelFormat},
    manager::DeviceManager,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition},
    event::{Modifiers, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{KeyCode, ModifiersKeyState, PhysicalKey},
    window::{Window as NativeWindow, WindowId},
};
const WIDTH: u32 = 1024;
const HEIGHT: u32 = 640;
type Stop = Box<dyn FnOnce() + Send>;
enum UiEvent {
    Gpu,
    Drawable(drawable::Drawable),
    Damage,
    Mode {
        generation: u64,
        config: FramebufferConfig,
        frames: Arc<Mutex<Frames>>,
    },
    Ready(Option<Stop>),
    Finished(Result<(), String>),
}

pub struct BootDisplay {
    gpu: crate::gpu::Endpoint,
    requested: Arc<AtomicU64>,
    config: FramebufferConfig,
    frames: Arc<Mutex<Frames>>,
    input: InputSender,
    proxy: EventLoopProxy<UiEvent>,
}
impl BootDisplay {
    pub fn register(self, manager: &DeviceManager, stop: Option<Stop>) -> Result<(), &'static str> {
        let proxy = self.proxy.clone();
        let scanout = Scanout::new(
            self.config.clone(),
            self.frames,
            Arc::new(move || {
                let _ = proxy.send_event(UiEvent::Damage);
            }),
        )?;
        self.gpu.register()?;
        self.input.register(65536, 65536)?;
        manager.register_device(Arc::new(Output::new(
            scanout,
            self.gpu,
            self.requested,
            self.proxy.clone(),
        )));
        self.proxy
            .send_event(UiEvent::Ready(stop))
            .map_err(|_| "Window closed during boot")
    }
}

pub fn run(
    boot: impl FnOnce(Option<BootDisplay>) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    crate::latency_trace::init();
    if std::env::var_os("SCARLET_HOSTED_GUI").is_some_and(|v| v == "0") {
        return boot(None);
    }
    let events = EventLoop::<UiEvent>::with_user_event()
        .build()
        .map_err(|e| e.to_string())?;
    events.set_control_flow(ControlFlow::Wait);
    let mut app = Handler {
        boot: Some(boot),
        requested: Arc::new(AtomicU64::new(0)),
        generation: 0,
        proxy: events.create_proxy(),
        input: None,
        frames: None,
        display: None,
        pixels: Vec::new(),
        ready: false,
        closing: false,
        stop: None,
        result: Ok(()),
    };
    let result = events.run_app(&mut app).map_err(|e| e.to_string());
    if let Some(stop) = app.stop.take() {
        stop();
    }
    result.and(app.result)
}
struct Display {
    presenter: Presenter,
    window: Arc<NativeWindow>,
    pointer: Option<PhysicalPosition<f64>>,
    scroll: (f64, f64),
    focused: bool,
    cursor_hidden: bool,
    occluded: bool,
    retry_at: Option<Instant>,
    retry_delay: Duration,
}

impl Display {
    fn open(
        events: &ActiveEventLoop,
        proxy: EventLoopProxy<UiEvent>,
    ) -> Result<(Self, crate::gpu::Endpoint), String> {
        let attributes = NativeWindow::default_attributes()
            .with_title("Scarlet")
            .with_inner_size(LogicalSize::new(WIDTH, HEIGHT));
        let window = Arc::new(
            events
                .create_window(attributes)
                .map_err(|e| e.to_string())?,
        );
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);
        let gpu_proxy = proxy.clone();
        let (presenter, gpu) = Presenter::open(
            window.clone(), width, height,
            move || { let _ = gpu_proxy.send_event(UiEvent::Gpu); },
            move |frame| { let _ = proxy.send_event(UiEvent::Drawable(frame)); },
        )?;
        Ok((
            Self {
                focused: window.has_focus(),
                presenter,
                window,
                pointer: None,
                scroll: (0.0, 0.0),
                cursor_hidden: false,
                occluded: false,
                retry_at: None,
                retry_delay: Duration::from_millis(16),
            },
            gpu,
        ))
    }

    fn guest_pointer(&self) -> Option<(i32, i32)> {
        let position = self.pointer?;
        let viewport = self.presenter.viewport(self.window.inner_size())?;
        viewport.guest_position(position, (65536, 65536))
    }

    fn update_cursor(&mut self) {
        let hide = self.focused && self.presenter.have_frame() && self.guest_pointer().is_some();
        if hide != self.cursor_hidden {
            self.window.set_cursor_visible(!hide);
            self.cursor_hidden = hide;
        }
    }

    fn update_pointer(&mut self, input: &mut InputSender) {
        self.update_cursor();
        if let Some((x, y)) = self.guest_pointer() {
            input.send(Input::Motion(x, y));
        }
    }

    fn redraw(&self) {
        if !self.occluded && self.retry_at.is_none() {
            self.window.request_redraw();
        }
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        if self.cursor_hidden {
            self.window.set_cursor_visible(true);
        }
    }
}

struct Handler<F> {
    boot: Option<F>,
    requested: Arc<AtomicU64>,
    generation: u64,
    proxy: EventLoopProxy<UiEvent>,
    input: Option<InputSender>,
    display: Option<Display>,
    frames: Option<Arc<Mutex<Frames>>>,
    pixels: Vec<u8>,
    ready: bool,
    closing: bool,
    stop: Option<Stop>,
    result: Result<(), String>,
}
impl<F> ApplicationHandler<UiEvent> for Handler<F>
where
    F: FnOnce(Option<BootDisplay>) -> Result<(), String> + Send + 'static,
{
    fn resumed(&mut self, events: &ActiveEventLoop) {
        let Some(boot) = self.boot.take() else {
            return;
        };
        let (display, gpu) = match Display::open(events, self.proxy.clone()) {
            Ok(d) => d,
            Err(e) => {
                self.result = Err(e);
                events.exit();
                return;
            }
        };
        let (width, height) = display.presenter.scanout_size();
        let config = FramebufferConfig::new(width, height, PixelFormat::BGRA8888);
        self.pixels.resize(config.size(), 0);
        let frames = Arc::new(Mutex::new(Frames::new(&config)));
        let input = InputSender::default();
        self.requested
            .store(dimensions(width, height), Ordering::Release);
        let device = BootDisplay {
            gpu,
            requested: self.requested.clone(),
            config,
            frames: frames.clone(),
            input: input.clone(),
            proxy: self.proxy.clone(),
        };
        let reply = self.proxy.clone();
        self.display = Some(display);
        self.frames = Some(frames);
        self.input = Some(input);
        if let Err(e) = std::thread::Builder::new()
            .name("scarlet-bsp".into())
            .spawn(move || {
                let result = boot(Some(device));
                let _ = reply.send_event(UiEvent::Finished(result));
            })
        {
            self.result = Err(e.to_string());
            events.exit();
        }
    }
    fn user_event(&mut self, events: &ActiveEventLoop, event: UiEvent) {
        match event {
            UiEvent::Mode {
                generation,
                config,
                frames,
            } => {
                if generation > self.generation {
                    self.generation = generation;
                    self.frames = Some(frames);
                    self.pixels.resize(config.size(), 0);
                    if let Some(display) = &mut self.display {
                        display
                            .presenter
                            .replace_scanout(config.width, config.height);
                        display.update_pointer(self.input.as_mut().unwrap());
                        display.redraw();
                    }
                }
            }
            UiEvent::Ready(stop) => {
                self.ready = true;
                self.stop = stop;
                if self.closing {
                    events.exit();
                }
            }
            UiEvent::Gpu => {
                if let Some(display) = &mut self.display {
                    if display.presenter.take_gpu_present() {
                        display.update_cursor();
                        display.redraw();
                    }
                }
            }
            UiEvent::Drawable(frame) => {
                let Some(display) = &mut self.display else { return };
                match display.presenter.draw(display.window.inner_size(), !display.occluded, frame) {
                    Ok(presenter::Draw::Complete) => {
                        display.retry_delay = Duration::from_millis(16);
                    }
                    Ok(presenter::Draw::Retry) => {
                        // A missing drawable can persist while AppKit changes the surface.
                        // Keep damage in the scanout and retry without spinning the event loop.
                        display.retry_at = Some(Instant::now() + display.retry_delay);
                        display.retry_delay = (display.retry_delay * 2).min(Duration::from_millis(250));
                    }
                    Err(e) => {
                        self.result = Err(e);
                        self.closing = true;
                        if self.ready {
                            events.exit();
                        }
                    }
                }
            }
            UiEvent::Damage => {
                let Some(display) = &mut self.display else {
                    return;
                };
                if let Some(region) = self
                    .frames
                    .as_ref()
                    .and_then(|frames| frames.lock().unwrap().take(&mut self.pixels))
                {
                    if let Err(e) = display.presenter.upload(&self.pixels, region) {
                        self.result = Err(e);
                        self.closing = true;
                        if self.ready {
                            events.exit();
                        }
                        return;
                    }
                    display.update_cursor();
                    display.redraw();
                }
            }
            UiEvent::Finished(result) => {
                self.result = result;
                events.exit();
            }
        }
    }
    fn about_to_wait(&mut self, events: &ActiveEventLoop) {
        if let Some(display) = &mut self.display {
            if let Some(deadline) = display.retry_at {
                if Instant::now() < deadline {
                    events.set_control_flow(ControlFlow::WaitUntil(deadline));
                    return;
                }
                display.retry_at = None;
                display.redraw();
            }
        }
        events.set_control_flow(ControlFlow::Wait);
    }

    fn window_event(&mut self, events: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let (Some(display), Some(input)) = (&mut self.display, &mut self.input) else {
            return;
        };
        if display.window.id() != id {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                self.closing = true;
                if self.ready {
                    events.exit();
                }
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                let size = display.window.inner_size();
                if size.width > 0 && size.height > 0 && size.width <= 8192 && size.height <= 8192 {
                    self.requested
                        .store(dimensions(size.width, size.height), Ordering::Release);
                }
                if self.ready {
                    display.update_pointer(input);
                }
                display.redraw();
            }
            WindowEvent::RedrawRequested if !display.occluded && display.retry_at.is_none() => {
                if let Err(error) = display.presenter.request_draw(display.window.inner_size()) {
                    self.result = Err(error);
                    self.closing = true;
                    if self.ready { events.exit(); }
                }
            }
            WindowEvent::Occluded(value) => {
                display.occluded = value;
                display.retry_at = None;
                display.retry_delay = Duration::from_millis(16);
                display.redraw();
            }
            _ if !self.ready => {}
            WindowEvent::Focused(focused) => {
                display.focused = focused;
                if !focused {
                    input.send(Input::ReleaseAll);
                    display.scroll = (0.0, 0.0);
                }
                display.update_cursor();
            }
            WindowEvent::CursorMoved { position, .. } => {
                display.pointer = Some(position);
                display.update_pointer(input);
            }
            WindowEvent::CursorLeft { .. } => {
                display.pointer = None;
                display.scroll = (0.0, 0.0);
                display.update_cursor();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    MouseButton::Left => PointerButton::Left,
                    MouseButton::Right => PointerButton::Right,
                    MouseButton::Middle => PointerButton::Middle,
                    _ => return,
                };
                if !state.is_pressed() || display.guest_pointer().is_some() {
                    input.send(Input::Button(button, state.is_pressed()));
                }
            }
            WindowEvent::MouseWheel { delta, .. } if display.guest_pointer().is_some() => {
                let (x, y) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (f64::from(x), f64::from(y)),
                    MouseScrollDelta::PixelDelta(pos) => {
                        let pixels_per_line = 32.0 * display.window.scale_factor();
                        (pos.x / pixels_per_line, pos.y / pixels_per_line)
                    }
                };
                display.scroll.0 += x;
                display.scroll.1 += y;
                let (x, y) = (display.scroll.0 as i32, display.scroll.1 as i32);
                display.scroll.0 -= f64::from(x);
                display.scroll.1 -= f64::from(y);
                input.send(Input::Wheel(x, y));
            }
            WindowEvent::ModifiersChanged(modifiers) => update_modifiers(input, modifiers),
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key
                    && let Some(usage) = key_usage(code)
                {
                    let trace = crate::latency_trace::keyboard(
                        usage, event.state.is_pressed(), event.repeat,
                    );
                    crate::latency_trace::hid_enqueue(trace, false);
                    input.send(Input::Key(usage, event.state.is_pressed(), event.repeat));
                    crate::latency_trace::hid_enqueue(trace, true);
                }
            }
            _ => {}
        }
    }
}

// USB HID keyboard usages understood by Scarlet's keyboard driver.
fn key_usage(code: KeyCode) -> Option<u8> {
    use KeyCode::*;
    #[rustfmt::skip]
    const LETTERS_AND_SYMBOLS: &[KeyCode] = &[
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM,
        KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
        Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9, Digit0,
        Enter, Escape, Backspace, Tab, Space, Minus, Equal, BracketLeft, BracketRight,
        Backslash,
    ];
    #[rustfmt::skip]
    const FUNCTION_AND_NAVIGATION: &[KeyCode] = &[
        Semicolon, Quote, Backquote, Comma, Period, Slash, CapsLock,
        F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
        PrintScreen, ScrollLock, Pause, Insert, Home, PageUp, Delete, End, PageDown,
        ArrowRight, ArrowLeft, ArrowDown, ArrowUp, NumLock, NumpadDivide, NumpadMultiply,
        NumpadSubtract, NumpadAdd, NumpadEnter, Numpad1, Numpad2, Numpad3, Numpad4,
        Numpad5, Numpad6, Numpad7, Numpad8, Numpad9, Numpad0, NumpadDecimal,
        IntlBackslash, ContextMenu, Power, NumpadEqual,
    ];
    [(0x04, LETTERS_AND_SYMBOLS), (0x33, FUNCTION_AND_NAVIGATION)]
        .into_iter()
        .find_map(|(first, keys)| {
            keys.iter()
                .position(|&key| key == code)
                .map(|i| first + i as u8)
        })
}

fn update_modifiers(input: &mut InputSender, modifiers: Modifiers) {
    let state = modifiers.state();
    let modifiers = [
        (
            state.control_key(),
            modifiers.lcontrol_state(),
            modifiers.rcontrol_state(),
        ),
        (
            state.shift_key(),
            modifiers.lshift_state(),
            modifiers.rshift_state(),
        ),
        (
            state.alt_key(),
            modifiers.lalt_state(),
            modifiers.ralt_state(),
        ),
        (
            state.super_key(),
            modifiers.lsuper_state(),
            modifiers.rsuper_state(),
        ),
    ];
    for (index, (active, left, right)) in modifiers.into_iter().enumerate() {
        // Some macOS events include the combined flag without a physical side.
        let right = right == ModifiersKeyState::Pressed;
        let left = left == ModifiersKeyState::Pressed || !right;
        input.send(Input::Key(0xe0 + index as u8, active && left, false));
        input.send(Input::Key(0xe4 + index as u8, active && right, false));
    }
}
