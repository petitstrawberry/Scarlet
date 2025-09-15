//! AArch64 early console implementation
//!
//! Provides early console output functionality for AArch64 architecture.
//! Uses direct UART register access for early boot output before proper
//! driver initialization.

use core::ptr::{read_volatile, write_volatile};

// QEMU virt machine PL011 UART base address
// This is the standard address for UART0 on QEMU's virt machine
const UART_BASE: usize = 0x0900_0000;

// PL011 UART register offsets
const UART_DR: usize = 0x000;   // Data Register
const UART_FR: usize = 0x018;   // Flag Register

// Flag Register bits
const UART_FR_TXFF: u32 = 1 << 5;  // Transmit FIFO Full

/// Early console putchar function for AArch64
/// 
/// This function provides character output during early boot before
/// the full UART driver is initialized. It directly accesses PL011
/// UART registers on QEMU virt machine.
/// 
/// # Arguments
/// * `c` - Character to output
/// 
/// # Safety
/// This function performs raw memory access to UART registers.
/// It should only be used during early boot when no other console
/// driver is available.
pub fn early_putc(c: u8) {
    unsafe {
        // Wait until transmit FIFO is not full
        while (read_volatile((UART_BASE + UART_FR) as *const u32) & UART_FR_TXFF) != 0 {
            core::hint::spin_loop();
        }
        
        // Write character to data register
        write_volatile((UART_BASE + UART_DR) as *mut u32, c as u32);
    }
}

/// Initialize early console (currently a no-op)
/// 
/// On QEMU virt machine, the PL011 UART is typically pre-configured
/// by the firmware/bootloader, so no additional initialization is
/// usually required for basic character output.
pub fn early_console_init() {
    // QEMU's PL011 UART is usually pre-configured by firmware
    // No additional initialization required for early output
}

/// Write a string to early console
/// 
/// # Arguments
/// * `s` - String to write
pub fn early_console_write(s: &str) {
    for byte in s.bytes() {
        early_putc(byte);
    }
}