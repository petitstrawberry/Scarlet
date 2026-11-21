//! AArch64 CASP operates on both little-endian register halves in one transaction.
use super::Trapframe;
use core::sync::atomic::{AtomicU64, AtomicU128, Ordering::SeqCst};

pub(super) fn apply(tf: &mut Trapframe, expected: usize, value: usize, ptr: usize, width: usize) {
    let bits = width * 4;
    let mask = (1u128 << bits) - 1;
    let pair = |reg: usize| {
        let low = tf.regs.reg[reg] as u128 & mask;
        let high = tf.regs.reg.get(reg + 1).copied().unwrap_or(0) as u128 & mask;
        low | (high << bits)
    };
    let (expected_value, replacement) = (pair(expected), pair(value));
    // The enclosing VM transaction checked natural alignment, both permissions,
    // and a contiguous RAM alias. Peer workers can operate on this same word.
    let old = if width == 8 {
        let dst = unsafe { AtomicU64::from_ptr(ptr as *mut u64) };
        dst.compare_exchange(expected_value as u64, replacement as u64, SeqCst, SeqCst)
            .unwrap_or_else(|observed| observed) as u128
    } else {
        let dst = unsafe { AtomicU128::from_ptr(ptr as *mut u128) };
        dst.compare_exchange(expected_value, replacement, SeqCst, SeqCst)
            .unwrap_or_else(|observed| observed)
    };
    tf.regs.reg[expected] = (old & mask) as usize;
    if let Some(high) = tf.regs.reg.get_mut(expected + 1) {
        *high = (old >> bits) as usize;
    }
}
