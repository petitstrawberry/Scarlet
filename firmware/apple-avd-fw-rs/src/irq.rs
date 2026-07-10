//! Cortex-M3 interrupt handling for the Apple AVD firmware.

use core::arch::asm;

use crate::abi::{MSG_PP_DONE, MSG_UNKNOWN_IRQ, MSG_VP_DONE, MSG_VP_ERROR};
use crate::mailbox::send_message;

/// Number of external NVIC lines exposed by the AVD Cortex-M3.
const NVIC_EXTERNAL_IRQS: usize = 224;

const NVIC_ISER_BASE: usize = 0xe000_e100;
const NVIC_ICER_BASE: usize = 0xe000_e180;
const NVIC_ICPR_BASE: usize = 0xe000_e280;
const NVIC_WORD_BITS: usize = 32;
const VIDEO_PIPE_IRQ_STRIDE: usize = 5;
const SYST_CSR: usize = 0xe000_e010;
const DECODE_STATUS_UNK: u32 = 1 << 0;
const DECODE_STATUS_ERR: u32 = 1 << 1;
const DECODE_STATUS_DONE: u32 = 1 << 2;
const DECODE_STATUS_CLEAR_POLLS: usize = 1_024;
#[cfg(feature = "v2-t0")]
const H264_STATUS_OFFSET: usize = 0x4060;
#[cfg(feature = "v2-t0")]
const H264_STATUS_IRQ1: u32 = 0x800;

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

#[cfg(feature = "v2-t0")]
const VIDEO_PIPE_IRQ_BASE: usize = 18;
#[cfg(feature = "v2-t0")]
const VIDEO_PIPE_COUNT: usize = 4;
#[cfg(feature = "v2-t0")]
const SUBMIT_IRQ: usize = 38;
#[cfg(feature = "v2-t0")]
const POST_PROCESS_IRQ: usize = 40;

#[cfg(not(feature = "v2-t0"))]
const VIDEO_PIPE_IRQ_BASE: usize = 78;
#[cfg(not(feature = "v2-t0"))]
const VIDEO_PIPE_COUNT: usize = 12;
#[cfg(not(feature = "v2-t0"))]
const SUBMIT_IRQ: usize = 62;
#[cfg(not(feature = "v2-t0"))]
const POST_PROCESS_IRQ: usize = 64;

/// Clear inherited NVIC state and enable only IRQs handled by this variant.
pub fn enable_known_nvic_irqs() {
    for word in 0..(NVIC_EXTERNAL_IRQS / NVIC_WORD_BITS) {
        let clear_enable = (NVIC_ICER_BASE + word * 4) as *mut u32;
        let clear_pending = (NVIC_ICPR_BASE + word * 4) as *mut u32;
        // SAFETY: NVIC ICER and ICPR are memory-mapped Cortex-M system registers.
        unsafe {
            core::ptr::write_volatile(clear_enable, u32::MAX);
            core::ptr::write_volatile(clear_pending, u32::MAX);
        }
    }

    #[cfg(feature = "v2-t0")]
    enable_nvic_irq(1);
    for pipe in 0..VIDEO_PIPE_COUNT {
        let base = VIDEO_PIPE_IRQ_BASE + pipe * VIDEO_PIPE_IRQ_STRIDE;
        enable_nvic_irq(base);
        enable_nvic_irq(base + 1);
        enable_nvic_irq(base + 2);
    }
    enable_nvic_irq(SUBMIT_IRQ);
    enable_nvic_irq(POST_PROCESS_IRQ);
}

/// Disable SysTick so inherited timer state cannot trap the firmware.
pub fn disable_systick() {
    // SAFETY: SYST_CSR is the Cortex-M SysTick control register.
    unsafe {
        core::ptr::write_volatile(SYST_CSR as *mut u32, 0);
    }
}

/// Acknowledge the t8103 H.264 accepted-status interrupt.
#[cfg(feature = "v2-t0")]
pub fn h264_status_irq1() {
    let ptr = (DECODE_CTRL_BASE + H264_STATUS_OFFSET) as *mut u32;
    // SAFETY: This is the CM3-visible t8103 packed decode status register.
    unsafe {
        core::ptr::write_volatile(ptr, H264_STATUS_IRQ1);
    }
}

/// Handle a video-pipe unknown-status IRQ.
///
/// # Arguments
///
/// * `pipe` - Hardware video pipe index.
pub fn video_pipe_unknown(pipe: u32) {
    clear_decode_status_or_fault(pipe, DECODE_STATUS_UNK);
}

/// Handle a video-pipe DONE IRQ.
///
/// # Arguments
///
/// * `pipe` - Hardware video pipe index.
pub fn video_pipe_done(pipe: u32) {
    clear_decode_status_or_fault(pipe, DECODE_STATUS_DONE);
    send_message(MSG_VP_DONE | (pipe & 0xff));
}

/// Handle a video-pipe ERROR IRQ.
///
/// # Arguments
///
/// * `pipe` - Hardware video pipe index.
pub fn video_pipe_error(pipe: u32) {
    clear_decode_status_or_fault(pipe, DECODE_STATUS_ERR);
    report_decode_fault(pipe)
}

/// Handle submit-queue unknown-status IRQ.
pub fn submit_unknown() {
    clear_decode_status_or_fault(IRQ_SUBMIT_SLOT, DECODE_STATUS_UNK);
}

/// Handle a post-process DONE IRQ.
pub fn post_process_done() {
    clear_decode_status_or_fault(IRQ_SUBMIT_SLOT, DECODE_STATUS_DONE);
    send_message(MSG_PP_DONE);
}

/// Handle an unexpected external IRQ.
///
/// # Arguments
///
/// * `irq` - IRQ number reported by the vector entry.
pub fn unknown_irq(irq: u32) -> ! {
    send_message(MSG_UNKNOWN_IRQ | irq);
    wait_forever()
}

/// Report the active Cortex-M exception using the Asahi firmware numbering.
pub fn unknown_exception() -> ! {
    let ipsr: u32;
    // SAFETY: Reading IPSR is side-effect free and identifies the active exception.
    unsafe {
        asm!("mrs {ipsr}, ipsr", ipsr = out(reg) ipsr, options(nomem, nostack, preserves_flags));
    }
    let exception = ipsr & 0x1ff;
    let irq = if exception < 16 {
        1000 + exception
    } else {
        exception - 16
    };
    unknown_irq(irq)
}

/// Report a firmware failure that occurred outside an exception handler.
pub fn fatal_exception(exception: u32) -> ! {
    unknown_irq(1000 + exception)
}

fn clear_decode_status_or_fault(slot: u32, status: u32) {
    if !clear_decode_status(slot, status) {
        report_decode_fault(slot);
    }
}

fn clear_decode_status(slot: u32, status: u32) -> bool {
    let ptr = decode_status_ptr(slot);
    let mask = decode_status_mask(slot, status);
    // SAFETY: These are AVD decode status registers in the CM3-visible MMIO
    // window; writing the observed bits acknowledges the corresponding IRQ.
    // Verify deassertion before exception return so a level IRQ cannot retrigger.
    // The bound prevents broken hardware from trapping the CM3 in MMIO polling.
    unsafe {
        core::ptr::write_volatile(ptr, mask);
        asm!("dsb sy", options(nostack, preserves_flags));
        for _ in 0..DECODE_STATUS_CLEAR_POLLS {
            if core::ptr::read_volatile(ptr) & mask == 0 {
                return true;
            }
            core::hint::spin_loop();
        }
    }
    false
}

fn report_decode_fault(slot: u32) -> ! {
    send_message(MSG_VP_ERROR | (slot & 0xff));
    wait_forever()
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

fn enable_nvic_irq(irq: usize) {
    debug_assert!(irq < NVIC_EXTERNAL_IRQS);
    let register_offset = (irq / NVIC_WORD_BITS) * core::mem::size_of::<u32>();
    let set_enable = (NVIC_ISER_BASE + register_offset) as *mut u32;
    let mask = 1u32 << (irq % NVIC_WORD_BITS);
    // SAFETY: NVIC ISER is a Cortex-M system register and `irq` is selected
    // from the firmware's statically defined vector map.
    unsafe {
        core::ptr::write_volatile(set_enable, mask);
    }
}

fn wait_forever() -> ! {
    loop {
        // SAFETY: `wfi` waits for an interrupt and has no Rust-visible side effects.
        unsafe {
            asm!("wfi", options(nomem, nostack, preserves_flags));
        }
    }
}
