//! AArch64 MMU implementation
//!
//! This module provides the Memory Management Unit (MMU) implementation for ARMv8.0-A
//! architecture with 4-level page tables supporting 48-bit virtual addresses.
//!
//! # Page Table Structure
//!
//! The implementation uses the 4KB granule with 4-level translation:
//! - Level 0: Page Global Directory (PGD) - bits 47:39 (9 bits, 512 entries)
//! - Level 1: Page Upper Directory (PUD) - bits 38:30 (9 bits, 512 entries)  
//! - Level 2: Page Middle Directory (PMD) - bits 29:21 (9 bits, 512 entries)
//! - Level 3: Page Table Entry (PTE) - bits 20:12 (9 bits, 512 entries)
//! - Page offset: bits 11:0 (12 bits, 4KB pages)
//!
//! # Features
//!
//! - 48-bit virtual address space (256TB)
//! - 4KB page granule
//! - Support for both secure and non-secure memory
//! - Memory attribute configuration via MAIR registers
//! - Translation table base registers (TTBR0/TTBR1) management
//! - TLB invalidation and memory barriers

pub mod armv8_4k;
pub use armv8_4k::*;