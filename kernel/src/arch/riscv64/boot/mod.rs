#[cfg(feature = "limine")]
pub mod limine;

use core::arch::asm;

use crate::{
    arch::{
        Riscv64, fpu,
        riscv64::CPUS,
        trap::kernel::{_kernel_trap_entry, arch_kernel_trap_handler},
    },
    early_println,
    environment::STACK_SIZE,
    mem::KERNEL_STACK,
};

pub fn init_boot_cpu(cpu_id: usize) {
    early_println!("[riscv64] init_boot_cpu: cpu_id={}", cpu_id);
    let riscv = unsafe { &mut *(&raw mut CPUS[cpu_id]) };
    early_println!(
        "[riscv64] init_boot_cpu: cpu struct={:#x}",
        riscv as *mut _ as usize
    );
    trap_init(riscv);
    early_println!("[riscv64] init_boot_cpu: done");
}

#[allow(static_mut_refs)]
pub(crate) fn trap_init(riscv: &mut Riscv64) {
    let trap_stack_start = unsafe { KERNEL_STACK.start() };
    let stack_size = STACK_SIZE;

    let trap_stack = trap_stack_start + stack_size * (riscv.hartid + 1) as usize;
    riscv.kernel_stack = trap_stack as u64;
    riscv.kernel_trap = arch_kernel_trap_handler as u64;
    let scratch_addr = riscv as *const _ as usize;

    early_println!(
        "[riscv64] trap_init: hart={} trap_stack={:#x} scratch={:#x}",
        riscv.hartid,
        trap_stack,
        scratch_addr
    );

    let sie: usize = 0x20;
    unsafe {
        asm!("
        csrci sstatus, 0x2 // Disable interrupts
        csrw  sie, {0}
        csrw  stvec, {1}
        csrw  sscratch, {2}
        ",
        in(reg) sie,
        in(reg) _kernel_trap_entry as usize,
        in(reg) scratch_addr,
        );
    }

    early_println!("[riscv64] trap_init: trap CSRs installed");

    // Enable FPU for user-space and kernel access
    fpu::enable_fpu();
    early_println!("[riscv64] trap_init: FPU enabled");

    // Enable Vector extension for user-space and kernel access
    fpu::enable_vector();
    early_println!("[riscv64] trap_init: Vector enabled");

    // early_println!("Trap stack area    : {:#x} - {:#x}", trap_stack - stack_size, trap_stack - 1);
    // early_println!("Trap stack size    : {:#x}", stack_size);
    // early_println!("Trap stack pointer : {:#x}", trap_stack);
    // early_println!("Scratch address    : {:#x}", scratch_addr);
}
