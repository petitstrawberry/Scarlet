use core::arch::asm;
use core::mem::transmute;
use trap::_trap_entry;

use crate::early_println;
use crate::early_print;

pub mod boot;
pub mod instruction;
pub mod kernel;
pub mod trap;
pub mod csr;
pub mod earlycon;

pub use earlycon::*;

pub type Arch = Riscv64;

#[repr(align(4))]
pub struct Riscv64 {
    regs: [u64; 32],
    epc: u64,
    hartid: u64,
}

impl Riscv64 {
    pub fn new(cpu_id: usize) -> Self {
        Riscv64 { hartid: cpu_id as u64, epc: 0, regs: [0; 32] }
    }

    pub fn init(&mut self, cpu_id: usize) {
        early_println!("[riscv64] Hart {}: Initializing core....", cpu_id);
        trap_init(self);
    }
}

fn trap_init(riscv: &mut Riscv64) {
    early_println!("[riscv64] Hart {}: Initializing trap....", riscv.hartid);
    let trap_stack_bottom: usize;
    let stack_size = 0x4000;
    unsafe {
        asm!("
        la      {0}, __KERNEL_TRAP_STACK_BOTTOM
        ",
        out(reg) trap_stack_bottom,
        );
    }

    let trap_stack = trap_stack_bottom - stack_size * (riscv.hartid) as usize;
    early_println!("[riscv64] Hart {}: Trap stack bottom  : {:#x}", riscv.hartid, trap_stack_bottom);
    early_println!("[riscv64] Hart {}: Trap stack size    : {:#x}", riscv.hartid, stack_size);

    // Setup for Scratch space for Riscv64 struct
    early_println!("[riscv64] Hart {}: Setting up scratch space....", riscv.hartid);
    let scratch: &mut Riscv64 = unsafe { transmute(trap_stack - 272) };
    scratch.hartid = riscv.hartid;
    let sie: usize = 0x20;
    unsafe {
        asm!("
        csrw  sie, {0}
        csrsi sstatus, 0x2
        csrw  stvec, {1}
        csrw  sscratch, {2}
        ",
        in(reg) sie,
        in(reg) _trap_entry as usize,
        in(reg) trap_stack,
        );
    }
}
