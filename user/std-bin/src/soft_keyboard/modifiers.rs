use sws_protocol::input_panel::{ALT, CTRL, SHIFT};

#[derive(Default)]
pub(crate) struct Modifiers {
    pub flags: u32,
    shift_locked: bool,
    last_shift_tap_ns: u64,
}
impl Modifiers {
    pub fn toggle(&mut self, flag: u32, time_ns: u64) {
        if flag != SHIFT {
            self.flags ^= flag;
            return;
        }
        const DOUBLE_TAP_NS: u64 = 350_000_000;
        let double_tap = self.last_shift_tap_ns != 0
            && time_ns.saturating_sub(self.last_shift_tap_ns) <= DOUBLE_TAP_NS;
        if self.shift_locked {
            self.shift_locked = false;
            self.flags &= !SHIFT;
            self.last_shift_tap_ns = 0;
        } else if double_tap {
            self.shift_locked = true;
            self.flags |= SHIFT;
            self.last_shift_tap_ns = 0;
        } else {
            self.flags ^= SHIFT;
            self.last_shift_tap_ns = time_ns;
        }
    }
    pub fn consume_stroke(&mut self) -> bool {
        if self.flags & SHIFT == 0 || self.shift_locked {
            return false;
        }
        self.flags &= !SHIFT;
        self.last_shift_tap_ns = 0;
        true
    }
    pub fn change_layer(&mut self) {
        // Ctrl/Alt may be selected on Fn before choosing a letter or symbol
        // on another page. Only Shift belongs to the old page's labels.
        self.flags &= CTRL | ALT;
        self.shift_locked = false;
        self.last_shift_tap_ns = 0;
    }
}
