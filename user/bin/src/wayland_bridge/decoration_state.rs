//! xdg-decoration state is latched with the acknowledged wl_surface commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Client = 1,
    Server = 2,
}

impl Mode {
    pub fn parse(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::Client),
            2 => Some(Self::Server),
            _ => None,
        }
    }
}

pub struct State {
    pub object: Option<u32>,
    pub active: Mode,
    pending: Option<(Mode, u32, bool)>,
    remove_on_commit: bool,
}

impl State {
    pub fn new(object: u32) -> Self {
        Self {
            object: Some(object),
            active: Mode::Client,
            pending: None,
            remove_on_commit: false,
        }
    }
    pub fn configure(&mut self, mode: Mode, serial: u32) {
        self.pending = Some((mode, serial, false));
    }
    pub fn ack(&mut self, serial: u32) {
        if let Some((_, expected, acknowledged)) = &mut self.pending {
            // A later configure includes the current decoration state too.
            if serial.wrapping_sub(*expected) < (1 << 31) {
                *acknowledged = true;
            }
        }
    }
    pub fn destroy(&mut self) {
        self.object = None;
        self.pending = None;
        self.remove_on_commit = true;
    }
    pub fn commit(&mut self) {
        if self.remove_on_commit {
            self.active = Mode::Client;
            self.remove_on_commit = false;
        } else if let Some((mode, _, true)) = self.pending {
            self.active = mode;
            self.pending = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mode_changes_wait_for_ack_and_commit() {
        let mut state = State::new(9);
        state.configure(Mode::Server, 10);
        state.commit();
        assert_eq!(state.active, Mode::Client);
        state.ack(9);
        state.commit();
        assert_eq!(state.active, Mode::Client);
        state.ack(10);
        assert_eq!(state.active, Mode::Client);
        state.commit();
        assert_eq!(state.active, Mode::Server);
        state.configure(Mode::Client, 11);
        state.ack(12);
        state.commit();
        assert_eq!(state.active, Mode::Client);
    }
    #[test]
    fn old_ack_cannot_accept_a_newer_mode_and_serials_wrap() {
        let mut state = State::new(9);
        state.configure(Mode::Server, u32::MAX);
        state.configure(Mode::Client, 1);
        state.ack(u32::MAX);
        state.commit();
        assert_eq!(state.active, Mode::Client);
        state.configure(Mode::Server, u32::MAX);
        state.ack(1);
        state.commit();
        assert_eq!(state.active, Mode::Server);
    }
    #[test]
    fn destroying_decoration_removes_frame_at_next_commit() {
        let mut state = State::new(9);
        state.configure(Mode::Server, 2);
        state.ack(2);
        state.commit();
        state.destroy();
        assert_eq!(state.active, Mode::Server);
        state.commit();
        assert_eq!(state.active, Mode::Client);
    }
    #[test]
    fn only_defined_modes_are_accepted() {
        assert_eq!(Mode::parse(0), None);
        assert_eq!(Mode::parse(1), Some(Mode::Client));
        assert_eq!(Mode::parse(2), Some(Mode::Server));
        assert_eq!(Mode::parse(3), None);
    }
}
