#[cfg(feature = "limine")]
pub mod limine;
#[cfg(all(target_arch = "riscv32", feature = "sbi-boot"))]
pub mod sbi;
#[cfg(test)]
pub mod test;

use core::arch::asm;

use crate::{
    arch::{
        Riscv, fpu,
        riscv::CPUS,
        trap::kernel::{_kernel_trap_entry, arch_kernel_trap_handler},
    },
    environment::STACK_SIZE,
    mem::KERNEL_STACK,
    println,
};

/// Initialize the current RISC-V CPU's per-CPU trap state.
///
/// # Arguments
///
/// * `cpu_id` - Logical CPU ID assigned to the current hart.
pub fn init_cpu(cpu_id: usize) {
    // SAFETY: Boot code initializes each `CPUS[cpu_id]` slot exactly once on
    // its assigned hart before publishing the pointer through `sscratch`.
    let riscv = unsafe { &mut *(&raw mut CPUS[cpu_id]) };
    riscv.cpu_id = cpu_id;
    trap_init(riscv);
    println!(
        "[riscv] init_cpu: cpu_id={} cpu struct={:#x} done",
        cpu_id, riscv as *mut _ as usize
    );
}

#[allow(static_mut_refs)]
pub(crate) fn trap_init(riscv: &mut Riscv) {
    // SAFETY: Per-hart boot owns its assigned kernel-stack slot.
    let trap_stack_start = unsafe { KERNEL_STACK.start() };
    let stack_size = STACK_SIZE;

    let trap_stack = trap_stack_start + stack_size * (riscv.cpu_id + 1) as usize;
    riscv.kernel_stack = trap_stack;
    riscv.kernel_trap = arch_kernel_trap_handler as usize;
    let scratch_addr = riscv as *const _ as usize;

    let sie: usize = 0x20;
    // SAFETY: This runs during per-hart boot with interrupts disabled. The
    // prepared stack, trap vector, and per-CPU pointer are valid for this hart.
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

    println!(
        "[riscv] trap_init: cpu={} trap_stack={:#x} scratch={:#x} trap CSRs installed",
        riscv.cpu_id, trap_stack, scratch_addr
    );

    #[cfg(feature = "user-fpu")]
    {
        fpu::enable_fpu();
        println!("[riscv] trap_init: FPU context enabled");
    }
    #[cfg(feature = "user-vector")]
    {
        fpu::enable_vector();
        println!("[riscv] trap_init: Vector context enabled");
    }

    // println!("Trap stack area    : {:#x} - {:#x}", trap_stack - stack_size, trap_stack - 1);
    // println!("Trap stack size    : {:#x}", stack_size);
    // println!("Trap stack pointer : {:#x}", trap_stack);
    // println!("Scratch address    : {:#x}", scratch_addr);
}
