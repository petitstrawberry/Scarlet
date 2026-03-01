use core::arch::asm;
use core::mem::transmute;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::early_println;
use crate::environment::MAX_NUM_CPUS;
use crate::environment::STACK_SIZE;
use crate::mem::KERNEL_STACK;
use crate::task::Task;

pub mod boot;
pub mod context;
pub mod earlycon;
pub mod fpu;
pub mod instruction;
pub mod interrupt;
pub mod kernel;
pub mod mmio;
pub mod registers;
pub mod switch;
pub mod timer;
pub mod trap;
pub mod vcpu;
pub mod vm;

pub use context::KernelContext;
pub use earlycon::*;
pub use registers::IntRegisters;

use crate::arch::vm::get_root_pagetable;
use crate::vm::vmem::MemoryArea;

pub type Arch = X86_64;

/// Per-CPU state for x86_64
///
/// Layout (offsets must match trampoline assembly):
///   0: scratch (temporary storage)
///   8: cpuid
///  16: kernel_stack
///  24: kernel_trap (user->kernel trap handler address)
///  32: kernel_cr3 (kernel CR3 for user->kernel entry)
///  40: trap_kind (0=exception,1=interrupt)
#[repr(C)]
#[derive(Debug, Clone)]
pub struct X86_64 {
    scratch: u64,
    pub cpuid: u64,
    kernel_stack: u64,
    kernel_trap: u64,
    kernel_cr3: u64,
    trap_kind: u64,
}

impl X86_64 {
    pub const fn new(cpu_id: usize) -> Self {
        X86_64 {
            scratch: 0,
            cpuid: cpu_id as u64,
            kernel_stack: 0,
            kernel_trap: 0,
            kernel_cr3: 0,
            trap_kind: 0,
        }
    }

    pub fn get_cpuid(&self) -> usize {
        self.cpuid as usize
    }

    pub fn get_trapframe_paddr(&self) -> usize {
        let addr = self.kernel_stack as usize - core::mem::size_of::<Trapframe>();
        addr
    }

    pub fn set_kernel_stack(&mut self, initial_top: u64) {
        self.kernel_stack = initial_top;
    }

    pub fn set_trap_handler(&mut self, addr: usize) {
        self.kernel_trap = addr as u64;
    }

    pub fn set_trap_kind(&mut self, kind: u64) {
        self.trap_kind = kind;
    }

    pub fn get_trap_kind(&self) -> u64 {
        self.trap_kind
    }

    pub fn set_next_address_space(&mut self, asid: u16) {
        let root_pagetable =
            get_root_pagetable(asid).expect("No root page table found for ASID (x86_64)");
        let cr3_val = root_pagetable.get_cr3_value();
        self.kernel_cr3 = cr3_val;
    }

    pub fn get_kernel_cr3(&self) -> u64 {
        self.kernel_cr3
    }

    pub fn set_scratch(&mut self, val: u64) {
        self.scratch = val;
    }

    pub fn get_scratch(&self) -> u64 {
        self.scratch
    }

    pub fn set_kernel_cr3(&mut self, val: u64) {
        self.kernel_cr3 = val;
    }

    pub fn as_paddr_cpu(&mut self) -> &mut X86_64 {
        unsafe { &mut CPUS[self.cpuid as usize] }
    }
}

#[unsafe(link_section = ".trampoline.data")]
static mut CPUS: [X86_64; MAX_NUM_CPUS] = [const { X86_64::new(0) }; MAX_NUM_CPUS];

pub fn init_arch(cpu_id: usize) {
    early_println!("[x86_64] CPU {}: Initializing core....", cpu_id);
    let x86_64: &mut X86_64 = unsafe { transmute(&CPUS[cpu_id] as *const _ as usize) };
    x86_64.cpuid = cpu_id as u64;
    trap_init(x86_64);
}

/// Returns the device memory areas for x86_64 QEMU platform.
/// These areas contain memory-mapped I/O devices and should be mapped
/// with device memory attributes (non-cacheable, no speculation).
pub fn get_device_memory_areas() -> alloc::vec::Vec<MemoryArea> {
    alloc::vec![
        // x86_64 QEMU: MMIO devices are typically below 4GB
        // PCI MMIO range: 0xC000_0000 - 0xFFFF_FFFF
        MemoryArea {
            start: 0x0000_0000,
            end: 0x000F_FFFF, // BIOS area
        },
        MemoryArea {
            start: 0xC000_0000,
            end: 0xFFFF_FFFF, // PCI MMIO
        },
    ]
}

#[repr(C, align(16))]
#[derive(Debug, Clone)]
pub struct Trapframe {
    pub regs: IntRegisters,
    pub rsp: u64,
    pub rip: u64,
    pub rflags: u64,
    /// User thread pointer (TLS) register.
    pub fsbase: u64,
    /// Reserved: currently used as a trampoline scratch (GS base).
    pub gsbase: u64,
    // exception information
    pub error_code: u64,
}

impl Trapframe {
    pub fn new() -> Self {
        Trapframe {
            regs: IntRegisters::new(),
            rsp: 0,
            rip: 0,
            rflags: 0x202, // IF flag set
            fsbase: 0,
            gsbase: 0,
            error_code: 0,
        }
    }

    pub fn get_syscall_number(&self) -> usize {
        self.regs.rax // RAX is used for syscall number in x86_64
    }

    pub fn set_syscall_number(&mut self, syscall_number: usize) {
        self.regs.rax = syscall_number;
    }

    pub fn get_return_value(&self) -> usize {
        self.regs.rax // RAX is used for return value in x86_64
    }

    pub fn set_return_value(&mut self, value: usize) {
        self.regs.rax = value;
    }

    pub fn get_arg(&self, index: usize) -> usize {
        // Arguments are passed in RDI, RSI, RDX, RCX, R8, R9 in x86_64
        match index {
            0 => self.regs.rdi,
            1 => self.regs.rsi,
            2 => self.regs.rdx,
            3 => self.regs.rcx,
            4 => self.regs.r8,
            5 => self.regs.r9,
            _ => 0, // TODO: Handle arguments on stack
        }
    }

    pub fn set_arg(&mut self, index: usize, value: usize) {
        match index {
            0 => self.regs.rdi = value,
            1 => self.regs.rsi = value,
            2 => self.regs.rdx = value,
            3 => self.regs.rcx = value,
            4 => self.regs.r8 = value,
            5 => self.regs.r9 = value,
            _ => {}
        }
    }

    pub fn get_current_pc(&self) -> u64 {
        self.rip
    }

    /// Increment the program counter (RIP) to the next instruction
    pub fn increment_pc_next(&mut self, _task: &Task) {
        // For syscall (int 0x80 or syscall instruction), RIP already points to next instruction
        // For exceptions, RIP points to the faulting instruction
        // This will be handled in the trap handler
    }
}

pub fn get_user_trapvector_paddr() -> usize {
    trap::user::_user_trap_entry as usize
}

pub fn get_kernel_trapvector_paddr() -> usize {
    trap::kernel::_kernel_trap_entry as usize
}

pub fn kernel_phys_memory_area(
    kernel_area: crate::vm::vmem::MemoryArea,
) -> crate::vm::vmem::MemoryArea {
    let kernel_phys_base = crate::arch::x86_64::boot::kernel_phys_base();
    let kernel_va_base = crate::arch::x86_64::vm::KERNEL_VA_BASE;
    let start = kernel_area.start - kernel_va_base + kernel_phys_base;
    let end = kernel_area.end - kernel_va_base + kernel_phys_base;
    crate::vm::vmem::MemoryArea::new(start, end)
}

pub fn get_dram_window_offset() -> usize {
    crate::arch::x86_64::boot::hhdm_offset()
}

pub fn get_kernel_trap_handler() -> usize {
    trap::kernel::arch_kernel_trap_handler as usize
}

pub fn get_user_trap_handler() -> usize {
    trap::user::arch_user_trap_handler as usize
}

#[allow(static_mut_refs)]
fn trap_init(x86_64: &mut X86_64) {
    crate::early_println!("[x86_64] trap_init: getting stack...");
    let trap_stack_start = unsafe { KERNEL_STACK.start() };
    let stack_size = STACK_SIZE;

    crate::early_println!("[x86_64] trap_init: calculating trap_stack...");
    let trap_stack = trap_stack_start + stack_size * (x86_64.cpuid + 1) as usize;
    x86_64.kernel_stack = trap_stack as u64;
    x86_64.kernel_trap = get_user_trap_handler() as u64;

    crate::early_println!("[x86_64] trap_init: enabling FSGSBASE...");
    unsafe {
        let mut cr4: u64;
        asm!("mov {}, cr4", out(reg) cr4);
        cr4 |= 1 << 16; // FSGSBASE
        asm!("mov cr4, {}", in(reg) cr4);
    }

    crate::early_println!("[x86_64] trap_init: setting GS base...");
    let scratch_addr = x86_64 as *const _ as usize;
    unsafe {
        asm!(
            "wrgsbase {0}",
            in(reg) scratch_addr,
        );
    }

    crate::early_println!("[x86_64] trap_init: enabling FPU...");
    fpu::set_user_fpu_enabled(false);

    crate::early_println!("[x86_64] trap_init: loading IDT...");
    trap::kernel::load_idt();
    crate::early_println!("[x86_64] trap_init: done");
}

/// x86_64-only: perform the very first transition into a runnable user task.
///
/// This avoids bootstrapping the first user entry via a timer IRQ.
pub fn first_switch_to_user(task: &mut Task) -> ! {
    // Prefer the high-VA kernel stack window if available.
    let kernel_sp = if let Some((_slot, base)) = task.get_kernel_stack_window_base() {
        (base + crate::environment::PAGE_SIZE + crate::environment::TASK_KERNEL_STACK_SIZE) as u64
    } else {
        panic!("Task has no kernel stack window");
    };

    crate::early_println!(
        "[x86_64] CPU {}: First switch to user task PID {} with kernel SP {:#x}",
        crate::arch::get_current_cpu_id(),
        task.get_id(),
        kernel_sp,
    );

    let cpu = crate::arch::get_cpu();
    cpu.set_kernel_stack(kernel_sp);
    cpu.set_trap_handler(get_user_trap_handler());
    cpu.set_next_address_space(task.vm_manager.get_asid());

    let task_ptr = task as *mut Task;
    unsafe {
        let trapframe = (*task_ptr).get_trapframe();
        (*task_ptr).vcpu.lock().switch(trapframe);

        crate::arch::configure_user_entry(
            trapframe,
            crate::arch::UserEntryOptions {
                irq_policy: crate::arch::UserReturnIrqPolicy::Enable,
            },
        );
    }

    let trap_exit_offset = crate::arch::x86_64::trap::user::_user_trap_exit as usize
        - crate::arch::x86_64::trap::user::_user_trap_entry as usize;
    let trampoline_base = crate::vm::get_trampoline_trap_vector();
    let trap_exit_addr = trampoline_base + trap_exit_offset;

    let cpu_id = get_current_cpu_id();
    set_arch(crate::vm::get_trampoline_arch(cpu_id));

    let trapframe_addr = kernel_sp as usize - core::mem::size_of::<Trapframe>();

    unsafe {
        crate::arch::x86_64::trap::user::x86_64_first_switch_to_user_naked(
            trapframe_addr,
            trap_exit_addr,
        )
    }
}

pub fn set_arch(addr: usize) {
    unsafe {
        asm!(
            "wrgsbase {0}",
            in(reg) addr,
        );
    }
}

pub fn enable_interrupt() {
    unsafe {
        asm!("sti", options(nostack));
    }
}

pub fn disable_interrupt() {
    unsafe {
        asm!("cli", options(nostack));
    }
}

pub fn get_cpu() -> &'static mut X86_64 {
    let gsbase: usize;
    unsafe {
        asm!(
            "rdgsbase {0}",
            out(reg) gsbase,
        );
    }
    unsafe { transmute(gsbase) }
}

/// Get current CPU core ID from Local APIC ID
pub fn get_current_cpu_id() -> usize {
    // For simplicity in initial implementation, return 0
    // In a full implementation, this would read the Local APIC ID
    0
}

pub fn set_next_mode(mode: vcpu::Mode) {
    let _ = mode;
    // x86_64 doesn't need explicit mode switching like RISC-V
}

/// Set trap vector (load IDT)
pub fn set_trapvector(_addr: usize) {
    trap::kernel::load_idt();
}

/// Memory barrier for device/MMIO (I/O) operations.
#[inline(always)]
pub fn io_mb() {
    unsafe {
        asm!("mfence", options(nostack));
    }
}

/// Read barrier for device/MMIO (I/O) operations.
#[inline(always)]
pub fn io_rmb() {
    unsafe {
        asm!("lfence", options(nostack));
    }
}

/// Write barrier for device/MMIO (I/O) operations.
#[inline(always)]
pub fn io_wmb() {
    unsafe {
        asm!("sfence", options(nostack));
    }
}

/// General memory barrier for normal memory operations.
#[inline(always)]
pub fn mb() {
    unsafe {
        asm!("mfence", options(nostack));
    }
}

/// Read memory barrier for normal memory operations.
#[inline(always)]
pub fn rmb() {
    unsafe {
        asm!("lfence", options(nostack));
    }
}

/// Write memory barrier for normal memory operations.
#[inline(always)]
pub fn wmb() {
    unsafe {
        asm!("sfence", options(nostack));
    }
}

/// Apply user-entry options for the upcoming `iretq`.
pub fn configure_user_entry(trapframe: &mut Trapframe, options: crate::arch::UserEntryOptions) {
    use crate::arch::UserReturnIrqPolicy;

    const RFLAGS_IF: u64 = 1 << 9;
    match options.irq_policy {
        UserReturnIrqPolicy::Inherit => {}
        UserReturnIrqPolicy::Enable => {
            trapframe.rflags |= RFLAGS_IF;
        }
        UserReturnIrqPolicy::Disable => {
            trapframe.rflags &= !RFLAGS_IF;
        }
    }

    // Configure FPU access for the next user return
    if crate::arch::user_fpu_enabled() {
        let cpu_id = crate::arch::get_current_cpu_id();
        if let Some(task) = crate::sched::scheduler::get_scheduler().get_current_task(cpu_id) {
            crate::arch::fpu::set_user_fpu_enabled(task.vcpu.lock().fpu_used);
        } else {
            crate::arch::fpu::set_user_fpu_enabled(false);
        }
    } else {
        crate::arch::fpu::set_user_fpu_enabled(false);
    }
}

pub fn shutdown() -> ! {
    early_println!("[x86_64] Shutdown requested");
    // Try to use ACPI to power off, or use QEMU debug port
    unsafe {
        // QEMU shutdown command via debug port
        asm!(
            "out dx, al",
            in("dx") 0x604u16, // QEMU debug port for shutdown
            in("al") 0x34u8,    // Shutdown command
        );
    }
    loop {
        unsafe {
            asm!("hlt");
        }
    }
}

pub fn shutdown_with_code(exit_code: u32) -> ! {
    early_println!("[x86_64] Shutdown with exit code {} requested", exit_code);
    shutdown()
}

pub fn reboot() -> ! {
    early_println!("[x86_64] Reboot requested");
    unsafe {
        // Create an invalid IDT pointer to cause triple fault
        let null_idt: u16 = 0;
        asm!(
            "lidt [{idt_ptr}]",
            "int3",
            idt_ptr = in(reg) &null_idt as *const u16,
            options(nostack)
        );
    }
    loop {
        unsafe {
            asm!("hlt");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test architecture-specific features for x86_64
    #[test_case]
    fn test_x86_64_specific_features() {
        use crate::arch::x86_64::vcpu::Mode;

        set_next_mode(Mode::Kernel);
        set_next_mode(Mode::User);

        let cpu_id = get_current_cpu_id();
        assert!(
            cpu_id < crate::environment::MAX_NUM_CPUS,
            "x86_64 CPU ID should be within valid range"
        );
    }
}
