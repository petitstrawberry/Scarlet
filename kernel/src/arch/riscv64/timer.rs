use core::arch::asm;

use crate::{arch::{get_cpu, instruction::sbi::sbi_set_timer}, environment::RISCV_STIMER_FREQ, interrupt::InterruptManager};

pub type ArchTimer = Stimer;

pub struct Stimer {
    pub next_event: u64,
    pub running: bool,
    frequency: u64
}

impl Stimer {
    pub fn new() -> Self {
        let freq = InterruptManager::with_manager(|manager| {
            let cpu_id = get_cpu().get_cpuid() as u32;
            match manager.get_timer_frequency_hz(cpu_id) {
                Ok(freq) => freq,
                Err(e) => {
                    panic!("Failed to get timer frequency: {}", e);
                }
            }
        });

        Stimer {
            next_event: 0,
            running: false,
            frequency: freq
        }
    }

    pub fn set_interval_us(&mut self, interval: u64) {
        let current = self.get_time();
        self.set_next_event(current + (interval * self.frequency / 1000000));
    }

    pub fn start(&mut self) {
        self.running = true;
        InterruptManager::with_manager(|manager| {
            let cpu_id = get_cpu().get_cpuid() as u32;
            if manager.set_timer(cpu_id, self.get_next_event()).is_err() {
                panic!("Failed to set timer for CPU {}", cpu_id);
            }
        });

        let mut sie: usize;
        unsafe {
            asm!(
                "csrr {0}, sie",
                out(reg) sie,
            );
            /* Enable timer interrupt */
            sie |= 1 << 5;
            asm!(
                "csrw sie, {0}",
                in(reg) sie,
            );
        }
    }

    pub fn stop(&mut self) {
        self.running = false;
        InterruptManager::with_manager(|manager| {
            let cpu_id = get_cpu().get_cpuid() as u32;
            if manager.set_timer(cpu_id, 0xFFFFFFFFFFFFFFFF).is_err() {
                panic!("Failed to stop timer for CPU {}", cpu_id);
            }
        });

        let mut sie: usize;
        unsafe {
            asm!(
                "csrr {0}, sie",
                out(reg) sie,
            );
            /* Disable timer interrupt */
            sie &= !(1 << 5);
            asm!(
                "csrw sie, {0}",
                in(reg) sie,
            );
        }
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    fn get_next_event(&self) -> u64 {
        self.next_event
    }

    pub fn get_time_us(&self) -> u64 {
        (self.get_time() * 1_000_000) / self.frequency
    }

    /// Get the current clock time
    fn get_time(&self) -> u64 {
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
