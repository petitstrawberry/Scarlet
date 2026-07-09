#![no_std]
#![no_main]

mod abi;
mod irq;
mod mailbox;
mod tunables;
mod vector;

use core::arch::asm;

use abi::MSG_PANIC;
use mailbox::{send_message, signal_booted};

/// Firmware reset entry point.
#[unsafe(no_mangle)]
pub extern "C" fn reset_handler() -> ! {
    tunables::apply_selected_tunables();
    irq::enable_all_nvic_irqs();
    enable_interrupts();
    signal_booted();

    loop {
        wait_for_interrupt();
    }
}

/// Panic handler for firmware faults that reach Rust.
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    send_message(MSG_PANIC);
    loop {
        wait_for_interrupt();
    }
}

#[inline(always)]
fn enable_interrupts() {
    // SAFETY: `cpsie i` enables normal IRQ delivery on Cortex-M.
    unsafe {
        asm!("cpsie i", options(nomem, nostack, preserves_flags));
    }
}

#[inline(always)]
fn wait_for_interrupt() {
    // SAFETY: `wfi` waits for the next interrupt and has no Rust-visible side effects.
    unsafe {
        asm!("wfi", options(nomem, nostack, preserves_flags));
    }
}
