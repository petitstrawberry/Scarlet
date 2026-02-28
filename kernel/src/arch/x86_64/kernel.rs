//! x86_64 kernel entry point
//!
//! Provides the main kernel entry function for x86_64

use crate::arch::x86_64::earlycon::early_println;

/// Kernel main entry point for x86_64
///
/// This is called after basic boot initialization.
/// It sets up the kernel and starts the scheduler.
///
/// # Safety
/// This should only be called once during boot.
pub fn kernel_main() -> ! {
    early_println!("[x86_64] Kernel main starting...");

    // Initialize kernel subsystems
    // (These would be initialized in the common kernel code)

    // Start the scheduler
    // (This would be done in the common kernel code)

    early_println!("[x86_64] Kernel main: nothing to do, halting...");
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}
