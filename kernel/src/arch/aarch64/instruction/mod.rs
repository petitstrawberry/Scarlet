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
    loop {
        crate::arch::interrupt::enable_interrupts();
        // SAFETY: These privileged instructions do not access Rust-managed
        // memory. The DSB completes prior memory accesses before entering WFI.
        unsafe {
            core::arch::asm!("dsb sy", "wfi", options(nostack, preserves_flags));
        }
    }
}
