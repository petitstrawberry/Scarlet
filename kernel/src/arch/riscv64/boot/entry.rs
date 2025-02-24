use core::arch::naked_asm;

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
                li      t0, 1        
                li      t1, 14
                sll     t0, t0, t1
                mul     t0, t0, a0          
                la      sp, __KERNEL_STACK_BOTTOM
                add     sp, sp, t0

                j       start_kernel
        "
        );
    }
}

/// Entry point for the secondary cores
#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_entry_ap")]
#[naked]
pub extern "C" fn _entry_ap() {
    unsafe {
        naked_asm! ("
        .option norvc
        .option norelax
        .align 8
                // a0 = hartid
                li      t0, 1        
                li      t1, 14
                sll     t0, t0, t1
                mul     t0, t0, a0          
                la      sp, __KERNEL_STACK_BOTTOM
                add     sp, sp, t0

                j       start_ap
        "
        );
    }
}