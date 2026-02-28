//! x86_64 timer support using Local APIC timer
//!
//! The Local APIC timer is a per-CPU timer that can be configured
//! for one-shot or periodic operation.

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::mmio;

/// Local APIC MMIO base (typically configured by firmware)
const LAPIC_BASE: usize = 0xFEE0_0000;

// Local APIC register offsets
const LAPIC_ID: usize = 0x020;
const LAPIC_VER: usize = 0x030;
const LAPIC_TPR: usize = 0x080;
const LAPIC_EOI: usize = 0x0B0;
const LAPIC_LDR: usize = 0x0D0;
const LAPIC_DFR: usize = 0x0E0;
const LAPIC_SVR: usize = 0x0F0;
const LAPIC_ISR: usize = 0x100;
const LAPIC_TMR: usize = 0x180;
const LAPIC_IRR: usize = 0x200;
const LAPIC_ESR: usize = 0x280;
const LAPIC_ICR: usize = 0x300;
const LAPIC_TIMER_LVT: usize = 0x320;
const LAPIC_TIMER_INIT: usize = 0x380;
const LAPIC_TIMER_CUR: usize = 0x390;
const LAPIC_TIMER_DIV: usize = 0x3E0;

/// Timer divisor values
#[allow(dead_code)]
#[repr(u32)]
enum TimerDivisor {
    Div1 = 0xB,
    Div2 = 0x0,
    Div4 = 0x1,
    Div8 = 0x2,
    Div16 = 0x3,
    Div32 = 0x8,
    Div64 = 0x9,
    Div128 = 0xA,
}

/// Global tick counter
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Read Local APIC register
#[inline(always)]
fn lapic_read(offset: usize) -> u32 {
    mmio::read_u32(LAPIC_BASE + offset)
}

/// Write Local APIC register
#[inline(always)]
fn lapic_write(offset: usize, value: u32) {
    mmio::write_u32(LAPIC_BASE + offset, value);
}

/// Initialize the Local APIC timer
pub fn init() {
    // Mask the timer initially
    let mut lvt = lapic_read(LAPIC_TIMER_LVT);
    lvt |= 0x10000; // Mask bit
    lapic_write(LAPIC_TIMER_LVT, lvt);

    // Set divisor to 1
    lapic_write(LAPIC_TIMER_DIV, TimerDivisor::Div1 as u32);

    // Set up one-shot mode with vector 32
    lvt = lapic_read(LAPIC_TIMER_LVT);
    lvt = (lvt & 0x00FF_FFFF) | 32; // Timer interrupt vector
    lapic_write(LAPIC_TIMER_LVT, lvt);
}

/// Start the timer with a given interval
///
/// The interval is in timer units (depends on bus frequency).
/// For practical use, you need to calibrate the timer first.
pub fn start(interval_us: u64) {
    // This is a simplified implementation
    // In a real OS, you'd calibrate the timer against a known time source

    // Assume bus frequency is roughly 1 GHz for this example
    // interval_us * 1000 = timer counts
    let counts = interval_us.saturating_mul(1000);

    // Unmask and set initial count
    let mut lvt = lapic_read(LAPIC_TIMER_LVT);
    lvt &= !0x10000; // Clear mask bit
    lapic_write(LAPIC_TIMER_LVT, lvt);
    lapic_write(LAPIC_TIMER_INIT, counts as u32);
}

/// Stop the timer
pub fn stop() {
    let mut lvt = lapic_read(LAPIC_TIMER_LVT);
    lvt |= 0x10000; // Set mask bit
    lapic_write(LAPIC_TIMER_LVT, lvt);
}

/// Get the current tick count
pub fn get_ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Acknowledge timer interrupt
pub fn ack() {
    lapic_write(LAPIC_EOI, 0);
}

/// Handle timer interrupt
pub fn handle_interrupt() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

/// Read current timer count
pub fn read_current_count() -> u32 {
    lapic_read(LAPIC_TIMER_CUR)
}

/// Architecture-specific timer interface
pub struct ArchTimer;

impl ArchTimer {
    /// Initialize the timer for the given CPU
    pub fn init(&self, cpu_id: usize) {
        let _ = cpu_id;
        init();
    }

    /// Get the current tick count
    pub fn get_ticks(&self) -> u64 {
        get_ticks()
    }

    /// Start periodic timer with interval in microseconds
    pub fn start_periodic(&self, interval_us: u64) {
        start(interval_us);
    }

    /// Stop the timer
    pub fn stop(&self) {
        stop();
    }

    /// Acknowledge timer interrupt
    pub fn ack(&self) {
        ack();
    }
}
