//! Guest MMU management for RISC-V H-extension

use core::arch::asm;

pub fn hfence_gvma_all() {
    unsafe {
        asm!("hfence.gvma zero, zero");
    }
}
