//! RISC-V SMP (Symmetric Multi-Processing) support
//!
//! This module handles starting and initializing secondary CPUs (harts)
//! using the SBI HSM (Hart State Management) extension.

use crate::arch::riscv64::instruction::sbi::{
    sbi_hsm_hart_get_status, sbi_hsm_hart_start, HartState,
};
use crate::early_println;
use core::sync::atomic::{AtomicUsize, Ordering};

/// CPU start barrier to ensure all CPUs are ready
static CPU_START_BARRIER: AtomicUsize = AtomicUsize::new(0);

/// Number of CPUs that have been started
static CPUS_STARTED: AtomicUsize = AtomicUsize::new(0);

/// The boot hart ID (set during initialization)
static BOOT_HART_ID: AtomicUsize = AtomicUsize::new(0);

/// Set the boot hart ID
pub fn set_boot_hart_id(hart_id: usize) {
    BOOT_HART_ID.store(hart_id, Ordering::SeqCst);
}

/// Get the boot hart ID
pub fn get_boot_hart_id() -> usize {
    BOOT_HART_ID.load(Ordering::SeqCst)
}

/// External symbol for the AP entry point
extern "C" {
    fn _entry_ap();
}

/// Start secondary CPUs (harts) using SBI HSM extension
///
/// This function starts all harts detected from the device tree except
/// the boot hart (the one that called this function).
///
/// # Arguments
/// * `boot_hart_id` - The hart ID of the boot processor
/// * `max_hart_id` - The maximum hart ID to attempt starting
pub fn start_secondary_cpus(boot_hart_id: usize, max_hart_id: usize) {
    early_println!(
        "[SMP] Starting secondary CPUs: boot_hart={}, max_hart_id={}",
        boot_hart_id,
        max_hart_id
    );

    let ap_start_addr = _entry_ap as usize;

    for hart_id in 0..=max_hart_id {
        // Skip the boot hart
        if hart_id == boot_hart_id {
            early_println!("[SMP] Skipping boot hart {}", hart_id);
            continue;
        }

        // Check if this hart exists by querying its status
        let state = match sbi_hsm_hart_get_status(hart_id) {
            Ok(s) => s,
            Err(_) => {
                // Hart doesn't exist or HSM not supported
                early_println!("[SMP] Hart {} does not exist, skipping", hart_id);
                continue;
            }
        };

        early_println!(
            "[SMP] Hart {} initial state: {:?}",
            hart_id,
            state
        );

        match state {
            HartState::Stopped => {
                // Hart is available and stopped, start it
                early_println!(
                    "[SMP] Starting hart {} at entry point {:#x}",
                    hart_id,
                    ap_start_addr
                );

                match sbi_hsm_hart_start(hart_id, ap_start_addr, hart_id) {
                    Ok(()) => {
                        early_println!("[SMP] Successfully started hart {}", hart_id);
                        // CPU start count is updated in `mark_cpu_started` when the hart finishes initialization
                    }
                    Err(e) => {
                        early_println!(
                            "[SMP] Failed to start hart {}: {:?}",
                            hart_id,
                            e
                        );
                    }
                }
            }
            HartState::Started => {
                // Hart is already running (shouldn't happen in normal boot)
                early_println!(
                    "[SMP] Hart {} is already started, skipping",
                    hart_id
                );
            }
            _ => {
                early_println!(
                    "[SMP] Hart {} is in unexpected state {:?}, skipping",
                    hart_id,
                    state
                );
            }
        }
    }

    let started = CPUS_STARTED.load(Ordering::SeqCst);
    early_println!(
        "[SMP] Secondary CPU startup complete: {} CPUs started",
        started
    );
}

/// Get the number of CPUs that have been started
pub fn get_cpus_started() -> usize {
    CPUS_STARTED.load(Ordering::SeqCst)
}

/// Mark a CPU as started (called by start_ap)
pub fn mark_cpu_started() {
    CPUS_STARTED.fetch_add(1, Ordering::SeqCst);
}

/// Wait for all CPUs to be ready
pub fn wait_for_cpus() {
    let expected = crate::environment::NUM_OF_CPUS - 1; // Exclude boot CPU
    let mut started = CPUS_STARTED.load(Ordering::SeqCst);
    let mut retries = 0;

    while started < expected && retries < 1000 {
        core::hint::spin_loop();
        started = CPUS_STARTED.load(Ordering::SeqCst);
        retries += 1;
    }

    if started < expected {
        early_println!(
            "[SMP] Warning: Only {} of {} expected secondary CPUs started",
            started,
            expected
        );
    } else {
        early_println!(
            "[SMP] All {} CPUs ready",
            crate::environment::NUM_OF_CPUS
        );
    }
}
