#![no_std]
#![no_main]

extern crate scarlet;

use core::arch::naked_asm;

use scarlet::environment::STACK_SIZE;

#[unsafe(link_section = ".init")]
#[unsafe(no_mangle)]
pub extern "C" fn arch_start_kernel() -> ! {
    scarlet::arch::riscv64::boot::limine::limine_entry()
}

/// Entry point for the secondary cores
#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_entry_ap")]
#[unsafe(naked)]
pub extern "C" fn _entry_ap() {
    naked_asm!("
        .attribute arch, \"rv64gc\"
        .option norvc
        .option norelax
        .align 8
                li      t0, {}
                mv      t1, a0
                addi    t1, t1, 1
                mul     t1, t1, a0          
                la      sp, KERNEL_STACK
                add     sp, sp, t0

                // Use indirect jump to avoid JAL range limitation
                la      t0, start_ap
                jr      t0
        ", const STACK_SIZE
    );
}
