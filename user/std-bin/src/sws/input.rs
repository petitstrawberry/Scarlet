//! Input event handling module

use core::sync::atomic::{AtomicU32, Ordering};
use scarlet_os::handle::capability::StreamError;
use scarlet_os::handle::{Handle, HandleResult};
use std::println;
use std::sync::Mutex;
use std::thread;
use std::vec::Vec;

/// Scarlet input-device stream opened through the native handle API.
struct InputDevice {
    handle: Handle,
}

impl InputDevice {
    /// Open an input device for reading.
    fn open(path: &str) -> HandleResult<Self> {
        Handle::open(path, 0).map(|handle| Self { handle })
    }

    /// Read bytes from the device stream.
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        self.handle
            .as_stream()
            .map_err(|_| StreamError::Unsupported)?
            .read(buffer)
    }

    /// Change whether reads block while no input event is available.
    fn set_nonblocking(&self, enabled: bool) -> HandleResult<()> {
        self.handle.set_nonblocking(enabled)
    }
}

/// Input event structure (16 bytes, matches kernel InputEvent)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct InputEvent {
    pub time: u64,  // 8 bytes - timestamp in nanoseconds
    pub type_: u16, // 2 bytes - event type
    pub code: u16,  // 2 bytes - event code
    pub value: i32, // 4 bytes - event value
}

impl InputEvent {
    pub const SIZE: usize = core::mem::size_of::<Self>();
}

/// Processed input events for compositor
#[derive(Debug, Clone)]
pub enum CompositorInputEvent {
    MouseMove {
        dx: i32,
        dy: i32,
    },
    MouseButton {
        button: u16,
        pressed: bool,
    },
    MouseAbsolute {
        x: i32,
        y: i32,
    },
    /// Mouse wheel/scroll event
    MouseWheel {
        dx: i32,
        dy: i32,
    },
    Keyboard {
        code: u16,
        pressed: bool,
    },
}

/// Global input event queue
static INPUT_EVENT_QUEUE: Mutex<Vec<CompositorInputEvent>> = Mutex::new(Vec::new());
static SCREEN_WIDTH: AtomicU32 = AtomicU32::new(1);
static SCREEN_HEIGHT: AtomicU32 = AtomicU32::new(1);

/// Update the screen size used to scale absolute input devices.
pub fn set_screen_size(width: u32, height: u32) {
    SCREEN_WIDTH.store(width.max(1), Ordering::Relaxed);
    SCREEN_HEIGHT.store(height.max(1), Ordering::Relaxed);
}

/// Add an input event to the global queue
pub fn push_input_event(event: CompositorInputEvent) {
    let mut queue = INPUT_EVENT_QUEUE.lock().expect("SWS mutex poisoned");
    let should_wake = queue.is_empty();
    queue.push(event);
    drop(queue);
    if should_wake {
        super::ipc::wake_compositor();
    }
}

/// Get all pending input events from the queue
pub fn pop_all_input_events() -> Vec<CompositorInputEvent> {
    let mut queue = INPUT_EVENT_QUEUE.lock().expect("SWS mutex poisoned");
    core::mem::take(&mut *queue)
}

/// Return whether the compositor input queue has pending events.
///
/// # Returns
///
/// `true` if input events are queued for the compositor.
pub fn has_pending_input_events() -> bool {
    !INPUT_EVENT_QUEUE
        .lock()
        .expect("SWS mutex poisoned")
        .is_empty()
}

/// Event types
pub mod event_types {
    pub const EV_SYN: u16 = 0x00;
    pub const EV_KEY: u16 = 0x01;
    pub const EV_REL: u16 = 0x02;
    pub const EV_ABS: u16 = 0x03;
}

/// Synchronization event codes
pub mod syn_codes {
    pub const SYN_REPORT: u16 = 0;
}

/// Relative axis codes
pub mod rel_codes {
    pub const REL_X: u16 = 0x00;
    pub const REL_Y: u16 = 0x01;
    pub const REL_HWHEEL: u16 = 0x06;
    pub const REL_WHEEL: u16 = 0x08;
    pub const REL_HWHEEL_HI_RES: u16 = 0x0c;
    pub const REL_WHEEL_HI_RES: u16 = 0x0b;
}

/// Surface pixels per wheel notch (compositor-side scaling, matches Mutter).
pub const WHEEL_PIXELS_PER_NOTCH: i32 = 10;

/// Absolute axis codes
pub mod abs_codes {
    pub const ABS_X: u16 = 0x00;
    pub const ABS_Y: u16 = 0x01;
}

/// Key codes
#[allow(dead_code)]
pub mod key_codes {
    pub const BTN_LEFT: u16 = 0x110;
    #[allow(dead_code)]
    pub const BTN_RIGHT: u16 = 0x111;
    #[allow(dead_code)]
    pub const BTN_MIDDLE: u16 = 0x112;
    pub const KEY_ESC: u16 = 0x01;
    pub const KEY_1: u16 = 0x02;
    pub const KEY_2: u16 = 0x03;
    pub const KEY_3: u16 = 0x04;
    pub const KEY_4: u16 = 0x05;
    pub const KEY_5: u16 = 0x06;
    pub const KEY_6: u16 = 0x07;
    pub const KEY_7: u16 = 0x08;
    pub const KEY_8: u16 = 0x09;
    pub const KEY_9: u16 = 0x0a;
    pub const KEY_0: u16 = 0x0b;
    pub const KEY_MINUS: u16 = 0x0c;
    pub const KEY_EQUAL: u16 = 0x0d;
    pub const KEY_BACKSPACE: u16 = 0x0e;
    pub const KEY_TAB: u16 = 0x0f;
    pub const KEY_LEFTBRACE: u16 = 0x1a;
    pub const KEY_RIGHTBRACE: u16 = 0x1b;
    pub const KEY_ENTER: u16 = 0x1c;
    pub const KEY_SPACE: u16 = 0x39;
    pub const KEY_LEFTCTRL: u16 = 0x1d;
    pub const KEY_RIGHTCTRL: u16 = 0x61;
    pub const KEY_LEFTSHIFT: u16 = 0x2a;
    pub const KEY_RIGHTSHIFT: u16 = 0x36;
    pub const KEY_LEFTALT: u16 = 0x38;
    pub const KEY_RIGHTALT: u16 = 0x64;
    pub const KEY_ZENKAKUHANKAKU: u16 = 0x55;
    pub const KEY_HENKAN: u16 = 0x5c;
    pub const KEY_MUHENKAN: u16 = 0x5e;
    pub const KEY_HANGUEL: u16 = 0x7a;
    pub const KEY_LEFTMETA: u16 = 0x7d;
    pub const KEY_RIGHTMETA: u16 = 0x7e;
    pub const KEY_BACKSLASH: u16 = 0x2b;
    pub const KEY_SEMICOLON: u16 = 0x27;
    pub const KEY_APOSTROPHE: u16 = 0x28;
    pub const KEY_GRAVE: u16 = 0x29;
    pub const KEY_COMMA: u16 = 0x33;
    pub const KEY_DOT: u16 = 0x34;
    pub const KEY_SLASH: u16 = 0x35;
    pub const KEY_CAPSLOCK: u16 = 0x3a;
}

/// Input manager - handles input devices and event reading
pub struct InputManager {
    mouse_file: InputDevice,
    /// Maximum value for tablet absolute coordinates (typically 32767)
    tablet_max: i32,
    /// Current accumulated position for absolute positioning
    pub abs_x: Option<i32>,
    pub abs_y: Option<i32>,
    /// Accumulated horizontal wheel delta (from REL_HWHEEL)
    wheel_dx: i32,
    /// Accumulated vertical wheel delta (from REL_WHEEL)
    wheel_dy: i32,
}

impl InputManager {
    /// Create a new input manager
    pub fn new() -> Result<Self, &'static str> {
        // Try to open tablet device first (absolute positioning), fallback to touchpad or mouse (relative)
        let mouse_file = match InputDevice::open("/dev/tablet0") {
            Ok(file) => {
                println!("[InputManager] Opened tablet device (absolute positioning)");
                file
            }
            Err(_) => {
                // Try touchpad (behaves like a relative mouse)
                match InputDevice::open("/dev/touchpad0") {
                    Ok(file) => {
                        println!("[InputManager] Opened touchpad device (relative positioning)");
                        file
                    }
                    Err(_) => {
                        println!(
                            "[InputManager] Touchpad/tablet not found, trying mouse device..."
                        );
                        InputDevice::open("/dev/mouse0")
                            .map_err(|_| "Failed to open mouse, tablet, or touchpad device")?
                    }
                }
            }
        };

        Ok(Self {
            mouse_file,
            tablet_max: 32767, // Standard virtio-tablet range
            abs_x: None,
            abs_y: None,
            wheel_dx: 0,
            wheel_dy: 0,
        })
    }

    /// Read a single input event (blocking)
    pub fn read_event(&mut self) -> Result<Option<InputEvent>, &'static str> {
        let mut buffer = [0u8; InputEvent::SIZE];

        let bytes_read = self.mouse_file.read(&mut buffer).map_err(|e| {
            println!("[InputManager] Read error: {:?}", e);
            "Failed to read input event"
        })?;

        if bytes_read != InputEvent::SIZE {
            return Ok(None);
        }

        // SAFETY: the kernel filled the complete fixed-size event record, every
        // bit pattern is valid for its integer fields, and `read_unaligned`
        // does not require the byte buffer to have `InputEvent` alignment.
        let event = unsafe { core::ptr::read_unaligned(buffer.as_ptr() as *const InputEvent) };
        Ok(Some(event))
    }

    /// Try to read a single input event without blocking.
    /// Returns Ok(Some(event)) if available, Ok(None) if no event pending.
    pub fn try_read_event(&mut self) -> Result<Option<InputEvent>, &'static str> {
        let mut buffer = [0u8; InputEvent::SIZE];

        let _ = self.mouse_file.set_nonblocking(true);
        let result = self.mouse_file.read(&mut buffer);
        let _ = self.mouse_file.set_nonblocking(false);

        match result {
            Ok(bytes_read) if bytes_read == InputEvent::SIZE => {
                // SAFETY: the complete initialized record has integer-only
                // fields, and unaligned reads are valid for this byte buffer.
                let event =
                    unsafe { core::ptr::read_unaligned(buffer.as_ptr() as *const InputEvent) };
                Ok(Some(event))
            }
            _ => Ok(None),
        }
    }

    /// Scale tablet coordinates to screen coordinates
    pub fn scale_tablet_coord(&self, value: i32, screen_dimension: u32) -> i32 {
        ((value as i64 * screen_dimension as i64) / self.tablet_max as i64) as i32
    }

    /// Start input processing thread
    pub fn start_input_thread(screen_width: u32, screen_height: u32) -> Result<(), &'static str> {
        println!("[InputManager] Starting input thread...");
        set_screen_size(screen_width, screen_height);

        thread::Builder::new()
            .spawn(move || {
                input_thread_main();
            })
            .map_err(|_| "Failed to start pointer input thread")?;

        thread::Builder::new()
            .spawn(move || {
                keyboard_thread_main();
            })
            .map_err(|_| "Failed to start keyboard input thread")?;

        println!("[InputManager] Input thread started");
        Ok(())
    }
}

/// Input thread main function
fn input_thread_main() {
    println!("[InputThread] Started");

    let mut attempts = 0usize;
    let mut input_manager = loop {
        match InputManager::new() {
            Ok(mgr) => break mgr,
            Err(e) => {
                attempts += 1;
                if attempts == 1 || attempts % 20 == 0 {
                    println!(
                        "[InputThread] Waiting for pointer device: {} (attempt {})",
                        e, attempts
                    );
                }
                thread::sleep(core::time::Duration::from_millis(250));
            }
        }
    };

    loop {
        super::trace::input_loop();
        match input_manager.read_event() {
            Ok(Some(event)) => {
                super::trace::input_event();
                process_mouse_event(&mut input_manager, event);

                while let Ok(Some(event)) = input_manager.try_read_event() {
                    super::trace::input_event();
                    process_mouse_event(&mut input_manager, event);
                }

                thread::sleep(core::time::Duration::from_millis(16));
            }
            Ok(None) => {
                // A disconnected or temporarily incomplete input device may
                // return a short read immediately. Do not turn that condition
                // into an unbounded userspace polling loop.
                super::trace::input_empty();
                thread::sleep(core::time::Duration::from_millis(10));
            }
            Err(e) => {
                println!("[InputThread] Error reading event: {}", e);
                break;
            }
        }
    }

    println!("[InputThread] Exited");
}

fn process_mouse_event(input_manager: &mut InputManager, event: InputEvent) {
    match event.type_ {
        event_types::EV_REL => match event.code {
            rel_codes::REL_X => {
                push_input_event(CompositorInputEvent::MouseMove {
                    dx: event.value,
                    dy: 0,
                });
            }
            rel_codes::REL_Y => {
                push_input_event(CompositorInputEvent::MouseMove {
                    dx: 0,
                    dy: event.value,
                });
            }
            rel_codes::REL_WHEEL => {
                input_manager.wheel_dy = input_manager.wheel_dy.saturating_add(event.value);
            }
            rel_codes::REL_HWHEEL => {
                input_manager.wheel_dx = input_manager.wheel_dx.saturating_add(event.value);
            }
            _ => {}
        },
        event_types::EV_ABS => match event.code {
            abs_codes::ABS_X => {
                input_manager.abs_x = Some(event.value);
                if let (Some(x), Some(y)) = (input_manager.abs_x, input_manager.abs_y) {
                    let screen_width = SCREEN_WIDTH.load(Ordering::Relaxed);
                    let screen_height = SCREEN_HEIGHT.load(Ordering::Relaxed);
                    let screen_x = input_manager.scale_tablet_coord(x, screen_width);
                    let screen_y = input_manager.scale_tablet_coord(y, screen_height);
                    push_input_event(CompositorInputEvent::MouseAbsolute {
                        x: screen_x,
                        y: screen_y,
                    });
                }
            }
            abs_codes::ABS_Y => {
                input_manager.abs_y = Some(event.value);
                if let (Some(x), Some(y)) = (input_manager.abs_x, input_manager.abs_y) {
                    let screen_width = SCREEN_WIDTH.load(Ordering::Relaxed);
                    let screen_height = SCREEN_HEIGHT.load(Ordering::Relaxed);
                    let screen_x = input_manager.scale_tablet_coord(x, screen_width);
                    let screen_y = input_manager.scale_tablet_coord(y, screen_height);
                    push_input_event(CompositorInputEvent::MouseAbsolute {
                        x: screen_x,
                        y: screen_y,
                    });
                }
            }
            _ => {}
        },
        event_types::EV_KEY => {
            let pressed = event.value == 1;
            push_input_event(CompositorInputEvent::MouseButton {
                button: event.code,
                pressed,
            });
        }
        event_types::EV_SYN => match event.code {
            syn_codes::SYN_REPORT => {
                if input_manager.wheel_dx != 0 || input_manager.wheel_dy != 0 {
                    push_input_event(CompositorInputEvent::MouseWheel {
                        dx: input_manager.wheel_dx,
                        dy: input_manager.wheel_dy,
                    });
                    input_manager.wheel_dx = 0;
                    input_manager.wheel_dy = 0;
                }
            }
            _ => {}
        },
        _ => {}
    }
}

/// Keyboard thread main function
fn keyboard_thread_main() {
    println!("[KeyboardThread] Started");

    let mut attempts = 0usize;
    let mut keyboard_file = loop {
        match InputDevice::open("/dev/keyboard0") {
            Ok(file) => {
                println!("[KeyboardThread] Opened keyboard device");
                break file;
            }
            Err(e) => {
                attempts += 1;
                if attempts == 1 || attempts % 20 == 0 {
                    println!(
                        "[KeyboardThread] Waiting for keyboard device: {:?} (attempt {})",
                        e, attempts
                    );
                }
                thread::sleep(core::time::Duration::from_millis(250));
            }
        }
    };

    loop {
        super::trace::keyboard_loop();
        let mut buffer = [0u8; InputEvent::SIZE];

        match keyboard_file.read(&mut buffer) {
            Ok(bytes_read) => {
                if bytes_read != InputEvent::SIZE {
                    super::trace::keyboard_short_read();
                    thread::sleep(core::time::Duration::from_millis(10));
                    continue;
                }

                super::trace::keyboard_event();
                // SAFETY: the complete initialized record has integer-only
                // fields, and unaligned reads are valid for this byte buffer.
                let event =
                    unsafe { core::ptr::read_unaligned(buffer.as_ptr() as *const InputEvent) };

                match event.type_ {
                    event_types::EV_KEY => {
                        let pressed = event.value == 1 || event.value == 2;
                        push_input_event(CompositorInputEvent::Keyboard {
                            code: event.code,
                            pressed,
                        });
                    }
                    _ => {}
                }

                let _ = keyboard_file.set_nonblocking(true);
                loop {
                    let mut buf = [0u8; InputEvent::SIZE];
                    match keyboard_file.read(&mut buf) {
                        Ok(n) if n == InputEvent::SIZE => {
                            super::trace::keyboard_event();
                            // SAFETY: the complete initialized record has integer-only
                            // fields, and unaligned reads are valid for this byte buffer.
                            let ev = unsafe {
                                core::ptr::read_unaligned(buf.as_ptr() as *const InputEvent)
                            };
                            if ev.type_ == event_types::EV_KEY {
                                let pressed = ev.value == 1 || ev.value == 2;
                                push_input_event(CompositorInputEvent::Keyboard {
                                    code: ev.code,
                                    pressed,
                                });
                            }
                        }
                        _ => break,
                    }
                }
                let _ = keyboard_file.set_nonblocking(false);

                thread::sleep(core::time::Duration::from_millis(16));
            }
            Err(e) => {
                println!("[KeyboardThread] Error reading event: {:?}", e);
                break;
            }
        }
    }

    println!("[KeyboardThread] Exited");
}
