//! Pure controlled-port incarnation rules, shared by the data path and host tests.
#[derive(Debug)]
pub struct Gate {
    pub link_id: u64,
    pub epoch: u64,
    pub authorized: bool,
}
impl Gate {
    pub fn new() -> Self {
        Self {
            link_id: 0,
            epoch: 1,
            authorized: false,
        }
    }
    pub fn token(&self) -> Option<(u64, u64)> {
        (self.authorized && self.link_id != 0).then_some((self.link_id, self.epoch))
    }
    pub fn accepts(&self, token: Option<(u64, u64)>) -> bool {
        token.is_some() && self.token() == token
    }
    pub fn retire(&mut self) -> bool {
        self.authorized = false;
        match self.epoch.checked_add(1) {
            Some(epoch) => {
                self.epoch = epoch;
                true
            }
            None => false,
        }
    }
    pub fn authorize(&mut self) -> bool {
        if self.link_id == 0 || self.epoch == u64::MAX {
            return false;
        }
        self.authorized = true;
        true
    }
}
