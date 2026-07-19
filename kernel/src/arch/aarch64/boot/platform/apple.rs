//! Apple Silicon early CPU initialization for AArch64 boot.

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

use fdt::node::FdtNode;

use crate::sched::scheduler::CpuCoreClass;

const MIDR_IMPLEMENTER_SHIFT: u32 = 24;
const APPLE_MIDR_IMPLEMENTER: u64 = 0x61;
const ACTLR_EN_MDSB: u64 = 1 << 12;
const VM_TIMER_FIQ_ENABLE_VIRTUAL: u64 = 1;
const VM_TIMER_FIQ_ENABLE_PHYSICAL: u64 = 1 << 1;
const UNAVAILABLE_EL2_DROP_DIAGNOSTIC: u64 = u64::MAX;

static HACR_EL2_BEFORE_DROP: AtomicU64 = AtomicU64::new(UNAVAILABLE_EL2_DROP_DIAGNOSTIC);
static HACR_EL2_AFTER_CLEAR: AtomicU64 = AtomicU64::new(UNAVAILABLE_EL2_DROP_DIAGNOSTIC);

pub(super) fn prepare_el2_drop() {
    if !is_apple_cpu(read_midr()) {
        return;
    }

    clear_el2_sysreg_traps();
    normalize_timer_fiq_gate();
}

pub(super) fn el2_drop_diagnostic_transition() -> Option<(u64, u64)> {
    let after = HACR_EL2_AFTER_CLEAR.load(Ordering::Relaxed);
    (after != UNAVAILABLE_EL2_DROP_DIAGNOSTIC)
        .then(|| (HACR_EL2_BEFORE_DROP.load(Ordering::Relaxed), after))
}

pub(super) fn initialize_current_cpu() -> Option<(u64, u64)> {
    if !is_apple_cpu(read_midr()) {
        return None;
    }

    let old_actlr = read_effective_actlr();
    let new_actlr = old_actlr | ACTLR_EN_MDSB;
    // SAFETY: The MIDR check above restricts the implementation-defined ACTLR
    // field to Apple CPUs. Each CPU updates its own effective ACTLR bank
    // during one-time early boot, before entering the scheduler.
    unsafe {
        asm!(
            "msr actlr_el1, {actlr}",
            "isb",
            actlr = in(reg) new_actlr,
            options(nostack)
        );
    }
    Some((old_actlr, read_effective_actlr()))
}

pub(super) fn classify_cpu_node(cpu: &FdtNode) -> Option<CpuCoreClass> {
    if compatible_contains_any(cpu, &[b"icestorm", b"blizzard", b"efficiency", b"e-core"]) {
        return Some(CpuCoreClass::Efficiency);
    }
    if compatible_contains_any(
        cpu,
        &[
            b"firestorm",
            b"avalanche",
            b"everest",
            b"performance",
            b"p-core",
        ],
    ) {
        return Some(CpuCoreClass::Performance);
    }

    None
}

#[inline(always)]
fn read_midr() -> u64 {
    let midr: u64;
    // SAFETY: MIDR_EL1 is architecturally readable in the EL1 and EL2 host
    // regimes used during early boot and has no memory side effects.
    unsafe {
        asm!("mrs {midr}, midr_el1", midr = out(reg) midr, options(nostack));
    }
    midr
}

#[inline(always)]
fn is_apple_cpu(midr: u64) -> bool {
    ((midr >> MIDR_IMPLEMENTER_SHIFT) & 0xff) == APPLE_MIDR_IMPLEMENTER
}

fn clear_el2_sysreg_traps() {
    // m1n1's hypervisor uses HACR_EL2 to trap and emulate Apple CPU-local
    // registers including fast IPI and PMU state. A direct EL2-to-EL1 handoff
    // has no EL2 trap handler, so inherited trap bits would make those EL1
    // accesses disappear into the previous firmware's EL2 vector.
    let before: u64;
    let after: u64;
    // SAFETY: The common boot path calls this hook only after confirming an
    // EL2 non-VHE drop. The MIDR gate in `prepare_el2_drop` limits this
    // implementation-defined register to Apple CPUs, while exceptions remain
    // masked until the handoff completes.
    unsafe {
        asm!(
            ".arch armv8.1-a",
            "mrs {before}, hacr_el2",
            "msr hacr_el2, xzr",
            "isb",
            "mrs {after}, hacr_el2",
            before = out(reg) before,
            after = out(reg) after,
            options(nomem, nostack, preserves_flags)
        );
    }
    HACR_EL2_BEFORE_DROP.store(before, Ordering::Relaxed);
    HACR_EL2_AFTER_CLEAR.store(after, Ordering::Relaxed);
}

fn normalize_timer_fiq_gate() {
    // SAFETY: The common boot path calls this hook only for a direct EL2-to-EL1
    // handoff, and `prepare_el2_drop` has verified Apple's MIDR implementer.
    // The implementation-defined gate is CPU-local and is normalized while
    // exceptions are masked before the generic EL2 transition.
    unsafe {
        asm!(
            ".arch armv8.1-a",
            "mrs {timer_gate}, S3_5_C15_C1_3",
            "bic {timer_gate}, {timer_gate}, {physical_timer_gate}",
            "orr {timer_gate}, {timer_gate}, {virtual_timer_gate}",
            "msr S3_5_C15_C1_3, {timer_gate}",
            "dsb sy",
            "isb",
            timer_gate = out(reg) _,
            physical_timer_gate = const VM_TIMER_FIQ_ENABLE_PHYSICAL,
            virtual_timer_gate = const VM_TIMER_FIQ_ENABLE_VIRTUAL,
            options(nostack)
        );
    }
}

#[inline(always)]
fn read_effective_actlr() -> u64 {
    let actlr: u64;
    // SAFETY: Reading ACTLR_EL1 has no memory side effects. The register name
    // addresses the control register effective for the current EL1 or EL2/VHE
    // host regime during early boot.
    unsafe {
        asm!("mrs {actlr}, actlr_el1", actlr = out(reg) actlr, options(nostack));
    }
    actlr
}

fn compatible_contains_any(cpu: &FdtNode, needles: &[&[u8]]) -> bool {
    let Some(prop) = cpu.property("compatible") else {
        return false;
    };

    needles
        .iter()
        .any(|needle| bytes_contains_ascii_case_insensitive(prop.value, needle))
}

fn bytes_contains_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }

    haystack.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle.iter())
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn detects_apple_midr_implementer() {
        assert!(is_apple_cpu(
            APPLE_MIDR_IMPLEMENTER << MIDR_IMPLEMENTER_SHIFT
        ));
        assert!(!is_apple_cpu(0x41 << MIDR_IMPLEMENTER_SHIFT));
    }

    #[test_case]
    fn el2_drop_normalizes_the_private_timer_gate() {
        let source = include_str!("apple.rs");
        let physical_timer_mask = source
            .find("bic {timer_gate}, {timer_gate}, {physical_timer_gate}")
            .expect("EL2 preparation must disable the physical timer FIQ gate");
        let virtual_timer_enable = source
            .find("orr {timer_gate}, {timer_gate}, {virtual_timer_gate}")
            .expect("EL2 preparation must enable the virtual timer FIQ gate");
        let timer_gate_write = source
            .find("msr S3_5_C15_C1_3, {timer_gate}")
            .expect("EL2 preparation must write the private timer gate");

        assert!(physical_timer_mask < timer_gate_write);
        assert!(virtual_timer_enable < timer_gate_write);
    }
}
