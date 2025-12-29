use core::arch::asm;
use core::mem::transmute;
use core::sync::atomic::{AtomicBool, Ordering};

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

pub use context::KernelContext;
pub use earlycon::*;
pub use registers::IntRegisters;

use crate::vm::vmem::MemoryArea;
use crate::arch::vm::get_root_pagetable;

pub type Arch = Aarch64;

// Common scheduler code enables interrupts before calling timer.start().
// On AArch64 we must keep global interrupts masked until the timer has been
// programmed (and the IRQ line is configured) to avoid spurious early IRQs.
static INTERRUPTS_ALLOWED: AtomicBool = AtomicBool::new(false);

/// Allow `enable_interrupt()` to actually unmask interrupts.
///
/// This is called from the AArch64 timer path once the timer has been
/// programmed at least once.
pub fn mark_interrupts_allowed() {
    INTERRUPTS_ALLOWED.store(true, Ordering::Relaxed);
}

pub(crate) fn interrupts_allowed() -> bool {
    INTERRUPTS_ALLOWED.load(Ordering::Relaxed)
}

/// Returns the device memory areas for AArch64 QEMU virt platform.
/// These areas contain memory-mapped I/O devices and should be mapped
/// with device memory attributes (non-cacheable, no speculation).
pub fn get_device_memory_areas() -> alloc::vec::Vec<MemoryArea> {
    alloc::vec![
        // QEMU virt: MMIO devices are below RAM base (0x4000_0000)
        MemoryArea {
            start: 0x0000_0000,
            end: 0x3fff_ffff,
        },
    ]
}

#[unsafe(link_section = ".trampoline.data")]
static mut CPUS: [Aarch64; NUM_OF_CPUS] = [const { Aarch64::new(0) }; NUM_OF_CPUS];

pub fn init_arch(cpu_id: usize) {
    early_println!("[aarch64] CPU {}: Initializing core....", cpu_id);
    // Get raw Aarch64 struct
    let aarch64: &mut Aarch64 = unsafe { transmute(&CPUS[cpu_id] as *const _ as usize) };
    aarch64.cpuid = cpu_id as u64;
    trap_init(aarch64);
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct Aarch64 {
    scratch: u64,      // offeset: 0 (unused, for compatibility)
    pub cpuid: u64,    // offset: 8 (equivalent to hartid in RISC-V)
    ttbr0: u64,        // offset: 16 (equivalent to satp in RISC-V)
    kernel_stack: u64, // offset: 24
    kernel_trap: u64,  // offset: 32
    kernel_ttbr0: u64, // offset: 40 (kernel TTBR0 value for EL0->EL1 entry)
    scratch_x1: u64,   // offset: 48 (temporary save area for EL0 return)
}

impl Aarch64 {
    pub const fn new(cpu_id: usize) -> Self {
        Aarch64 {
            scratch: 0,
            cpuid: cpu_id as u64,
            ttbr0: 0,
            kernel_stack: 0,
            kernel_trap: 0,
            kernel_ttbr0: 0,
            scratch_x1: 0,
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
        let root_pagetable =
            get_root_pagetable(asid).expect("No root page table found for ASID (aarch64)");
        let ttbr_val = root_pagetable.get_val_for_ttbr(asid);
        crate::early_println!("[aarch64] set_next_address_space: asid={} ttbr={:#x}", asid, ttbr_val);
        self.ttbr0 = ttbr_val;
    }

    pub fn get_ttbr0(&self) -> u64 {
        self.ttbr0
    }

    pub fn set_scratch(&mut self, val: u64) {
        self.scratch = val;
    }

    pub fn get_scratch(&self) -> u64 {
        self.scratch
    }

    pub fn set_kernel_ttbr0(&mut self, val: u64) {
        self.kernel_ttbr0 = val;
    }

    pub fn get_kernel_ttbr0(&self) -> u64 {
        self.kernel_ttbr0
    }

    pub fn as_paddr_cpu(&mut self) -> &mut Aarch64 {
        unsafe { &mut CPUS[self.cpuid as usize] }
    }
}

#[repr(C, align(16))]
#[derive(Debug, Clone)]
pub struct Trapframe {
    pub regs: IntRegisters,
    pub epc: u64, // Using epc name for compatibility (maps to ELR_EL1 in AArch64)
    pub _pad: u64,
}

impl Trapframe {
    pub fn new() -> Self {
        Trapframe {
            regs: IntRegisters::new(),
            epc: 0,
            _pad: 0,
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
    trap::user::_user_trap_entry as usize
}

pub fn get_kernel_trapvector_paddr() -> usize {
    // For now, reuse the trampoline vector base.
    // TODO: Provide a dedicated kernel vector table.
    trap::user::_user_trap_entry as usize
}

pub fn get_kernel_trap_handler() -> usize {
    // TODO: Provide a dedicated kernel trap handler.
    trap::user::arch_user_trap_handler as usize
}

pub fn get_user_trap_handler() -> usize {
    trap::user::arch_user_trap_handler as usize
}

#[allow(static_mut_refs)]
fn trap_init(aarch64: &mut Aarch64) {
    let trap_stack_start = unsafe { KERNEL_STACK.start() };
    let stack_size = STACK_SIZE;

    let trap_stack = trap_stack_start + stack_size * (aarch64.cpuid + 1) as usize;
    aarch64.kernel_stack = trap_stack as u64;
    aarch64.kernel_trap = trap::user::arch_user_trap_handler as u64;

    let scratch_addr = aarch64 as *const _ as usize;

    // Set up thread pointer registers to point to our aarch64 struct
    // Use TPIDRRO_EL0 as the single source of truth (trampoline also reads this).
    unsafe {
        asm!(
            "msr tpidrro_el0, {0}",
            "msr tpidr_el1, {0}",
            in(reg) scratch_addr,
        );
    }
}

pub fn set_trapvector(addr: usize) {
    debug_assert_eq!(addr & 0x7ff, 0, "VBAR_EL1 base must be 2KB-aligned");
    unsafe {
        asm!(
            "msr vbar_el1, {0}",
            "isb",
            in(reg) addr,
            options(nostack)
        );
    }
}

pub fn set_arch(addr: usize) {
    // Store the trampoline-visible Aarch64 pointer in TPIDRRO_EL0.
    // TPIDRRO_EL0 is readable from EL0 but not writable by EL0, so user-space
    // cannot clobber it (unlike TPIDR_EL0 which is commonly used for TLS).
    unsafe {
        asm!(
            "msr tpidrro_el0, {0}",
            in(reg) addr,
        );
    }
}

pub fn enable_interrupt() {
    // Keep interrupts globally masked until the timer has started.
    if !interrupts_allowed() {
        unsafe { asm!("msr daifset, #0xf", options(nostack)); }
        return;
    }
    unsafe { asm!("msr daifclr, #0xf", options(nostack)); }
}

pub fn disable_interrupt() {
    unsafe {
        asm!("msr daifset, #0xf", options(nostack));
    }
}

pub fn get_cpu() -> &'static mut Aarch64 {
    // Prefer the EL1 thread pointer (kept at the kernel-mapped Arch address).
    let tpidr_el1: usize;
    unsafe {
        asm!(
            "mrs {0}, tpidr_el1",
            out(reg) tpidr_el1,
        );
    }

    if tpidr_el1 != 0 {
        // Kernel context always has access to this mapping.
        return unsafe { transmute(tpidr_el1) };
    }

    // Fallback to the trampoline-visible pointer for early bring-up or if EL1
    // TPIDR was not initialized yet.
    let tpidrro_el0: usize;
    unsafe {
        asm!(
            "mrs {0}, tpidrro_el0",
            out(reg) tpidrro_el0,
        );
    }

    if tpidrro_el0 == 0 {
        // Final fallback: derive from MPIDR_EL1 if both registers are unset.
        let core_id = get_current_cpu_id();
        early_println!(
            "[aarch64] Warning: TPIDR* not set, using MPIDR_EL1 core {}",
            core_id
        );
        unsafe { &mut CPUS[core_id] }
    } else {
        unsafe { transmute(tpidrro_el0) }
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
    // AArch64 return mode is currently chosen in the trampoline (`_user_trap_exit`).
    // Keep this as a no-op so shared scheduler code can call it without
    // architecture-specific branching or noisy TODO logs.
    let _ = mode;
}

/// Memory barrier for device/MMIO (I/O) operations.
///
/// AArch64 uses DSB and DMB instructions for memory ordering.
/// For device memory (MMIO), we use DSB to ensure all previous memory
/// accesses are complete before proceeding.
#[inline(always)]
pub fn io_mb() {
    unsafe {
        // DSB SY - Data Synchronization Barrier, full system
        asm!("dsb sy", options(nostack));
    }
}

/// Read barrier for device/MMIO (I/O) operations.
#[inline(always)]
pub fn io_rmb() {
    unsafe {
        // DSB LD - Data Synchronization Barrier for loads
        asm!("dsb ld", options(nostack));
    }
}

/// Write barrier for device/MMIO (I/O) operations.
#[inline(always)]
pub fn io_wmb() {
    unsafe {
        // DSB ST - Data Synchronization Barrier for stores
        asm!("dsb st", options(nostack));
    }
}

/// General memory barrier for normal memory operations.
#[inline(always)]
pub fn mb() {
    unsafe {
        // DMB SY - Data Memory Barrier, full system
        asm!("dmb sy", options(nostack));
    }
}

/// Read memory barrier for normal memory operations.
#[inline(always)]
pub fn rmb() {
    unsafe {
        // DMB LD - Data Memory Barrier for loads
        asm!("dmb ld", options(nostack));
    }
}

/// Write memory barrier for normal memory operations.
#[inline(always)]
pub fn wmb() {
    unsafe {
        // DMB ST - Data Memory Barrier for stores
        asm!("dmb st", options(nostack));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Test architecture-specific features for AArch64
    #[test_case]
    fn test_aarch64_specific_features() {
        use crate::arch::aarch64::vcpu::Mode;

        // Test mode switching
        set_next_mode(Mode::Kernel);
        set_next_mode(Mode::User);

        // Test AArch64-specific CPU ID retrieval
        let cpu_id = get_current_cpu_id();
        assert!(
            cpu_id < crate::environment::NUM_OF_CPUS,
            "AArch64 CPU ID should be within valid range"
        );
    }

    /// Test platform-specific interrupt controllers for AArch64
    mod platform_tests {
        use super::*;

        #[test_case]
        fn test_gic_availability() {
            use crate::drivers::pic::Gic;

            // Test that GIC can be instantiated (actual hardware interaction would need setup)
            // This test mainly verifies compilation and basic structure
        }
    }
}
