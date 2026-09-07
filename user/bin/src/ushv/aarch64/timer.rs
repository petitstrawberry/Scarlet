extern crate alloc;

use alloc::sync::Arc;
use scarlet_std::handle::Handle;
use scarlet_std::hypervisor::Vcpu;
use scarlet_std::println;
use scarlet_std::sync::Mutex;
use scarlet_std::thread;
use scarlet_std::tty::{KeyboardMode, ReadPolicy, Terminal};

use crate::devices::uart::Ns16550a;

pub const EXTERNAL_IRQ_TYPE: usize = 2;

pub struct TimerState {
    vcpu_handle: Option<u32>,
}

impl TimerState {
    pub fn new() -> Self {
        Self { vcpu_handle: None }
    }

    pub fn set_vcpu_handle(&mut self, handle: u32) {
        self.vcpu_handle = Some(handle);
    }
}

pub fn start_timer_thread(_state: Arc<Mutex<TimerState>>) {
    println!("[ushv] AArch64 virtual timer: hardware-assisted (no timer thread)");
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
