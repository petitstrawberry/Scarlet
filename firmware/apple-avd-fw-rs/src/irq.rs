//! Cortex-M3 interrupt handling for the Apple AVD firmware.

use crate::abi::{MSG_PP_DONE, MSG_UNKNOWN_IRQ, MSG_VP_DONE, MSG_VP_ERROR};
use crate::mailbox::send_message;

/// Number of external NVIC lines enabled by the skeleton firmware.
pub const NVIC_EXTERNAL_IRQS: usize = 64;

const NVIC_ISER_BASE: usize = 0xe000_e100;

/// Enable all known AVD NVIC external IRQ lines.
pub fn enable_all_nvic_irqs() {
    for word in 0..(NVIC_EXTERNAL_IRQS / 32) {
        let reg = (NVIC_ISER_BASE + word * 4) as *mut u32;
        // SAFETY: NVIC ISER registers are memory-mapped Cortex-M system control registers.
        unsafe {
            core::ptr::write_volatile(reg, u32::MAX);
        }
    }
}

/// Handle a video-pipe DONE IRQ.
///
/// # Arguments
///
/// * `pipe` - Hardware video pipe index.
pub fn video_pipe_done(pipe: u32) {
    send_message(MSG_VP_DONE | (pipe & 0xff));
}

/// Handle a video-pipe ERROR IRQ.
///
/// # Arguments
///
/// * `pipe` - Hardware video pipe index.
pub fn video_pipe_error(pipe: u32) {
    send_message(MSG_VP_ERROR | (pipe & 0xff));
}

/// Handle a post-process DONE IRQ.
pub fn post_process_done() {
    send_message(MSG_PP_DONE);
}

/// Handle an unexpected external IRQ.
///
/// # Arguments
///
/// * `irq` - IRQ number reported by the vector entry.
pub fn unknown_irq(irq: u32) {
    send_message(MSG_UNKNOWN_IRQ | (irq & 0xff));
}
