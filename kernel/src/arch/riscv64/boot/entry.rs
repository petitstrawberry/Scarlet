use core::{arch::naked_asm, mem::transmute};

use crate::{arch::{Riscv64, riscv64::{TRAPFRAME, trap_init}}, device::fdt::{init_fdt, relocate_fdt, create_bootinfo_from_fdt}, environment::STACK_SIZE, mem::{__FDT_RESERVED_START, init_bss}, start_kernel};

/// Entry point for the primary core
#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_entry")]
#[unsafe(naked)]
pub extern "C" fn _entry() {
    unsafe {
        naked_asm!("
        .attribute arch, \"rv64gc\"
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

                la     t0, arch_start_kernel
                jr      t0
        ", const STACK_SIZE
        );
    }
}

/// Entry point for the secondary cores
#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_entry_ap")]
#[unsafe(naked)]
pub extern "C" fn _entry_ap() {
    unsafe {
        naked_asm!("
        .attribute arch, \"rv64gc\"
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

                // Use indirect jump to avoid JAL range limitation
                la      t0, start_ap
                jr      t0
        ", const STACK_SIZE
        );
    }
}



#[unsafe(no_mangle)]
pub extern "C" fn arch_start_kernel(hartid: usize, fdt_ptr: usize) {
    // Initialize .bss section
    init_bss();
    // Initialize FDT
    init_fdt(fdt_ptr);
    
    // Relocate FDT to safe memory
    let fdt_reloc_start = unsafe { &__FDT_RESERVED_START as *const usize as usize };
    let dest_ptr = fdt_reloc_start as *mut u8;
    let relocated_fdt_area = relocate_fdt(dest_ptr);
    
    // Create BootInfo with relocated FDT address
    let bootinfo = create_bootinfo_from_fdt(hartid, relocated_fdt_area.start);

    crate::early_println!("Hart {}: Initializing core....", hartid);
    // Get raw Riscv64 struct
    let riscv: &mut Riscv64 = unsafe { transmute(&TRAPFRAME[hartid] as *const _ as usize ) };
    trap_init(riscv);

    start_kernel(&bootinfo);
}