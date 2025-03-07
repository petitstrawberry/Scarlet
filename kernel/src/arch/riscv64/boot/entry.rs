use core::arch::naked_asm;

use crate::environment::STACK_SIZE;

/// Entry point for the primary core
#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_entry")]
#[naked]
pub extern "C" fn _entry() {
    unsafe {
        naked_asm!("
        .option norvc
        .option norelax
        .align 8
                // a0 = hartid     
                li      t0, {}
                mv      t1, a0
                addi    t1, t1, 1
                mul     t1, t1, a0          
                la      sp, KERNEL_STACK
                add     sp, sp, t0

                j       start_kernel
        ", const STACK_SIZE
        );
    }
}

/// Entry point for the secondary cores
#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_entry_ap")]
#[naked]
pub extern "C" fn _entry_ap() {
    unsafe {
        naked_asm!("
        .option norvc
        .option norelax
        .align 8
                // a0 = hartid     
                li      t0, {}
                mv      t1, a0
                addi    t1, t1, 1
                mul     t1, t1, a0          
                la      sp, KERNEL_STACK
                add     sp, sp, t0

                j       start_ap
        ", const STACK_SIZE
        );
    }
}