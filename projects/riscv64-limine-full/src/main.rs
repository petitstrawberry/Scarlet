#![no_std]
#![no_main]

extern crate scarlet_modules;

use core::arch::naked_asm;

use scarlet_modules::scarlet::{environment::STACK_SIZE, start_ap};

// Placeholder to force the linker to emit the .scarlet_ksyms section with enough
// room for the post-link tool to inject the real symbol table via objcopy --update-section.
#[unsafe(link_section = ".scarlet_ksyms")]
#[used]
static _KSYM_PLACEHOLDER: [u64; 65536] = [0u64; 65536];

#[unsafe(link_section = ".init")]
#[unsafe(no_mangle)]
pub extern "C" fn arch_start_kernel() -> ! {
    scarlet_modules::force_link();
    scarlet_modules::scarlet::arch::riscv64::boot::limine::limine_entry()
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
                li      t0, {stack_size}
                mv      t1, a0
                addi    t1, t1, 1
                mul     t1, t1, t0
                la      sp, KERNEL_STACK
                add     sp, sp, t1

                // Use indirect jump to avoid JAL range limitation
                la      t0, {start_ap}
                jr      t0
        ", stack_size = const STACK_SIZE,
           start_ap = sym start_ap,
    );
}
