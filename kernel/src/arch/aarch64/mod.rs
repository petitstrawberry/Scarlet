use core::arch::asm;
use core::mem::transmute;

use crate::early_println;
use crate::environment::NUM_OF_CPUS;
use crate::environment::STACK_SIZE;
use crate::mem::KERNEL_STACK;
use crate::task::Task;

pub mod boot;
pub mod context;
pub mod earlycon;
pub mod instruction;
pub mod interrupt;
pub mod kernel;
pub mod registers;
pub mod switch;
pub mod timer;
pub mod trap;
pub mod vcpu;
pub mod vm;

pub use earlycon::*;
pub use registers::Registers;
pub use context::KernelContext;

pub type Arch = Aarch64;
pub type Trapframe = Aarch64;

#[unsafe(link_section = ".trampoline.data")]
static mut TRAPFRAME: [Aarch64; NUM_OF_CPUS] = [const { Aarch64::new(0) }; NUM_OF_CPUS];

#[repr(align(4))]
#[derive(Debug, Clone)]
pub struct Aarch64 {
    pub regs: Registers,
    pub elr: u64,    // Exception Link Register (equivalent to epc in RISC-V)
    pub spsr: u64,   // Saved Program Status Register
    pub cpuid: u64,  // CPU ID (equivalent to hartid in RISC-V)
    ttbr0: u64,      // Translation Table Base Register 0 (equivalent to satp in RISC-V)
    kernel_stack: u64,
    kernel_trap: u64,
}

pub fn init_arch(cpu_id: usize) {
    early_println!("[aarch64] CPU {}: Initializing core....", cpu_id);
    // Get raw Aarch64 struct
    let aarch64: &mut Aarch64 = unsafe { transmute(&TRAPFRAME[cpu_id] as *const _ as usize ) };
    trap_init(aarch64);
}

impl Aarch64 {
    pub const fn new(cpu_id: usize) -> Self {
        Aarch64 { 
            cpuid: cpu_id as u64, 
            elr: 0, 
            spsr: 0,
            regs: Registers::new(), 
            kernel_stack: 0, 
            kernel_trap: 0, 
            ttbr0: 0 
        }
    }

    pub fn get_cpuid(&self) -> usize {
        self.cpuid as usize
    }

    pub fn get_trapframe_paddr(&self) -> usize {
        /* Get pointer of TRAP_FRAME[cpuid] */
        let addr = unsafe { &raw mut TRAPFRAME[self.cpuid as usize] } as *const _ as usize;
        addr
    }

    pub fn get_trapframe(&mut self) -> &mut Trapframe {
        self
    }
}

impl Trapframe {
    pub fn set_trap_handler(&mut self, addr: usize) {
        self.kernel_trap = addr as u64;
    }

    pub fn set_next_address_space(&mut self, asid: u16) {
        // TODO: Implement TTBR0 setup for aarch64 once VM is implemented
        // let root_pagetable = get_root_pagetable(asid).expect("No root page table found for ASID");
        // let ttbr0 = root_pagetable.get_val_for_ttbr0(asid);
        // self.ttbr0 = ttbr0 as u64;
        early_println!("[aarch64] TODO: set_next_address_space for ASID {}", asid);
    }

    pub fn set_kernel_stack(&mut self, initial_top: u64) {
        self.kernel_stack = initial_top;
    }

    pub fn get_syscall_number(&self) -> usize {
        self.regs.reg[8] // X8 is used for syscall number in AArch64
    }

    pub fn set_syscall_number(&mut self, syscall_number: usize) {
        self.regs.reg[8] = syscall_number; // X8
    }

    pub fn get_return_value(&self) -> usize {
        self.regs.reg[0] // X0 is used for return value in AArch64
    }

    pub fn set_return_value(&mut self, value: usize) {
        self.regs.reg[0] = value; // X0
    }

    pub fn get_arg(&self, index: usize) -> usize {
        // Arguments are passed in X0-X7 in AArch64
        if index < 8 {
            self.regs.reg[index]
        } else {
            0 // TODO: Handle arguments on stack
        }
    }

    pub fn set_arg(&mut self, index: usize, value: usize) {
        // Arguments are passed in X0-X7 in AArch64
        if index < 8 {
            self.regs.reg[index] = value;
        }
        // TODO: Handle arguments on stack
    }

    /// Increment the program counter (elr) to the next instruction
    /// This is typically used after handling a trap or syscall to continue execution.
    /// 
    pub fn increment_pc_next(&mut self, _task: &Task) {
        // AArch64 instructions are 4 bytes (32-bit) in AArch64 state
        // TODO: Handle Thumb mode (2-byte instructions) if needed
        self.elr += 4;
    }
}

pub fn get_user_trapvector_paddr() -> usize {
    // TODO: Implement user trap entry
    0
}

pub fn get_kernel_trapvector_paddr() -> usize {
    // TODO: Implement kernel trap entry
    0
}

pub fn get_kernel_trap_handler() -> usize {
    // TODO: Implement kernel trap handler
    0
}

pub fn get_user_trap_handler() -> usize {
    // TODO: Implement user trap handler
    0
}

#[allow(static_mut_refs)]
fn trap_init(aarch64: &mut Aarch64) {
    early_println!("[aarch64] CPU {}: Initializing trap....", aarch64.cpuid);
    
    let trap_stack_start = unsafe { KERNEL_STACK.start() };
    let stack_size = STACK_SIZE;

    let trap_stack = trap_stack_start + stack_size * (aarch64.cpuid + 1) as usize;
    early_println!("[aarch64] CPU {}: Trap stack area    : {:#x} - {:#x}", aarch64.cpuid, trap_stack - stack_size, trap_stack - 1);
    early_println!("[aarch64] CPU {}: Trap stack size    : {:#x}", aarch64.cpuid, stack_size);
    early_println!("[aarch64] CPU {}: Trap stack pointer : {:#x}", aarch64.cpuid, trap_stack);
    early_println!("[aarch64] CPU {}: Setting up scratch space....", aarch64.cpuid);
    aarch64.kernel_stack = trap_stack as u64;
    // TODO: Set kernel trap handler once implemented
    
    let scratch_addr = aarch64 as *const _ as usize;
    early_println!("[aarch64] CPU {}: Scratch address    : {:#x}", aarch64.cpuid, scratch_addr);
    
    // TODO: Set up AArch64 exception handling registers
    // This would involve setting VBAR_EL1, TPIDR_EL1, etc.
    early_println!("[aarch64] TODO: Complete trap initialization");
}

pub fn set_trapvector(addr: usize) {
    // TODO: Implement setting VBAR_EL1 for AArch64
    early_println!("[aarch64] TODO: set_trapvector to {:#x}", addr);
}

pub fn set_trapframe(addr: usize) {
    // TODO: Implement setting thread pointer for AArch64
    early_println!("[aarch64] TODO: set_trapframe to {:#x}", addr);
}

pub fn enable_interrupt() {
    // TODO: Implement enabling interrupts for AArch64
    // This would involve clearing the interrupt mask bits in DAIF
    early_println!("[aarch64] TODO: enable_interrupt");
}

pub fn get_cpu() -> &'static mut Aarch64 {
    // TODO: Implement proper CPU identification for AArch64
    // For now, return CPU 0's trapframe
    unsafe { &mut TRAPFRAME[0] }
}

pub fn set_next_mode(mode: vcpu::Mode) {
    // TODO: Implement mode switching for AArch64
    // This would involve setting SPSR_EL1 appropriately
    match mode {
        vcpu::Mode::User => {
            early_println!("[aarch64] TODO: set_next_mode to User");
        }
        vcpu::Mode::Kernel => {
            early_println!("[aarch64] TODO: set_next_mode to Kernel");
        }
    }
}

pub fn early_putc(c: u8) {
    // TODO: Implement early console output for AArch64
    // For now, just ignore
    let _ = c;
}