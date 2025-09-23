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
pub use registers::IntRegisters;
pub use context::KernelContext;

pub type Arch = Aarch64;

#[unsafe(link_section = ".trampoline.data")]
static mut CPUS: [Aarch64; NUM_OF_CPUS] = [const { Aarch64::new(0) }; NUM_OF_CPUS];

pub fn init_arch(cpu_id: usize) {
    early_println!("[aarch64] CPU {}: Initializing core....", cpu_id);
    // Get raw Aarch64 struct
    let aarch64: &mut Aarch64 = unsafe { transmute(&CPUS[cpu_id] as *const _ as usize) };
    trap_init(aarch64);
}

#[repr(align(4))]
#[derive(Debug, Clone)]
pub struct Aarch64 {
    scratch: u64,       // offeset: 0 (unused, for compatibility)
    pub cpuid: u64,     // offset: 8 (equivalent to hartid in RISC-V)
    ttbr0: u64,         // offset: 16 (equivalent to satp in RISC-V)
    kernel_stack: u64,  // offset: 24
    kernel_trap: u64,   // offset: 32
}

impl Aarch64 {
    pub const fn new(cpu_id: usize) -> Self {
        Aarch64 { 
            scratch: 0,
            cpuid: cpu_id as u64, 
            ttbr0: 0,
            kernel_stack: 0, 
            kernel_trap: 0,
        }
    }

    pub fn get_cpuid(&self) -> usize {
        self.cpuid as usize
    }

    pub fn get_trapframe_paddr(&self) -> usize {
        /* Get pointer of the trapframe, which is located at the top of the kernel stack */
        let addr = self.kernel_stack as usize - core::mem::size_of::<Trapframe>();
        addr
    }

    pub fn set_kernel_stack(&mut self, initial_top: u64) {
        self.kernel_stack = initial_top;
    }

    pub fn set_trap_handler(&mut self, addr: usize) {
        self.kernel_trap = addr as u64;
    }

    pub fn set_next_address_space(&mut self, asid: u16) {
        // TODO: Implement TTBR0 setup for aarch64 once VM is implemented
        early_println!("[aarch64] TODO: set_next_address_space for ASID {}", asid);
    }

    pub fn as_paddr_cpu(&mut self) -> &mut Aarch64 {
        unsafe {
            &mut CPUS[self.cpuid as usize]
        }
    }
}

#[repr(align(4))]
#[derive(Debug, Clone)]
pub struct Trapframe {
    pub regs: IntRegisters,
    pub epc: u64,  // Using epc name for compatibility (maps to ELR_EL1 in AArch64)
}

impl Trapframe {
    pub fn new() -> Self {
        Trapframe {
            regs: IntRegisters::new(),
            epc: 0,
        }
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

    /// Increment the program counter (epc) to the next instruction
    /// This is typically used after handling a trap or syscall to continue execution.
    /// 
    pub fn increment_pc_next(&mut self, _task: &Task) {
        // AArch64 instructions are 4 bytes (32-bit) in AArch64 state
        self.epc += 4;
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
    let trap_stack_start = unsafe { KERNEL_STACK.start() };
    let stack_size = STACK_SIZE;

    let trap_stack = trap_stack_start + stack_size * (aarch64.cpuid + 1) as usize;
    aarch64.kernel_stack = trap_stack as u64;
    // TODO: Set kernel trap handler once implemented
    
    let scratch_addr = aarch64 as *const _ as usize;
    
    // Set up thread pointer register to point to our aarch64 struct
    unsafe {
        asm!(
            "msr tpidr_el1, {0}",
            in(reg) scratch_addr,
        );
    }
}

pub fn set_trapvector(addr: usize) {
    // TODO: Implement setting VBAR_EL1 for AArch64
    early_println!("[aarch64] TODO: set_trapvector to {:#x}", addr);
}

pub fn set_arch(addr: usize) {
    // Set TPIDR_EL1 to point to the Aarch64 struct (equivalent to sscratch in RISC-V)
    unsafe {
        asm!(
            "msr tpidr_el1, {0}",
            in(reg) addr,
        );
    }
}

pub fn enable_interrupt() {
    unsafe {
        asm!("msr daifclr, #0xf");
    }
}

pub fn disable_interrupt() {
    unsafe {
        asm!("msr daifset, #0xf");
    }
}

pub fn get_cpu() -> &'static mut Aarch64 {
    // Get the Aarch64 struct address from TPIDR_EL1 (equivalent to sscratch in RISC-V)
    let scratch_addr: usize;
    unsafe {
        asm!(
            "mrs {0}, tpidr_el1",
            out(reg) scratch_addr,
        );
    }
    
    if scratch_addr == 0 {
        // Fallback: get CPU ID from MPIDR_EL1 and use that
        let core_id = get_current_cpu_id();
        early_println!("[aarch64] Warning: TPIDR_EL1 not set, using MPIDR_EL1 core {}", core_id);
        unsafe { &mut CPUS[core_id] }
    } else {
        unsafe { transmute(scratch_addr) }
    }
}

/// Get current CPU core ID from MPIDR_EL1 register
pub fn get_current_cpu_id() -> usize {
    let mpidr: u64;
    unsafe {
        asm!(
            "mrs {0}, MPIDR_EL1",
            out(reg) mpidr,
        );
    }
    // Extract Aff0 field (bits 7:0) which contains the core ID
    (mpidr & 0xFF) as usize
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

pub fn shutdown() -> ! {
    // TODO: Implement PSCI shutdown for AArch64
    early_println!("[aarch64] Shutdown requested - entering infinite loop");
    loop {
        unsafe {
            asm!("wfi");
        }
    }
}

pub fn shutdown_with_code(exit_code: u32) -> ! {
    early_println!("[aarch64] Shutdown with exit code {} requested", exit_code);
    shutdown()
}

pub fn reboot() -> ! {
    // TODO: Implement PSCI reboot for AArch64
    early_println!("[aarch64] Reboot requested - entering infinite loop");
    loop {
        unsafe {
            asm!("wfi");
        }
    }
}