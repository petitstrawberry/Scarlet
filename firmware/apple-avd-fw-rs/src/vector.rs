//! Cortex-M3 vector table for the AVD firmware.

use core::arch::global_asm;

use crate::abi::MSG_PANIC;
use crate::irq;
use crate::mailbox::send_message;

global_asm!(
    r#"
    .section .vector_table, "a", %progbits
    .word __stack_top
    .word reset_handler
    .word nmi_handler
    .word hardfault_handler
    .word default_exception_handler
    .word default_exception_handler
    .word default_exception_handler
    .word 0
    .word 0
    .word 0
    .word 0
    .word default_exception_handler
    .word default_exception_handler
    .word 0
    .word default_exception_handler
    .word systick_handler
    .word irq0_handler
    .word irq1_handler
    .word irq2_handler
    .word irq3_handler
    "#
);

/// Non-maskable interrupt handler.
#[unsafe(no_mangle)]
pub extern "C" fn nmi_handler() -> ! {
    send_message(MSG_PANIC | 0x10);
    loop {
        core::hint::spin_loop();
    }
}

/// HardFault handler.
#[unsafe(no_mangle)]
pub extern "C" fn hardfault_handler() -> ! {
    send_message(MSG_PANIC | 0x20);
    loop {
        core::hint::spin_loop();
    }
}

/// Default exception handler.
#[unsafe(no_mangle)]
pub extern "C" fn default_exception_handler() {
    send_message(MSG_PANIC | 0x30);
}

/// SysTick handler.
#[unsafe(no_mangle)]
pub extern "C" fn systick_handler() {
    irq::unknown_irq(15);
}

/// External IRQ 0 handler.
#[unsafe(no_mangle)]
pub extern "C" fn irq0_handler() {
    irq::video_pipe_done(0);
}

/// External IRQ 1 handler.
#[unsafe(no_mangle)]
pub extern "C" fn irq1_handler() {
    irq::video_pipe_error(0);
}

/// External IRQ 2 handler.
#[unsafe(no_mangle)]
pub extern "C" fn irq2_handler() {
    irq::post_process_done();
}

/// External IRQ 3 handler.
#[unsafe(no_mangle)]
pub extern "C" fn irq3_handler() {
    irq::unknown_irq(3);
}
