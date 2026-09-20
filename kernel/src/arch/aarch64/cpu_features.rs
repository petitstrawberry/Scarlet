//! AArch64 features that are safe for every CPU allowed to run user tasks.
//!
//! APs report their ID registers before the first ELF is loaded. The published
//! HWCAP is immutable: an AP arriving later may enter the scheduler only if it
//! supports every advertised feature.

use crate::arch::cpu_features::{CpuCapabilities, CpuFeatureRegistry};
use crate::environment::MAX_NUM_CPUS;
use core::arch::asm;

const HWCAP_FP: u64 = 1 << 0;
const HWCAP_ASIMD: u64 = 1 << 1;
const HWCAP_AES: u64 = 1 << 3;
const HWCAP_PMULL: u64 = 1 << 4;
const HWCAP_SHA1: u64 = 1 << 5;
const HWCAP_SHA2: u64 = 1 << 6;
const HWCAP_CRC32: u64 = 1 << 7;
const HWCAP_ATOMICS: u64 = 1 << 8;
const HWCAP_FPHP: u64 = 1 << 9;
const HWCAP_ASIMDHP: u64 = 1 << 10;
const HWCAP_SHA512: u64 = 1 << 21;

static REGISTRY: CpuFeatureRegistry = CpuFeatureRegistry::new();

const fn field(value: u64, shift: u32) -> u64 {
    (value >> shift) & 0xf
}

/// Convert architected ID fields into the supported Linux AArch64 HWCAP ABI.
/// Reserved field values are never interpreted as future feature support.
fn hwcap_from_id(isar0: u64, pfr0: u64, user_fpu: bool) -> u64 {
    let mut cap = 0;
    // FP/SIMD state is only promised when Scarlet saves and restores it.
    let fp_field = field(pfr0, 16);
    let asimd_field = field(pfr0, 20);
    let fp = user_fpu && fp_field <= 1;
    let asimd = fp && asimd_field <= 1;
    if fp {
        cap |= HWCAP_FP;
        if fp_field == 1 {
            cap |= HWCAP_FPHP;
        }
    }
    if asimd {
        cap |= HWCAP_ASIMD;
        if asimd_field == 1 {
            cap |= HWCAP_ASIMDHP;
        }
        match field(isar0, 4) {
            1 => cap |= HWCAP_AES,
            2 => cap |= HWCAP_AES | HWCAP_PMULL,
            _ => {}
        }
        if field(isar0, 8) == 1 {
            cap |= HWCAP_SHA1;
        }
        match field(isar0, 12) {
            1 => cap |= HWCAP_SHA2,
            2 => cap |= HWCAP_SHA2 | HWCAP_SHA512,
            _ => {}
        }
    }
    if field(isar0, 16) == 1 {
        cap |= HWCAP_CRC32;
    }
    // Atomic == 2 denotes FEAT_LSE; 3 adds FEAT_LSE128 and retains LSE.
    // Higher encodings are reserved here, so do not guess their semantics.
    if matches!(field(isar0, 20), 2 | 3) {
        cap |= HWCAP_ATOMICS;
    }
    cap
}

fn local_hwcap() -> u64 {
    let isar0: u64;
    let pfr0: u64;
    // SAFETY: EL1 may read the architected ID registers on this CPU.
    unsafe {
        asm!("mrs {value}, id_aa64isar0_el1", value = out(reg) isar0, options(nomem, nostack));
        asm!("mrs {value}, id_aa64pfr0_el1", value = out(reg) pfr0, options(nomem, nostack));
    }
    hwcap_from_id(isar0, pfr0, crate::arch::user_fpu_enabled())
}

fn local_capabilities() -> CpuCapabilities {
    CpuCapabilities {
        hwcap: local_hwcap(),
        hwcap2: 0,
    }
}

pub(crate) fn register_boot_cpu(cpu_id: usize) {
    REGISTRY.report(cpu_id, local_capabilities());
}

/// An AP must be probed before it can pass the ordinary scheduler release gate.
pub(crate) fn register_secondary_cpu(cpu_id: usize) {
    if !REGISTRY.report_secondary_and_wait(cpu_id, local_capabilities()) {
        crate::println!(
            "[aarch64] CPU {} lacks published userspace features; parked",
            cpu_id
        );
        loop {
            crate::arch::instruction::idle();
        }
    }
}

pub(crate) fn probed_cpu_mask() -> u64 {
    REGISTRY.reported_mask()
}

/// Publish once, before any initial or subsequent exec can construct auxv.
pub(crate) fn publish(cpu_count: usize) {
    let expected = (1u64 << cpu_count.min(MAX_NUM_CPUS)) - 1;
    let frequency: u64;
    let start: u64;
    // SAFETY: The architected counter is initialized before user task loading.
    unsafe {
        asm!("mrs {value}, cntfrq_el0", value = out(reg) frequency, options(nomem, nostack));
        asm!("mrs {value}, cntvct_el0", value = out(reg) start, options(nomem, nostack));
    }
    while probed_cpu_mask() & expected != expected && frequency != 0 {
        let now: u64;
        // SAFETY: Reading the virtual counter has no side effects.
        unsafe { asm!("mrs {value}, cntvct_el0", value = out(reg) now, options(nomem, nostack)) };
        if now.wrapping_sub(start) >= frequency {
            break;
        }
        core::hint::spin_loop();
    }
    let mask = probed_cpu_mask();
    let common = REGISTRY.publish();
    crate::println!(
        "[aarch64] user HWCAP={:#x}, probed CPUs={:#x}",
        common.hwcap,
        mask
    );
}

pub(crate) fn userspace_capabilities() -> CpuCapabilities {
    REGISTRY.published_capabilities()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn lse_and_reserved_values() {
        assert_eq!(hwcap_from_id(2 << 20, 0, false), HWCAP_ATOMICS);
        assert_eq!(hwcap_from_id(3 << 20, 0, false), HWCAP_ATOMICS);
        for atomic in [0, 1, 4, 15] {
            assert_eq!(hwcap_from_id(atomic << 20, 0, false) & HWCAP_ATOMICS, 0);
        }
    }

    #[test_case]
    fn simd_features_require_saved_context() {
        let crypto = (2 << 4) | (1 << 8) | (2 << 12) | (1 << 16);
        assert_eq!(hwcap_from_id(crypto, 0, false), HWCAP_CRC32);
        assert_eq!(
            hwcap_from_id(crypto, 0, true),
            HWCAP_FP
                | HWCAP_ASIMD
                | HWCAP_AES
                | HWCAP_PMULL
                | HWCAP_SHA1
                | HWCAP_SHA2
                | HWCAP_SHA512
                | HWCAP_CRC32
        );
        assert_eq!(
            hwcap_from_id(crypto, 15 << 20, true),
            HWCAP_FP | HWCAP_CRC32
        );
        assert_eq!(
            hwcap_from_id(0, (1 << 16) | (1 << 20), true),
            HWCAP_FP | HWCAP_ASIMD | HWCAP_FPHP | HWCAP_ASIMDHP
        );
    }
}
