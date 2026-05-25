//! Input event handling module

use core::sync::atomic::{AtomicU32, Ordering};
use std::fs::File;
use std::println;
use std::sync::Mutex;
use std::thread;
use std::vec::Vec;

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
    MouseMove { dx: i32, dy: i32 },
    MouseButton { button: u16, pressed: bool },
    MouseAbsolute { x: i32, y: i32 },
    Keyboard { code: u16, pressed: bool },
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
    let mut queue = INPUT_EVENT_QUEUE.lock();
    let should_wake = queue.is_empty();
    queue.push(event);
    drop(queue);
    if should_wake {
        super::ipc::wake_compositor();
    }
}

/// Get all pending input events from the queue
pub fn pop_all_input_events() -> Vec<CompositorInputEvent> {
    let mut queue = INPUT_EVENT_QUEUE.lock();
    core::mem::take(&mut *queue)
}

/// Event types
pub mod event_types {
    pub const EV_SYN: u16 = 0x00;
    pub const EV_KEY: u16 = 0x01;
    pub const EV_REL: u16 = 0x02;
    pub const EV_ABS: u16 = 0x03;
}

/// Relative axis codes
pub mod rel_codes {
    pub const REL_X: u16 = 0x00;
    pub const REL_Y: u16 = 0x01;
    pub const REL_WHEEL: u16 = 0x08;
}

/// Absolute axis codes
pub mod abs_codes {
    pub const ABS_X: u16 = 0x00;
    pub const ABS_Y: u16 = 0x01;
}

/// Key codes
pub mod key_codes {
    pub const BTN_LEFT: u16 = 0x110;
    pub const BTN_RIGHT: u16 = 0x111;
    pub const BTN_MIDDLE: u16 = 0x112;
}

/// Input manager - handles input devices and event reading
pub struct InputManager {
    mouse_file: File,
    keyboard_file: Option<File>,
    /// Maximum value for tablet absolute coordinates (typically 32767)
    tablet_max: i32,
    /// Current accumulated position for absolute positioning
    pub abs_x: Option<i32>,
    pub abs_y: Option<i32>,
}

impl InputManager {
    /// Create a new input manager
    pub fn new() -> Result<Self, &'static str> {
        // Try to open tablet device first (absolute positioning), fallback to mouse (relative)
        let mouse_file = match File::open("/dev/tablet0") {
            Ok(file) => {
                println!("[InputManager] Opened tablet device (absolute positioning)");
                file
            }
            Err(_) => {
                println!("[InputManager] Tablet device not found, trying mouse device...");
                File::open("/dev/mouse0").map_err(|_| "Failed to open mouse or tablet device")?
            }
        };

        // Try to open keyboard device
        let keyboard_file = match File::open("/dev/keyboard0") {
            Ok(file) => {
                println!("[InputManager] Opened keyboard device");
                Some(file)
            }
            Err(_) => {
                println!("[InputManager] Keyboard device not found, keyboard input unavailable");
                None
            }
        };

        Ok(Self {
            mouse_file,
            keyboard_file,
            tablet_max: 32767, // Standard virtio-tablet range
            abs_x: None,
            abs_y: None,
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

        let event = unsafe { core::ptr::read(buffer.as_ptr() as *const InputEvent) };
        Ok(Some(event))
    }

    /// Try to read a single input event without blocking.
    /// Returns Ok(Some(event)) if available, Ok(None) if no event pending.
    pub fn try_read_event(&mut self) -> Result<Option<InputEvent>, &'static str> {
        let mut buffer = [0u8; InputEvent::SIZE];

        self.mouse_file.set_nonblocking(true);
        let result = self.mouse_file.read(&mut buffer);
        self.mouse_file.set_nonblocking(false);

        match result {
            Ok(bytes_read) if bytes_read == InputEvent::SIZE => {
                let event = unsafe { core::ptr::read(buffer.as_ptr() as *const InputEvent) };
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

        thread::spawn(move || {
            input_thread_main();
        });

        // Start keyboard thread if keyboard device is available
        if File::open("/dev/keyboard0").is_ok() {
            thread::spawn(move || {
                keyboard_thread_main();
            });
        }

        println!("[InputManager] Input thread started");
        Ok(())
    }
}

/// Input thread main function
fn input_thread_main() {
    println!("[InputThread] Started");

    let mut input_manager = match InputManager::new() {
        Ok(mgr) => mgr,
        Err(e) => {
            println!("[InputThread] Failed to create InputManager: {}", e);
            return;
        }
    };

    loop {
        match input_manager.read_event() {
            Ok(Some(event)) => {
                process_mouse_event(&mut input_manager, event);

                while let Ok(Some(event)) = input_manager.try_read_event() {
                    process_mouse_event(&mut input_manager, event);
                }

                thread::sleep(core::time::Duration::from_millis(16));
            }
            Ok(None) => {}
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
        _ => {}
    }
}

/// Keyboard thread main function
fn keyboard_thread_main() {
    println!("[KeyboardThread] Started");

    let mut keyboard_file = match File::open("/dev/keyboard0") {
        Ok(file) => file,
        Err(e) => {
            println!("[KeyboardThread] Failed to open keyboard device: {:?}", e);
            return;
        }
    };

    loop {
        let mut buffer = [0u8; InputEvent::SIZE];

        match keyboard_file.read(&mut buffer) {
            Ok(bytes_read) => {
                if bytes_read != InputEvent::SIZE {
                    continue;
                }

                let event = unsafe { core::ptr::read(buffer.as_ptr() as *const InputEvent) };

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

                keyboard_file.set_nonblocking(true);
                loop {
                    let mut buf = [0u8; InputEvent::SIZE];
                    match keyboard_file.read(&mut buf) {
                        Ok(n) if n == InputEvent::SIZE => {
                            let ev = unsafe { core::ptr::read(buf.as_ptr() as *const InputEvent) };
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
                keyboard_file.set_nonblocking(false);

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
