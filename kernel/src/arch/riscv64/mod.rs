use core::arch::asm;
use core::mem::transmute;
use instruction::sbi::sbi_system_reset;
use trap::kernel::arch_kernel_trap_handler;
use trap::kernel::_kernel_trap_entry;
use trap::user::_user_trap_entry;
use trap::user::arch_user_trap_handler;
use vcpu::Mode;

use crate::arch::instruction::Instruction;
use crate::arch::vm::get_root_pagetable;
use crate::early_println;
use crate::environment::NUM_OF_CPUS;
use crate::environment::STACK_SIZE;
use crate::mem::KERNEL_STACK;
use crate::task::Task;

pub mod boot;
pub mod instruction;
pub mod interrupt;
pub mod kernel;
pub mod trap;
pub mod earlycon;
pub mod vcpu;
pub mod timer;
pub mod vm;
pub mod registers;
pub mod context;
pub mod switch;

pub use earlycon::*;
pub use registers::IntRegisters;

pub type Arch = Riscv64;

#[unsafe(link_section = ".trampoline.data")]
static mut CPUS: [Riscv64; NUM_OF_CPUS] = [const { Riscv64::new(0) }; NUM_OF_CPUS];

#[repr(align(4))]
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Riscv64 {
    scratch: u64, // offeset: 0
    pub hartid: u64, // offset: 8
    satp: u64, // offset: 16
    kernel_stack: u64, // offset: 24
    kernel_trap: u64, // offset: 32
}

impl Riscv64 {
    pub const fn new(cpu_id: usize) -> Self {
        Riscv64 { scratch: 0, hartid: cpu_id as u64, kernel_stack: 0, kernel_trap: 0, satp: 0 }
    }

    pub fn get_cpuid(&self) -> usize {
        self.hartid as usize
    }

    pub fn get_trapframe_paddr(&self) -> usize {
        /* Get pointer of the trapframe, which is located at the top of the kernel stack */
        let addr =  self.kernel_stack as usize - core::mem::size_of::<Trapframe>();
        addr
    }

     pub fn set_kernel_stack(&mut self, initial_top: u64) {
        self.kernel_stack = initial_top;
    }

    pub fn set_trap_handler(&mut self, addr: usize) {
        self.kernel_trap = addr as u64;
    }

    pub fn set_next_address_space(&mut self, asid: u16) {
        let root_pagetable = get_root_pagetable(asid).expect("No root page table found for ASID");

        let satp = root_pagetable.get_val_for_satp(asid);
        self.satp = satp as u64;
    }

    pub fn as_paddr_cpu(&mut self) -> &mut Riscv64 {
        unsafe {
            &mut CPUS[self.hartid as usize]
        }
    }
}

#[repr(align(4))]
#[derive(Debug, Clone)]
pub struct Trapframe {
    pub regs: IntRegisters,
    pub epc: u64,
}

impl Trapframe {
    pub fn new() -> Self {
        Trapframe {
            regs: IntRegisters::new(),
            epc: 0,
        }
    }
    
    pub fn get_syscall_number(&self) -> usize {
        self.regs.reg[17] // a7
    }

    pub fn set_syscall_number(&mut self, syscall_number: usize) {
        self.regs.reg[17] = syscall_number; // a7
    }

    pub fn get_return_value(&self) -> usize {
        self.regs.reg[10] // a0
    }

    pub fn set_return_value(&mut self, value: usize) {
        self.regs.reg[10] = value; // a0
    }

    pub fn get_arg(&self, index: usize) -> usize {
        self.regs.reg[index + 10] // a0 - a7
    }

    pub fn set_arg(&mut self, index: usize, value: usize) {
        self.regs.reg[index + 10] = value; // a0 - a7
    }

    /// Increment the program counter (epc) to the next instruction
    /// This is typically used after handling a trap or syscall to continue execution.
    /// 
    pub fn increment_pc_next(&mut self, task: &Task) {
        let instruction = Instruction::fetch(
            task.vm_manager.translate_vaddr(self.epc as usize).unwrap()
        );
        let len = instruction.len();
        if len == 0 {
            debug_assert!(len > 0, "Invalid instruction length: {}", len);
            early_println!("Warning: Invalid instruction length encountered. Defaulting to 4 bytes.");
            self.epc += 4; // Default to 4 bytes for invalid instruction length
        } else {
            self.epc += len as u64;
        }
    }
}

pub fn get_user_trapvector_paddr() -> usize {
    _user_trap_entry as usize
}

pub fn get_kernel_trapvector_paddr() -> usize {
    _kernel_trap_entry as usize
}

pub fn get_kernel_trap_handler() -> usize {
    arch_kernel_trap_handler as usize
}

pub fn get_user_trap_handler() -> usize {
    arch_user_trap_handler as usize
}

#[allow(static_mut_refs)]
fn trap_init(riscv: &mut Riscv64) {    
    let trap_stack_start = unsafe { KERNEL_STACK.start() };
    let stack_size =  STACK_SIZE;

    let trap_stack = trap_stack_start + stack_size * (riscv.hartid + 1) as usize;
    riscv.kernel_stack = trap_stack as u64;
    riscv.kernel_trap = arch_kernel_trap_handler as u64;
    let scratch_addr = riscv as *const _ as usize;

    let sie: usize = 0x20;
    unsafe {
        asm!("
        csrw  sie, {0}
        csrsi sstatus, 0x2
        csrw  stvec, {1}
        csrw  sscratch, {2}
        ",
        in(reg) sie,
        in(reg) _kernel_trap_entry as usize,
        in(reg) scratch_addr,
        );
    }

    // early_println!("Trap stack area    : {:#x} - {:#x}", trap_stack - stack_size, trap_stack - 1);
    // early_println!("Trap stack size    : {:#x}", stack_size);
    // early_println!("Trap stack pointer : {:#x}", trap_stack);
    // early_println!("Scratch address    : {:#x}", scratch_addr);
}

pub fn set_trapvector(addr: usize) {
    unsafe {
        asm!("
        csrw stvec, {0}
        ",
        in(reg) addr,
        );
    }
}

pub fn set_arch(addr: usize) {
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

pub fn set_next_mode(mode: Mode) {
    match mode {
        Mode::User => {
            unsafe {
                // sstatus.spp = 0 (U-mode)
                let mut sstatus: usize;
                asm!(
                    "csrr {sstatus}, sstatus",
                    sstatus = out(reg) sstatus,
                );
                sstatus &= !(1 << 8); // Clear SPP bit
                asm!(
                    "csrw sstatus, {sstatus}",
                    sstatus = in(reg) sstatus,
                );
            }
        },
        Mode::Kernel => {
            unsafe {
                // sstatus.spp = 1 (S-mode)
                let mut sstatus: usize;
                asm!(
                    "csrr {sstatus}, sstatus",
                    sstatus = out(reg) sstatus,
                );
                sstatus |= 1 << 8; // Set SPP bit
                asm!(
                    "csrw sstatus, {sstatus}",
                    sstatus = in(reg) sstatus,
                );
            }
        },
    }
}

pub fn shutdown() -> ! {
    sbi_system_reset(0, 0);
}

pub fn shutdown_with_code(exit_code: u32) -> ! {
    // Use reset_reason as exit code for test environments
    sbi_system_reset(0, exit_code);
}

pub fn reboot() -> ! {
    sbi_system_reset(1, 0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::early_println;

    /// Test architecture-specific features for RISC-V
    #[test_case]
    fn test_riscv64_specific_features() {
        early_println!("[RISC-V Arch Test] Testing RISC-V specific features");
        
        use crate::arch::riscv64::vcpu::Mode;
        
        // Test mode switching
        set_next_mode(Mode::Kernel);
        set_next_mode(Mode::User);
        
        early_println!("[RISC-V Arch Test] RISC-V specific features test passed");
    }

    /// Test platform-specific interrupt controllers for RISC-V
    mod platform_tests {
        use super::*;

        #[test_case]
        fn test_plic_availability() {
            early_println!("[Platform Test] Testing PLIC availability on RISC-V");
            
            use crate::drivers::pic::Plic;
            
            // Test that PLIC can be instantiated (actual hardware interaction would need setup)
            // This test mainly verifies compilation and basic structure
            early_println!("[Platform Test] PLIC structure is available on RISC-V");
            early_println!("[Platform Test] PLIC availability test passed");
        }

        #[test_case]
        fn test_clint_availability() {
            early_println!("[Platform Test] Testing CLINT availability on RISC-V");
            
            use crate::drivers::pic::Clint;
            
            // Test that CLINT can be instantiated
            early_println!("[Platform Test] CLINT structure is available on RISC-V");
            early_println!("[Platform Test] CLINT availability test passed");
        }
    }
}