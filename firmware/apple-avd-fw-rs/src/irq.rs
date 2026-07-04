//! Cortex-M3 interrupt handling for the Apple AVD firmware.

use crate::abi::{MSG_PP_DONE, MSG_UNKNOWN_IRQ, MSG_VP_DONE, MSG_VP_ERROR};
use crate::mailbox::send_message;

/// Number of external NVIC lines enabled by the AVD firmware.
pub const NVIC_EXTERNAL_IRQS: usize = 64;

const NVIC_ISER_BASE: usize = 0xe000_e100;
const AVD_DECODE_STATUS: usize = 0x0011_4060;
const AVD_POSTPROCESS_STATUS: usize = 0x0011_4064;
const DECODE_STATUS_DONE: u32 = 0x0084_2108;
const DECODE_STATUS_ERR: u32 = 0x0000_0003;
const POSTPROCESS_STATUS_DONE: u32 = 0x0000_0001;

/// Enable all known AVD NVIC external IRQ lines.
pub fn enable_all_nvic_irqs() {
    arm_decode_irqs();
}

/// Enable AVD decode IRQ delivery through NVIC.
pub fn arm_decode_irqs() {
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
    clear_decode_status(DECODE_STATUS_DONE);
    send_message(MSG_VP_DONE | (pipe & 0xff));
}

/// Handle a video-pipe ERROR IRQ.
///
/// # Arguments
///
/// * `pipe` - Hardware video pipe index.
pub fn video_pipe_error(pipe: u32) {
    clear_decode_status(DECODE_STATUS_ERR);
    send_message(MSG_VP_ERROR | (pipe & 0xff));
}

/// Handle a post-process DONE IRQ.
pub fn post_process_done() {
    clear_postprocess_status(POSTPROCESS_STATUS_DONE);
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

fn clear_decode_status(mask: u32) {
    // SAFETY: This is the AVD decode status register in the CM3-visible MMIO
    // window; writing the observed bits acknowledges the corresponding IRQ.
    unsafe {
        core::ptr::write_volatile(AVD_DECODE_STATUS as *mut u32, mask);
    }
}

fn clear_postprocess_status(mask: u32) {
    // SAFETY: This is the AVD post-process status register in the CM3-visible
    // MMIO window; writing the observed bits acknowledges the corresponding IRQ.
    unsafe {
        core::ptr::write_volatile(AVD_POSTPROCESS_STATUS as *mut u32, mask);
    }
}
