//! x86_64 interrupt handling support
//!
//! Provides interrupt management functions for x86_64,
//! including Local APIC and 8259 PIC handling.

use core::arch::asm;

use crate::arch::x86_64::mmio;

/// Local APIC MMIO base
const LAPIC_BASE: usize = 0xFEE0_0000;

// Local APIC register offsets
const LAPIC_SVR: usize = 0x0F0;
const LAPIC_EOI: usize = 0x0B0;
const LAPIC_ICR1: usize = 0x300;
const LAPIC_ICR2: usize = 0x310;

/// 8259 PIC ports
const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_CMD: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

/// Initialize the 8259 PICs and disable them
/// (in favor of Local APIC in modern systems)
pub fn init_pic() {
    unsafe {
        // ICW1: Initialize, ICW4 needed
        asm!("out dx, al", in("dx") PIC1_CMD, in("al") 0x11u8);
        asm!("out dx, al", in("dx") PIC2_CMD, in("al") 0x11u8);

        // ICW2: Vector offset (unused since we're disabling)
        asm!("out dx, al", in("dx") PIC1_DATA, in("al") 0x08u8); // IRQ 0-7 -> 0x08-0x0F
        asm!("out dx, al", in("dx") PIC2_DATA, in("al") 0x70u8); // IRQ 8-15 -> 0x70-0x77

        // ICW3: Cascade wiring
        asm!("out dx, al", in("dx") PIC1_DATA, in("al") 0x04u8); // PIC2 at IRQ2
        asm!("out dx, al", in("dx") PIC2_DATA, in("al") 0x02u8); // Cascade identity

        // ICW4: 8086 mode
        asm!("out dx, al", in("dx") PIC1_DATA, in("al") 0x01u8);
        asm!("out dx, al", in("dx") PIC2_DATA, in("al") 0x01u8);

        // OCW1: Disable all IRQs
        asm!("out dx, al", in("dx") PIC1_DATA, in("al") 0xFFu8);
        asm!("out dx, al", in("dx") PIC2_DATA, in("al") 0xFFu8);
    }
}

/// Initialize Local APIC
pub fn init_lapic() {
    // Enable Local APIC by setting SVR bit 8
    let mut svr = mmio::read_u32(LAPIC_BASE + LAPIC_SVR);
    svr |= 0x100; // Enable bit
    mmio::write_u32(LAPIC_BASE + LAPIC_SVR, svr);
}

/// End of Interrupt - acknowledge interrupt to Local APIC
pub fn eoi() {
    mmio::write_u32(LAPIC_BASE + LAPIC_EOI, 0);
}

/// Send IPI (Inter-Processor Interrupt)
pub fn send_ipi(cpu_id: u8, vector: u8) {
    // ICR2: Destination
    let icr2 = ((cpu_id as u32) << 24) & 0xFF_0000;
    mmio::write_u32(LAPIC_BASE + LAPIC_ICR2, icr2);

    // ICR1: Vector and delivery mode
    let icr1 = (vector as u32) | 0x4000; // Fixed delivery mode
    mmio::write_u32(LAPIC_BASE + LAPIC_ICR1, icr1);

    // Wait for delivery
    while mmio::read_u32(LAPIC_BASE + LAPIC_ICR1) & 0x1000 != 0 {
        core::hint::spin_loop();
    }
}

/// Enable interrupts
pub fn enable() {
    unsafe {
        asm!("sti", options(nostack));
    }
}

/// Disable interrupts
pub fn disable() {
    unsafe {
        asm!("cli", options(nostack));
    }
}

/// Check if interrupts are enabled
pub fn are_enabled() -> bool {
    let rflags: u64;
    unsafe {
        asm!("pushfq; pop {}", out(reg) rflags, options(nostack));
    }
    (rflags & 0x200) != 0
}
