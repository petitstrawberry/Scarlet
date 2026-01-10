//! Input event handling module

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
}

/// Global input event queue
static INPUT_EVENT_QUEUE: Mutex<Vec<CompositorInputEvent>> = Mutex::new(Vec::new());

/// Add an input event to the global queue
pub fn push_input_event(event: CompositorInputEvent) {
    let mut queue = INPUT_EVENT_QUEUE.lock();
    queue.push(event);
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

        Ok(Self {
            mouse_file,
            tablet_max: 32767, // Standard virtio-tablet range
            abs_x: None,
            abs_y: None,
        })
    }

    /// Read a single input event
    pub fn read_event(&mut self) -> Result<Option<InputEvent>, &'static str> {
        let mut buffer = [0u8; InputEvent::SIZE];

        let bytes_read = self.mouse_file.read(&mut buffer).map_err(|e| {
            println!("[InputManager] Read error: {:?}", e);
            "Failed to read input event"
        })?;

        if bytes_read != InputEvent::SIZE {
            return Ok(None); // No complete event available
        }

        // Parse event
        let event = unsafe { core::ptr::read(buffer.as_ptr() as *const InputEvent) };

        Ok(Some(event))
    }

    /// Scale tablet coordinates to screen coordinates
    pub fn scale_tablet_coord(&self, value: i32, screen_dimension: u32) -> i32 {
        ((value as i64 * screen_dimension as i64) / self.tablet_max as i64) as i32
    }

    /// Start input processing thread
    pub fn start_input_thread(screen_width: u32, screen_height: u32) -> Result<(), &'static str> {
        println!("[InputManager] Starting input thread...");

        thread::spawn(move || {
            input_thread_main(screen_width, screen_height);
        });

        println!("[InputManager] Input thread started");
        Ok(())
    }
}

/// Input thread main function
fn input_thread_main(screen_width: u32, screen_height: u32) {
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
                // Convert raw event to compositor event
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
            Ok(None) => {
                // No event, continue
            }
            Err(e) => {
                println!("[InputThread] Error reading event: {}", e);
                break;
            }
        }
    }

    println!("[InputThread] Exited");
}
