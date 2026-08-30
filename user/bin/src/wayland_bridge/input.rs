//! Wayland Input Protocol Support (wl_seat, wl_pointer, wl_keyboard)

use std::collections::BTreeMap;

/// Input device seat
#[derive(Debug)]
pub struct Seat {
    pub seat_id: u32,
    pub name: &'static str,
    pub capabilities: u32,
}

/// Pointer (mouse) device
#[derive(Debug)]
pub struct Pointer {
    pub pointer_id: u32,
    pub seat_id: u32,
}

/// Keyboard device
#[derive(Debug)]
pub struct Keyboard {
    pub keyboard_id: u32,
    pub seat_id: u32,
}

/// Input manager for Wayland
pub struct InputManager {
    seats: BTreeMap<u32, Seat>,
    pointers: BTreeMap<u32, Pointer>,
    keyboards: BTreeMap<u32, Keyboard>,
}

impl InputManager {
    pub fn new() -> Self {
        Self {
            seats: BTreeMap::new(),
            pointers: BTreeMap::new(),
            keyboards: BTreeMap::new(),
        }
    }

    pub fn create_seat(&mut self, seat_id: u32, name: &'static str) {
        let seat = Seat {
            seat_id,
            name,
            capabilities: seat_capabilities::POINTER | seat_capabilities::KEYBOARD,
        };
        self.seats.insert(seat_id, seat);
    }

    pub fn get_seat(&self, seat_id: u32) -> Option<&Seat> {
        self.seats.get(&seat_id)
    }

    pub fn create_pointer(&mut self, pointer_id: u32, seat_id: u32) {
        let pointer = Pointer {
            pointer_id,
            seat_id,
        };
        self.pointers.insert(pointer_id, pointer);
    }

    pub fn pointer_seat_id(&self, pointer_id: u32) -> Option<u32> {
        self.pointers
            .get(&pointer_id)
            .map(|pointer| pointer.seat_id)
    }

    pub fn create_keyboard(&mut self, keyboard_id: u32, seat_id: u32) {
        let keyboard = Keyboard {
            keyboard_id,
            seat_id,
        };
        self.keyboards.insert(keyboard_id, keyboard);
    }
}

/// wl_seat capabilities
pub mod seat_capabilities {
    pub const POINTER: u32 = 1;
    pub const KEYBOARD: u32 = 2;
    pub const TOUCH: u32 = 4;
}

/// wl_seat requests
pub mod seat_request {
    pub const GET_POINTER: u16 = 0;
    pub const GET_KEYBOARD: u16 = 1;
    pub const GET_TOUCH: u16 = 2;
    pub const RELEASE: u16 = 3;
}

/// wl_seat events
pub mod seat_event {
    pub const CAPABILITIES: u16 = 0;
    pub const NAME: u16 = 1;
}

/// wl_pointer button state
pub mod pointer_button_state {
    pub const RELEASED: u32 = 0;
    pub const PRESSED: u32 = 1;
}

/// wl_pointer events
pub mod pointer_event {
    pub const ENTER: u16 = 0;
    pub const LEAVE: u16 = 1;
    pub const MOTION: u16 = 2;
    pub const BUTTON: u16 = 3;
    pub const AXIS: u16 = 4;
    pub const FRAME: u16 = 5;
}

/// wl_keyboard events
pub mod keyboard_event {
    pub const KEYMAP: u16 = 0;
    pub const ENTER: u16 = 1;
    pub const LEAVE: u16 = 2;
    pub const KEY: u16 = 3;
    pub const MODIFIERS: u16 = 4;
    pub const REPEAT_INFO: u16 = 5;
}
