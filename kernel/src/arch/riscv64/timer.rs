use core::arch::asm;

use crate::arch::get_cpu;

pub type ArchTimer = Stimer;

#[inline]
fn saturating_u128_to_u64(value: u128) -> u64 {
    value.min(u64::MAX as u128) as u64
}

pub struct Stimer {
    pub next_event: u64,
    pub running: bool,
    frequency: u64,
}

impl Stimer {
    pub fn new() -> Self {
        let freq = {
            let cpu_id = get_cpu().get_cpuid() as u32;
            match crate::interrupt::InterruptManager::global().get_timer_frequency_hz(cpu_id) {
                Ok(freq) => freq,
                Err(e) => {
                    panic!("Failed to get timer frequency: {}", e);
                }
            }
        };

        Stimer {
            next_event: 0,
            running: false,
            frequency: freq,
        }
    }

    pub fn set_interval_us(&mut self, interval: u64) {
        let current = self.get_time();
        self.set_next_event(current + (interval * self.frequency / 1000000));
    }

    /// Program the next timer event at an absolute hardware-counter deadline.
    ///
    /// # Arguments
    ///
    /// * `deadline` - Absolute hardware-counter value to fire at.
    pub fn set_deadline(&mut self, deadline: u64) {
        self.set_next_event(deadline);
    }

    /// Program an absolute monotonic nanosecond deadline.
    ///
    /// Expired or very-near deadlines are moved at least one microsecond into
    /// the future so a stale queue head cannot create an interrupt storm.
    pub fn set_deadline_ns(&mut self, deadline_ns: u64) {
        let now = self.get_time();
        if self.frequency == 0 {
            self.set_next_event(now.saturating_add(1));
            return;
        }

        let deadline = saturating_u128_to_u64(
            (deadline_ns as u128).saturating_mul(self.frequency as u128) / 1_000_000_000,
        );
        let minimum_delta = self.frequency.div_ceil(1_000_000).max(1);
        self.set_next_event(deadline.max(now.saturating_add(minimum_delta)));
    }

    pub fn start(&mut self) {
        self.running = true;
        let cpu_id = get_cpu().get_cpuid() as u32;
        if crate::interrupt::InterruptManager::global()
            .set_timer(cpu_id, self.get_next_event())
            .is_err()
        {
            panic!("Failed to set timer for CPU {}", cpu_id);
        }

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
        let cpu_id = get_cpu().get_cpuid() as u32;
        if crate::interrupt::InterruptManager::global()
            .set_timer(cpu_id, 0xFFFFFFFFFFFFFFFF)
            .is_err()
        {
            panic!("Failed to stop timer for CPU {}", cpu_id);
        }

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

    pub fn get_time_ns(&self) -> u64 {
        if self.frequency == 0 {
            return 0;
        }
        saturating_u128_to_u64(
            (self.get_time() as u128).saturating_mul(1_000_000_000) / self.frequency as u128,
        )
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

#[cfg(test)]
mod tests {
    use super::saturating_u128_to_u64;

    #[test_case]
    fn counter_conversion_saturates_instead_of_truncating() {
        assert_eq!(saturating_u128_to_u64(u64::MAX as u128), u64::MAX);
        assert_eq!(saturating_u128_to_u64((u64::MAX as u128) + 1), u64::MAX);
    }
}
