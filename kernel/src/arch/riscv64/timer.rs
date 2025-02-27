use core::arch::asm;

use crate::{arch::instruction::sbi::sbi_set_timer, environment::RISCV_STIMER_FREQ};

pub type ArchTimer = Stimer;

pub struct Stimer {
    pub next_event: u64,
    pub running: bool,
}

impl Stimer {
    pub const fn new() -> Self {
        Stimer {
            next_event: 0,
            running: false,
        }
    }

    pub fn set_interval_us(&mut self, interval: u64) {
        let current = self.get_time();
        
        self.set_next_event(current + (interval * RISCV_STIMER_FREQ) / 1000000);
    }

    pub fn start(&mut self) {
        self.running = true;
        sbi_set_timer(self.next_event);
    }

    pub fn stop(&mut self) {
        self.running = false;
        sbi_set_timer(0xffffffff_ffffffff);
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn get_next_event(&self) -> u64 {
        self.next_event
    }

    pub fn get_time(&self) -> u64 {
        let time: u64;
        unsafe {
            asm!(
                "rdtime {0}",
                out(reg) time,
            );
        }
        time
    }

    fn set_next_event(&mut self, next_event: u64) {
        self.next_event = next_event;
    }
}
