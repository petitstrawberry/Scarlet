#![no_std]
#![no_main]

use core::arch::naked_asm;

use scarlet_modules::scarlet::{environment::STACK_SIZE, start_ap};

extern crate scarlet_modules;

#[unsafe(link_section = ".init")]
#[unsafe(no_mangle)]
pub extern "C" fn arch_start_kernel() -> ! {
    scarlet_modules::force_link();
    scarlet_modules::scarlet::arch::aarch64::boot::limine::limine_entry()
}

#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_entry_ap")]
#[unsafe(naked)]
pub extern "C" fn _entry_ap() {
    unsafe {
        naked_asm!(
            "mrs x4, MPIDR_EL1",
            "and x4, x4, #0xFF",
            "mov x2, {stack_size}",
            "adrp x3, KERNEL_STACK",
            "add x3, x3, :lo12:KERNEL_STACK",
            "add x5, x4, #1",
            "mul x5, x5, x2",
            "add x5, x3, x5",
            "and sp, x5, #~0xF",
            "mov x0, x4",
            "bl {start_ap}",
            "1:",
            "wfi",
            "b 1b",
            stack_size = const STACK_SIZE,
            start_ap = sym start_ap,
        );
    }
}
