//! AArch64 instruction handling
//!
//! Instruction parsing and handling for AArch64 architecture.

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
    // Wait For Interrupt in an infinite loop.
    // WFI may return spuriously (e.g., on timer interrupts), so we loop forever.
    // The scheduler expects idle() to never return - task switching happens
    // via timer interrupts that call schedule() and context switch away.
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack, preserves_flags));
        }
    }
}
