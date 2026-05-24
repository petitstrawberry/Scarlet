extern crate alloc;

use alloc::sync::Arc;
use core::time::Duration;
use scarlet_std::handle::Handle;
use scarlet_std::hypervisor::Vcpu;
use scarlet_std::sync::Mutex;
use scarlet_std::thread;
use scarlet_std::tty::{KeyboardMode, ReadPolicy, Terminal};

use crate::devices::uart::Ns16550a;

pub const TIMER_IRQ_TYPE: usize = 1;
pub const EXTERNAL_IRQ_TYPE: usize = 2;
const TIMEBASE_FREQ: u64 = 10_000_000;

pub struct TimerState {
    next_timer: Option<u64>,
    vcpu_handle: Option<u32>,
}

impl TimerState {
    pub fn new() -> Self {
        Self {
            next_timer: None,
            vcpu_handle: None,
        }
    }

    pub fn set_timer(&mut self, stime_value: u64) {
        self.next_timer = Some(stime_value);
    }

    pub fn set_vcpu_handle(&mut self, handle: u32) {
        self.vcpu_handle = Some(handle);
    }

    pub fn next_timer(&self) -> Option<u64> {
        self.next_timer
    }

    pub fn clear_timer(&mut self) {
        self.next_timer = None;
    }

    pub fn vcpu_handle(&self) -> Option<u32> {
        self.vcpu_handle
    }
}

pub fn start_timer_thread(state: Arc<Mutex<TimerState>>) {
    thread::spawn(move || {
        timer_loop(state);
    });
}

pub fn start_uart_thread(uart: Arc<Ns16550a>, vcpu: Arc<Vcpu>) {
    thread::spawn(move || {
        uart_loop(uart, vcpu);
    });
}

fn set_raw_mode() {
    if let Ok(stdin_handle) = unsafe { Handle::from_raw(0) } {
        let terminal = Terminal::from_handle(&stdin_handle);
        let _ = terminal.set_canonical(false);
        let _ = terminal.set_echo(false);
        let _ = terminal.set_keyboard_mode(KeyboardMode::Xlate);
        let _ = terminal.set_read_policy(ReadPolicy::new(1, 0));
        core::mem::forget(stdin_handle);
    }
}

fn uart_loop(uart: Arc<Ns16550a>, vcpu: Arc<Vcpu>) {
    set_raw_mode();

    let stdin = scarlet_std::io::stdin();
    let mut buf = [0u8; 1];

    loop {
        if let Ok(1) = stdin.read(&mut buf) {
            uart.trigger_rx_with_byte(buf[0]);
            let _ = vcpu.inject_interrupt(EXTERNAL_IRQ_TYPE);
        }
    }
}

fn inject_timer_interrupt(vcpu_handle: u32) {
    use scarlet_std::syscall::{Syscall, syscall3};
    const VCPU_CTL_INJECT_INTERRUPT: u32 = 0x04;
    let _ = syscall3(
        Syscall::HandleControl,
        vcpu_handle as usize,
        VCPU_CTL_INJECT_INTERRUPT as usize,
        TIMER_IRQ_TYPE,
    );
}

fn timer_loop(state: Arc<Mutex<TimerState>>) {
    loop {
        let sleep_duration = {
            let s = state.lock();
            if let Some(next) = s.next_timer {
                let now = read_time();
                if now >= next {
                    Duration::ZERO
                } else {
                    cycles_to_duration(next - now)
                }
            } else {
                Duration::from_millis(100)
            }
        };

        if sleep_duration.is_zero() {
            let mut s = state.lock();
            s.clear_timer();
            if let Some(handle) = s.vcpu_handle() {
                inject_timer_interrupt(handle);
            }
        } else {
            thread::sleep(sleep_duration);
        }
    }
}

fn read_time() -> u64 {
    let time: u64;
    unsafe {
        core::arch::asm!(
            "rdtime {0}",
            out(reg) time,
            options(nostack)
        );
    }
    time
}

fn cycles_to_duration(cycles: u64) -> Duration {
    let ns = cycles * 1_000_000_000 / TIMEBASE_FREQ;
    Duration::from_nanos(ns)
}
