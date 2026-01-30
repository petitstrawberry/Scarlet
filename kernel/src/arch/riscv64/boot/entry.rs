use core::{arch::naked_asm, mem::transmute};

use crate::{
    arch::{
        Riscv64,
        riscv64::{CPUS, trap_init},
    },
    device::fdt::{create_bootinfo_from_fdt, init_fdt, relocate_fdt},
    environment::STACK_SIZE,
    mem::{__FDT_RESERVED_START, init_bss},
    start_kernel,
};

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

    // Decide whether user-mode FPU/Vector handling is enabled based on DTB.
    crate::arch::init_user_context_from_fdt();

    crate::early_println!("Hart {}: Initializing core....", hartid);
    // Get raw Riscv64 struct
    let riscv: &mut Riscv64 = unsafe { transmute(&CPUS[hartid] as *const _ as usize) };
    trap_init(riscv);

    // Start secondary CPUs if this is the boot hart (hart 0)
    if hartid == 0 {
        use crate::arch::riscv64::kernel::cpu;
        use crate::arch::riscv64::kernel::smp;

        // Set the boot hart ID
        smp::set_boot_hart_id(hartid);

        // Get CPU information from device tree
        if let Some((num_cpus, max_hart_id)) = cpu::get_cpu_info_from_fdt() {
            crate::early_println!(
                "[SMP] Boot hart: Detected {} CPUs in device tree, max hart ID: {}",
                num_cpus,
                max_hart_id
            );

            // Start secondary CPUs
            if num_cpus > 1 {
                smp::start_secondary_cpus(hartid, max_hart_id);
            }
        } else {
            crate::early_println!("[SMP] No CPU info found in device tree, skipping SMP init");
        }
    }

    start_kernel(&bootinfo);
}
