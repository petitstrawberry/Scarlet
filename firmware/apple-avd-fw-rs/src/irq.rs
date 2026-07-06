//! Cortex-M3 interrupt handling for the Apple AVD firmware.

use crate::abi::{
    CMD_H264_DECODE, MSG_PP_DONE, MSG_UNKNOWN_IRQ, MSG_VP_DONE, MSG_VP_ERROR, command_kind,
    command_tag,
};
use crate::mailbox::send_message;

/// Number of external NVIC lines enabled by the AVD firmware.
pub const NVIC_EXTERNAL_IRQS: usize = 224;

const NVIC_ISER_BASE: usize = 0xe000_e100;
const DECODE_STATUS_UNK: u32 = 1 << 0;
const DECODE_STATUS_ERR: u32 = 1 << 1;
const DECODE_STATUS_DONE: u32 = 1 << 2;
const H264_STATUS_OFFSET: usize = 0x4060;
const H264_STATUS_IRQ1: u32 = 0x800;
const DECODE_STATUS_CLEAR_POLLS: usize = 100_000;

#[cfg(feature = "v2-t0")]
const DECODE_CTRL_BASE: usize = 0x4010_0000;
#[cfg(any(feature = "v3-t0", feature = "v3-t1"))]
const DECODE_CTRL_BASE: usize = 0x4010_0000;
#[cfg(any(feature = "v4-t0", feature = "v5-t0", feature = "v5-t1"))]
const DECODE_CTRL_BASE: usize = 0x4110_0000;

#[cfg(feature = "v2-t0")]
const VP_OFFSET: usize = 0x4060;
#[cfg(any(feature = "v3-t0", feature = "v3-t1"))]
const VP_OFFSET: usize = 0x124;
#[cfg(any(feature = "v4-t0", feature = "v5-t0", feature = "v5-t1"))]
const VP_OFFSET: usize = 0x194;

#[cfg(feature = "v2-t0")]
const IRQ_SUBMIT_SLOT: u32 = 4;
#[cfg(any(feature = "v3-t0", feature = "v3-t1"))]
const IRQ_SUBMIT_SLOT: u32 = 9;
#[cfg(any(feature = "v4-t0", feature = "v5-t0", feature = "v5-t1"))]
const IRQ_SUBMIT_SLOT: u32 = 13;

#[cfg(feature = "v2-t0")]
const PACKED_STATUS: bool = true;
#[cfg(not(feature = "v2-t0"))]
const PACKED_STATUS: bool = false;

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

/// Dispatch one AP-to-CM3 mailbox command.
///
/// # Arguments
///
/// * `command` - Raw AP-to-CM3 command word.
pub fn dispatch_mailbox_command(command: u32) {
    match command_kind(command) {
        CMD_H264_DECODE => arm_decode_irqs(),
        _ => {
            let kind = command_kind(command) & 0xff;
            let tag = command_tag(command) & 0xff;
            send_message(MSG_UNKNOWN_IRQ | (kind << 8) | tag);
        }
    }
}

/// Handle the early H.264 status IRQ observed when a frame is accepted.
pub fn h264_status_irq1() {
    clear_h264_status(H264_STATUS_IRQ1);
}

/// Handle a video-pipe unknown-status IRQ.
///
/// # Arguments
///
/// * `pipe` - Hardware video pipe index.
pub fn video_pipe_unknown(pipe: u32) {
    clear_decode_status(pipe, DECODE_STATUS_UNK);
}

/// Handle a video-pipe DONE IRQ.
///
/// # Arguments
///
/// * `pipe` - Hardware video pipe index.
pub fn video_pipe_done(pipe: u32) {
    if !clear_decode_status(pipe, DECODE_STATUS_DONE) {
        send_message(MSG_VP_ERROR | (pipe & 0xff));
        return;
    }
    send_message(MSG_VP_DONE | (pipe & 0xff));
}

/// Handle a video-pipe ERROR IRQ.
///
/// # Arguments
///
/// * `pipe` - Hardware video pipe index.
pub fn video_pipe_error(pipe: u32) {
    let _ = clear_decode_status(pipe, DECODE_STATUS_ERR);
    send_message(MSG_VP_ERROR | (pipe & 0xff));
}

/// Handle submit-queue unknown-status IRQ.
pub fn submit_unknown() {
    clear_decode_status(IRQ_SUBMIT_SLOT, DECODE_STATUS_UNK);
}

/// Handle a post-process DONE IRQ.
pub fn post_process_done() {
    if !clear_decode_status(IRQ_SUBMIT_SLOT, DECODE_STATUS_DONE) {
        send_message(MSG_VP_ERROR | (IRQ_SUBMIT_SLOT & 0xff));
        return;
    }
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

fn clear_decode_status(slot: u32, status: u32) -> bool {
    let ptr = decode_status_ptr(slot);
    let mask = decode_status_mask(slot, status);
    // SAFETY: These are AVD decode status registers in the CM3-visible MMIO
    // window; writing the observed bits acknowledges the corresponding IRQ.
    unsafe {
        core::ptr::write_volatile(ptr, mask);
        for _ in 0..DECODE_STATUS_CLEAR_POLLS {
            if core::ptr::read_volatile(ptr) & mask == 0 {
                return true;
            }
            core::hint::spin_loop();
        }
    }
    false
}

fn clear_h264_status(status: u32) {
    let ptr = (DECODE_CTRL_BASE + H264_STATUS_OFFSET) as *mut u32;
    // SAFETY: This is the CM3-visible alias of the H.264 status register.
    // Writing the observed bit acknowledges the corresponding IRQ.
    unsafe {
        core::ptr::write_volatile(ptr, status);
    }
}

fn decode_status_ptr(slot: u32) -> *mut u32 {
    let offset = if PACKED_STATUS {
        VP_OFFSET
    } else {
        VP_OFFSET + slot as usize * 4
    };
    (DECODE_CTRL_BASE + offset) as *mut u32
}

fn decode_status_mask(slot: u32, status: u32) -> u32 {
    if PACKED_STATUS {
        status << (slot * 5)
    } else {
        status
    }
}
