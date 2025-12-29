//! AArch64 instruction handling
//!
//! Instruction parsing and handling for AArch64 architecture.

use core::sync::atomic::{AtomicUsize, Ordering};

// TODO: Implement AArch64 instruction handling
// This includes instruction fetching, decoding, etc.

pub struct Instruction {
    // TODO: Define AArch64 instruction structure
    pub raw: u32,
}

impl Instruction {
    pub fn fetch(_addr: usize) -> Self {
        // TODO: Fetch instruction from memory
        Instruction { raw: 0 }
    }

    pub fn len(&self) -> usize {
        // AArch64 instructions are 4 bytes in AArch64 state
        4
    }
}

pub fn idle() -> ! {
    crate::early_println!("[idle] ENTERED idle function");
    
    // TEMPORARY DEBUGGING: Don't use WFI, just busy loop and let interrupts fire
    // This helps determine if the issue is with WFI not waking, or interrupts not firing at all
    crate::early_println!("[idle] Using busy loop instead of WFI for debugging");

    // Keep these logs bounded so they don't drown out fault/scheduler output.
    static IDLE_LOG_BUDGET: AtomicUsize = AtomicUsize::new(16);
    
    let mut count = 0;
    loop {
        count += 1;
        if count % 1000000000 == 0 {
            let mut ctl: u64;
            unsafe {
                core::arch::asm!("mrs {0}, CNTP_CTL_EL0", out(reg) ctl);
            }
            if IDLE_LOG_BUDGET
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                    if v == 0 { None } else { Some(v - 1) }
                })
                .is_ok()
            {
                crate::early_println!(
                    "[idle] Busy loop iteration {}, CNTP_CTL={:#x}",
                    count / 10000000,
                    ctl
                );
            }
        }
        // Give interrupts a chance to fire
        unsafe {
            core::arch::asm!("nop", options(nomem, nostack, preserves_flags));
        }
    }
}
