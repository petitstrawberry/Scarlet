//! x86_64 instruction utilities
//!
//! Provides safe wrappers around various x86_64 instructions

use core::arch::asm;

/// Pause instruction (for spin loops)
#[inline(always)]
pub fn pause() {
    unsafe {
        asm!("pause", options(nostack));
    }
}

/// Serialize instruction execution
#[inline(always)]
pub fn serialize() {
    unsafe {
        asm!("mfence; lfence", options(nostack));
    }
}

/// Read Time Stamp Counter
#[inline(always)]
pub fn rdtsc() -> u64 {
    let high: u32;
    let low: u32;
    unsafe {
        asm!(
            "rdtsc",
            out("eax") low,
            out("edx") high,
            options(nostack)
        );
    }
    ((high as u64) << 32) | (low as u64)
}

/// Read Time Stamp Counter with serializing
#[inline(always)]
pub fn rdtscp() -> u64 {
    let high: u32;
    let low: u32;
    unsafe {
        asm!(
            "rdtscp",
            out("eax") low,
            out("edx") high,
            options(nostack)
        );
    }
    ((high as u64) << 32) | (low as u64)
}

/// Read CR3 register (page table base)
#[inline(always)]
pub fn read_cr3() -> u64 {
    let value: u64;
    unsafe {
        asm!("mov {}, cr3", out(reg) value, options(nostack));
    }
    value
}

/// Write CR3 register (page table base)
#[inline(always)]
pub fn write_cr3(value: u64) {
    unsafe {
        asm!("mov cr3, {}", in(reg) value, options(nostack));
    }
}

/// Read CR4 register
#[inline(always)]
pub fn read_cr4() -> u64 {
    let value: u64;
    unsafe {
        asm!("mov {}, cr4", out(reg) value, options(nostack));
    }
    value
}

/// Write CR4 register
#[inline(always)]
pub fn write_cr4(value: u64) {
    unsafe {
        asm!("mov cr4, {}", in(reg) value, options(nostack));
    }
}

/// Read CR0 register
#[inline(always)]
pub fn read_cr0() -> u64 {
    let value: u64;
    unsafe {
        asm!("mov {}, cr0", out(reg) value, options(nostack));
    }
    value
}

/// Write CR0 register
#[inline(always)]
pub fn write_cr0(value: u64) {
    unsafe {
        asm!("mov cr0, {}", in(reg) value, options(nostack));
    }
}

/// Read RFLAGS register
#[inline(always)]
pub fn read_rflags() -> u64 {
    let value: u64;
    unsafe {
        asm!(
            "pushfq",
            "pop {}",
            out(reg) value,
            options(nostack)
        );
    }
    value
}

/// Write RFLAGS register
#[inline(always)]
pub fn write_rflags(value: u64) {
    unsafe {
        asm!(
            "push {}",
            "popfq",
            in(reg) value,
            options(nostack)
        );
    }
}

/// Invalidate TLB entry
#[inline(always)]
pub fn invlpg(addr: usize) {
    unsafe {
        asm!("invlpg [{}]", in(reg) addr, options(nostack));
    }
}

/// INVLPGB - Invalidate TLB entries (extended)
#[inline(always)]
pub fn invlpgb() {
    unsafe {
        asm!("invlpgb", options(nostack));
    }
}

/// TLB flush (reload CR3)
#[inline(always)]
pub fn tlb_flush() {
    let cr3 = read_cr3();
    write_cr3(cr3);
}

/// SFENCE - Store Fence
#[inline(always)]
pub fn sfence() {
    unsafe {
        asm!("sfence", options(nostack));
    }
}

/// LFENCE - Load Fence
#[inline(always)]
pub fn lfence() {
    unsafe {
        asm!("lfence", options(nostack));
    }
}

/// MFENCE - Memory Fence
#[inline(always)]
pub fn mfence() {
    unsafe {
        asm!("mfence", options(nostack));
    }
}

/// CPUID instruction
#[inline(always)]
pub fn cpuid(eax: u32, ecx: u32) -> (u32, u32, u32, u32) {
    let (eax_res, ebx_res, ecx_res, edx_res);
    unsafe {
        asm!(
            "cpuid",
            inlateout("eax") eax => eax_res,
            inlateout("ecx") ecx => ecx_res,
            out("ebx") ebx_res,
            out("edx") edx_res,
        );
    }
    (eax_res, ebx_res, ecx_res, edx_res)
}
