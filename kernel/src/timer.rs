use crate::arch::timer::ArchTimer;
use crate::environment::NUM_OF_CPUS;

pub struct KernelTimer {
    pub core_local_timer: [ArchTimer; NUM_OF_CPUS],
    pub interval: u64,
}

static mut KERNEL_TIMER: Option<KernelTimer> = None;

pub fn get_kernel_timer() -> &'static mut KernelTimer {
    unsafe {
        match KERNEL_TIMER {
            Some(ref mut t) => t,
            None => {
                KERNEL_TIMER = Some(KernelTimer::new());
                get_kernel_timer()
            }
        }
    }
}

impl KernelTimer {
    const fn new() -> Self {
        KernelTimer {
            core_local_timer: [const { ArchTimer::new() }; NUM_OF_CPUS],
            interval: 0xffffffff_ffffffff,
        }
    }

    pub fn init(&mut self) {
        for i in 0..NUM_OF_CPUS {
            self.core_local_timer[i].stop();
        }
    }

    pub fn start(&mut self, cpu_id: usize) {
        self.core_local_timer[cpu_id].start();
    }

    pub fn stop(&mut self, cpu_id: usize) {
        self.core_local_timer[cpu_id].stop();
    }

    pub fn restart(&mut self, cpu_id: usize) {
        self.stop(cpu_id);
        self.start(cpu_id);
    }

    /* Set the interval in microseconds */
    pub fn set_interval_us(&mut self, cpu_id: usize, interval: u64) {
        self.core_local_timer[cpu_id].set_interval_us(interval);
    }

    pub fn get_time(&self, cpu_id: usize) -> u64 {
        self.core_local_timer[cpu_id].get_time()
    }
}

