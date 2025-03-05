use core::arch::asm;
use core::mem::transmute;
use instruction::sbi::sbi_system_reset;
use trap::arch_trap_handler;
use trap::kernel::_trap_entry;
use trap::user::_user_trap_entry;

use crate::early_println;
use crate::early_print;
use crate::environment::NUM_OF_CPUS;

pub mod boot;
pub mod instruction;
pub mod kernel;
pub mod trap;
pub mod earlycon;
pub mod vcpu;
pub mod timer;
pub mod vm;
pub mod registers;

pub use earlycon::*;
pub use registers::Registers;

pub type Arch = Riscv64;
pub type TrapFrame = Riscv64;

#[unsafe(link_section = ".trampoline.data")]
static mut TRAP_FRAME: [Riscv64; NUM_OF_CPUS] = [const { Riscv64::new(0) }; NUM_OF_CPUS];

#[repr(align(4))]
#[derive(Debug)]
pub struct Riscv64 {
    regs: Registers,
    epc: u64,
    hartid: u64,
    kernel_stack: u64,
    kernel_trap: u64,
}

pub fn arch_init(cpu_id: usize) {
    early_println!("[riscv64] Hart {}: Initializing core....", cpu_id);
    // Get raw Riscv64 struct
    let riscv: &mut Riscv64 = unsafe { transmute(&TRAP_FRAME[cpu_id] as *const _ as usize ) };
    trap_init(riscv);
}

impl Riscv64 {
    pub const fn new(cpu_id: usize) -> Self {
        Riscv64 { hartid: cpu_id as u64, epc: 0, regs: Registers::new(), kernel_stack: 0, kernel_trap: 0 }
    }

    pub fn get_cpuid(&self) -> usize {
        self.hartid as usize
    }

    pub fn get_trapframe_paddr(&self) -> usize {
        self as *const _ as usize
    }

    pub fn get_user_trap_entry_paddr(&self) -> usize {
        _user_trap_entry as usize
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
    riscv.kernel_stack = trap_stack as u64;
    riscv.kernel_trap = arch_trap_handler as u64;
    
    let scratch_addr = riscv as *const _ as usize;
    early_println!("[riscv64] Hart {}: Scratch address    : {:#x}", riscv.hartid, scratch_addr);
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
        in(reg) scratch_addr,
        );
    }
}

pub fn set_trap_vector(addr: usize) {
    unsafe {
        asm!("
        csrw stvec, {0}
        ",
        in(reg) addr,
        );
    }
}

pub fn set_trap_frame(addr: usize) {
    unsafe {
        asm!("
        csrw sscratch, {0}
        ",
        in(reg) addr,
        );
    }
}

pub fn enable_interrupt() {
    unsafe {
        asm!("
        csrsi sstatus, 0x2
        ");
    }
}

pub fn disable_interrupt() {
    unsafe {
        asm!("
        csrci sstatus, 0x2
        ");
    }
}

pub fn get_cpu() -> &'static mut Riscv64 {
    let scratch: usize;

    unsafe {
        asm!("
        csrr {0}, sscratch
        ",
        out(reg) scratch,
        );
    }
    unsafe { transmute(scratch) }
}

pub fn shutdown() -> ! {
    sbi_system_reset(0, 0);
}

pub fn reboot() -> ! {
    sbi_system_reset(1, 0);
}