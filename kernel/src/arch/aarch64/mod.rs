use core::arch::asm;
use core::mem::transmute;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::early_println;
use crate::environment::MAX_NUM_CPUS;
use crate::environment::STACK_SIZE;
use crate::mem::KERNEL_STACK;
use crate::task::Task;

pub mod boot;
pub use boot::limine::{is_hv_available, is_vhe_enabled};
pub mod context;
pub mod earlycon;
pub mod fpu;
#[cfg(feature = "hypervisor")]
pub mod hv;
pub mod instruction;
pub mod interrupt;
pub mod kernel;
pub mod lsm;
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
static mut CPUS: [Aarch64; MAX_NUM_CPUS] = [const { Aarch64::new(0) }; MAX_NUM_CPUS];

pub fn init_arch(cpu_id: usize) {
    early_println!("[aarch64] CPU {}: Initializing core....", cpu_id);
    // Get raw Aarch64 struct
    let aarch64: &mut Aarch64 = unsafe { transmute(&CPUS[cpu_id] as *const _ as usize) };
    aarch64.cpuid = cpu_id as u64;

    trap_init(aarch64);
}

pub fn init_ap_cpu(cpu_id: usize) {
    early_println!("[aarch64] CPU {}: Initializing core....", cpu_id);
    // Get raw Aarch64 struct
    let aarch64: &mut Aarch64 = unsafe { transmute(&CPUS[cpu_id] as *const _ as usize) };
    aarch64.cpuid = cpu_id as u64;

    unsafe {
        asm!(
            "mrs {0}, ttbr0_el1",
            out(reg) aarch64.ttbr0,
        );
    }

    trap_init(aarch64);
}

/// AArch64-only: perform the very first transition into a runnable user task.
///
/// This avoids bootstrapping the first user entry via a timer IRQ (which makes the
/// initial control-flow sensitive to VBAR timing and can be unstable during bring-up).
///
/// The function:
/// - Chooses the task-provided kernel stack (SP_EL1) for the upcoming EL0->EL1 traps.
/// - Programs per-CPU trampoline-visible state (kernel stack top, trap handler, TTBR0).
/// - Performs a direct transition via the trampoline exit path.
pub fn first_switch_to_user(task: &Task) -> ! {
    // Prefer the high-VA kernel stack window if available.
    let kernel_sp = if let Some((_slot, base)) = task.get_kernel_stack_window_base() {
        (base + crate::environment::PAGE_SIZE + crate::environment::TASK_KERNEL_STACK_SIZE) as u64
    } else {
        panic!("Task has no kernel stack window");
    };

    // Update trampoline-visible CPU struct.
    let cpu = crate::arch::get_cpu();
    crate::println!(
        "[aarch64] CPU {}: First switch to user task PID {} with kernel SP {:#x}",
        cpu.get_cpuid(),
        task.get_id(),
        kernel_sp,
    );
    cpu.set_kernel_stack(kernel_sp);
    cpu.set_trap_handler(get_user_trap_handler());
    cpu.set_next_address_space(task.vm_manager.get_asid());

    let task_mode = task.vcpu.lock().get_mode();
    let trapframe = task.get_trapframe();
    task.vcpu.lock().switch(trapframe);

    trapframe.spsr = target_spsr_for_mode(task_mode);

    crate::arch::configure_user_entry(
        trapframe,
        crate::arch::UserEntryOptions {
            irq_policy: crate::arch::UserReturnIrqPolicy::Enable,
        },
    );

    // Compute trampoline exit target.
    let trap_exit_offset = (crate::arch::aarch64::trap::user::_switch_to_user as usize)
        .wrapping_sub(crate::arch::aarch64::trap::user::_user_trap_entry as usize);
    let trampoline_base = crate::vm::get_trampoline_trap_vector();
    let trap_exit_addr = trampoline_base.wrapping_add(trap_exit_offset);

    // Program per-CPU arch pointer and the target trap vector right before the jump.
    let cpu_id = cpu.get_cpuid();
    set_arch(crate::vm::get_trampoline_arch(cpu_id));
    if is_privileged_return_mode(task.get_trapframe().spsr) {
        set_trapvector(get_kernel_trapvector_paddr());
    } else {
        set_trapvector(trampoline_base);
    }

    let trapframe_addr = (kernel_sp as usize).wrapping_sub(core::mem::size_of::<Trapframe>());

    // Final transition must not touch the stack after switching SP.
    unsafe {
        crate::arch::aarch64::trap::user::aarch64_first_switch_to_user_naked(
            trapframe_addr,
            trap_exit_addr,
        )
    }
}

/// Per-CPU state for AArch64
///
/// Layout (offsets must match trampoline assembly):
///   0: scratch (temporary storage)
///   8: cpuid
///  16: ttbr0 (user TTBR saved on entry)
///  24: kernel_stack
///  32: kernel_trap (EL0->EL1 trap handler address; trampoline jumps here)
///  40: kernel_ttbr0 (kernel TTBR for EL0->EL1 entry)
///  48: trap_kind (0=sync,1=irq,2=fiq,3=serror)
#[repr(C)]
#[derive(Debug, Clone)]
pub struct Aarch64 {
    scratch: u64,      // offset: 0
    pub cpuid: u64,    // offset: 8
    ttbr0: u64,        // offset: 16
    kernel_stack: u64, // offset: 24　// Deprecated
    kernel_trap: u64,  // offset: 32
    kernel_ttbr0: u64, // offset: 40
    trap_kind: u64,    // offset: 48
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
            trap_kind: 0,
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
        // This setter is used by the common scheduler code.
        // On AArch64, this should configure the handler used for EL0->EL1 traps.
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
            get_root_pagetable(asid).expect("No root page table found for ASID (aarch64)");
        let ttbr_val_raw = root_pagetable.get_val_for_ttbr(asid);
        self.ttbr0 = ttbr_val_raw;

        // Clean this CPU struct from D-cache so that the trampoline assembly
        // (which may read via a different VA alias) sees the updated ttbr0.
        crate::arch::aarch64::clean_dcache_to_poc_range(
            self as *const _ as usize,
            core::mem::size_of::<Aarch64>(),
        );
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

        // The user trampoline may observe this CPU struct through a different
        // VA alias, so push the update to PoC before returning to EL0.
        crate::arch::aarch64::clean_dcache_to_poc_range(
            self as *const _ as usize,
            core::mem::size_of::<Aarch64>(),
        );
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
    pub sp: u64,
    pub elr: u64,  // ELR_EL1
    pub spsr: u64, // SPSR_EL1
    /// User thread pointer (TLS) register.
    pub tpidr_el0: u64,
    /// Reserved: currently used as a trampoline scratch (TPIDRRO_EL0).
    pub tpidrro_el0: u64,
    // exception information
    pub esr_el1: u64,
    pub _padding: u64, // must be 304 bytes to match trampoline's sub sp, sp, #304
}

impl Trapframe {
    pub fn new() -> Self {
        Trapframe {
            regs: IntRegisters::new(),
            sp: 0,
            elr: 0,
            spsr: 0,
            tpidr_el0: 0,
            tpidrro_el0: 0,
            esr_el1: 0,
            _padding: 0,
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

    pub fn get_current_pc(&self) -> u64 {
        let esr = self.esr_el1;
        let ec = trap::exception::ExceptionClass::from(esr);
        match ec {
            trap::exception::ExceptionClass::SvcAarch64 => self.elr - 4,
            _ => self.elr,
        }
    }

    pub fn set_pc(&mut self, pc: u64) {
        self.elr = pc;
    }

    /// Increment the program counter (epc) to the next instruction
    /// This is typically used after handling a trap or syscall to continue execution.
    ///
    pub fn increment_pc_next(&mut self, _task: &Task) {
        // NOTE:
        // For AArch64 `svc` (and most synchronous exceptions in this kernel path),
        // ELR_EL1 already contains the address of the *next* instruction.
        // Incrementing here would skip the instruction immediately following `svc`
        // in userspace, breaking syscall wrappers that spill x0 to memory.
    }
}

pub fn get_user_trapvector_paddr() -> usize {
    trap::user::_user_trap_entry as usize
}

pub fn get_guest_trapvector_paddr() -> usize {
    #[cfg(feature = "hypervisor")]
    {
        return hv::switch::guest_exit_vector_base();
    }

    #[allow(unreachable_code)]
    0
}

pub fn get_kernel_trapvector_paddr() -> usize {
    trap::kernel::_kernel_trap_entry as usize
}

pub fn get_kernel_trap_handler() -> usize {
    trap::kernel::arch_kernel_trap_handler as usize
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
    // Trampoline (EL0->EL1) jumps to this handler via CPU struct.
    aarch64.kernel_trap = get_user_trap_handler() as u64;

    let scratch_addr = aarch64 as *const _ as usize;

    // Set up thread pointer registers to point to our aarch64 struct
    unsafe {
        asm!(
            "msr tpidr_el1, {0}",
            in(reg) scratch_addr,
        );
    }

    // Allow EL1 to use FP/SIMD (for save/restore), but trap EL0 by default.
    // Tasks that actually use FP/SIMD will enable EL0 access on-demand.
    fpu::set_user_fpu_enabled(false);

    // Default to kernel vector while executing in EL1.
    set_trapvector(get_kernel_trapvector_paddr());
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

pub fn get_trapvector() -> usize {
    let vbar: usize;
    unsafe {
        asm!(
            "mrs {0}, vbar_el1",
            out(reg) vbar,
            options(nostack)
        );
    }
    vbar
}

/// Apply user-entry options for the upcoming `eret`.
///
/// This only affects the user PSTATE restored from `trapframe.spsr`.
pub fn configure_user_entry(trapframe: &mut Trapframe, options: crate::arch::UserEntryOptions) {
    use crate::arch::UserReturnIrqPolicy;

    const DAIF_I: u64 = 1 << 7;
    const DAIF_F: u64 = 1 << 6;
    const DAIF_MASK: u64 = DAIF_I | DAIF_F;
    const SPSR_MODE_MASK: u64 = 0xF;

    let el = if let Some(task) = crate::sched::scheduler::current_task(get_cpu().get_cpuid()) {
        target_spsr_for_mode(task.vcpu.lock().get_mode())
    } else {
        target_spsr_for_mode(crate::arch::Mode::User)
    };

    let daif = match options.irq_policy {
        UserReturnIrqPolicy::Inherit => trapframe.spsr & DAIF_MASK,
        UserReturnIrqPolicy::Enable => 0,
        UserReturnIrqPolicy::Disable => DAIF_MASK,
    };
    trapframe.spsr = el | daif | (trapframe.spsr & !(SPSR_MODE_MASK | DAIF_MASK));

    if crate::arch::user_fpu_enabled() {
        let cpu_id = get_cpu().get_cpuid();
        if let Some(task) = crate::sched::scheduler::current_task(cpu_id) {
            crate::arch::fpu::set_user_fpu_enabled(task.vcpu.lock().fpu_used);
        } else {
            crate::arch::fpu::set_user_fpu_enabled(false);
        }
    } else {
        crate::arch::fpu::set_user_fpu_enabled(false);
    }
}

pub fn set_arch(addr: usize) {
    // Store the trampoline-visible Aarch64 pointer in TPIDR_EL1.
    unsafe {
        asm!(
            "msr tpidr_el1, {0}",
            in(reg) addr,
        );
    }
}

pub fn enable_interrupt() {
    // Keep interrupts globally masked until the timer has started.
    if !interrupts_allowed() {
        unsafe {
            asm!("msr daifset, #0xf", options(nostack));
        }
        return;
    }
    unsafe {
        asm!("msr daifclr, #0xf", options(nostack));
    }
}

pub fn disable_interrupt() {
    unsafe {
        asm!("msr daifset, #0xf", options(nostack));
    }
}

pub fn send_reschedule_ipi(target_cpu: usize) {
    use crate::interrupt::controllers::LocalInterruptType;
    let _ = crate::interrupt::InterruptManager::global()
        .send_ipi(target_cpu as u32, LocalInterruptType::Software);
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

    // Kernel context always has access to this mapping.
    return unsafe { transmute(tpidr_el1) };
}

pub fn set_next_mode(mode: crate::arch::Mode) {
    let spsr = target_spsr_for_mode(mode);
    // SAFETY: writing SPSR_EL1 only affects the exception-return EL.
    unsafe {
        core::arch::asm!("msr spsr_el1, {0}", in(reg) spsr);
    }
}

pub fn is_privileged_return_mode(spsr: u64) -> bool {
    matches!(spsr & 0xF, 0x5 | 0x9)
}

fn target_spsr_for_mode(mode: crate::arch::Mode) -> u64 {
    const SPSR_EL0T: u64 = 0x0;
    const SPSR_EL1H: u64 = 0x5;
    const SPSR_EL2H: u64 = 0x9;

    match mode {
        crate::arch::Mode::Kernel if is_vhe_enabled() => SPSR_EL2H,
        crate::arch::Mode::Kernel | crate::arch::Mode::GuestKernel => SPSR_EL1H,
        _ => SPSR_EL0T,
    }
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

#[inline(always)]
fn cache_line_bytes_dcache() -> usize {
    // CTR_EL0.DminLine is log2(number of 32-bit words in the smallest D-cache line).
    // LineBytes = 4 << DminLine.
    let ctr: u64;
    unsafe {
        asm!("mrs {0}, ctr_el0", out(reg) ctr, options(nostack));
    }
    let dminline = ((ctr >> 16) & 0xf) as usize;
    4usize << dminline
}

/// Clean D-cache to Point of Coherency (PoC) for the given virtual address range.
///
/// This is primarily useful for ensuring page-table updates become visible to the
/// hardware table walker during bring-up/debugging.
#[inline(always)]
pub fn clean_dcache_to_poc_range(start_vaddr: usize, len: usize) {
    if len == 0 {
        return;
    }

    let line = cache_line_bytes_dcache();
    let mut addr = start_vaddr & !(line - 1);
    let end = start_vaddr.saturating_add(len);

    unsafe {
        while addr < end {
            asm!("dc cvac, {0}", in(reg) addr, options(nostack));
            addr = addr.saturating_add(line);
        }
        // Ensure the clean completes before subsequent operations (e.g. TLBI).
        asm!("dsb sy", options(nostack));
    }
}

/// Clean and invalidate D-cache to Point of Coherency (PoC) for a virtual range.
///
/// Use this before reading memory that may have been modified through another
/// virtual alias, such as a userspace mapping of a kernel-owned shared page.
#[inline(always)]
pub fn clean_invalidate_dcache_to_poc_range(start_vaddr: usize, len: usize) {
    if len == 0 {
        return;
    }

    let line = cache_line_bytes_dcache();
    let mut addr = start_vaddr & !(line - 1);
    let end = start_vaddr.saturating_add(len);

    unsafe {
        while addr < end {
            asm!("dc civac, {0}", in(reg) addr, options(nostack));
            addr = addr.saturating_add(line);
        }
        asm!("dsb sy", options(nostack));
    }
}

/// Clean D-cache to Point of Unification (PoU) for the given virtual address range.
///
/// Required when code is written via the data cache and will later be executed
/// (self-modifying code / JIT / loading user text). The canonical sequence is:
/// - clean D-cache to PoU over the modified range
/// - `dsb ishst`
/// - invalidate I-cache (`ic ivau` per-line or `ic iallu`)
/// - `dsb ish; isb`
#[inline(always)]
pub fn clean_dcache_to_pou_range(start_vaddr: usize, len: usize) {
    if len == 0 {
        return;
    }

    let line = cache_line_bytes_dcache();
    let mut addr = start_vaddr & !(line - 1);
    let end = start_vaddr.saturating_add(len);

    unsafe {
        while addr < end {
            asm!("dc cvau, {0}", in(reg) addr, options(nostack));
            addr = addr.saturating_add(line);
        }
        // Ensure the clean completes before subsequent I-cache invalidation.
        asm!("dsb ishst", options(nostack));
    }
}

pub fn shutdown() -> ! {
    // Prefer PSCI to power off the machine. QEMU virt advertises PSCI and will
    // terminate the emulator on SYSTEM_OFF.
    const PSCI_SYSTEM_OFF: u64 = 0x8400_0008;

    #[inline(always)]
    unsafe fn psci_hvc(fid: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
        let ret: u64;
        unsafe {
            asm!(
                "hvc #0",
                inout("x0") fid => ret,
                in("x1") arg0,
                in("x2") arg1,
                in("x3") arg2,
                options(nostack)
            );
        }
        ret
    }

    early_println!("[aarch64] Shutdown requested (PSCI SYSTEM_OFF)");
    unsafe {
        let _ = psci_hvc(PSCI_SYSTEM_OFF, 0, 0, 0);
    }

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
    const PSCI_SYSTEM_RESET: u64 = 0x8400_0009;

    #[inline(always)]
    unsafe fn psci_hvc(fid: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
        let ret: u64;
        unsafe {
            asm!(
                "hvc #0",
                inout("x0") fid => ret,
                in("x1") arg0,
                in("x2") arg1,
                in("x3") arg2,
                options(nostack)
            );
        }
        ret
    }

    early_println!("[aarch64] Reboot requested (PSCI SYSTEM_RESET)");
    unsafe {
        let _ = psci_hvc(PSCI_SYSTEM_RESET, 0, 0, 0);
    }

    early_println!("[aarch64] Reboot requested - entering infinite loop");
    loop {
        unsafe {
            asm!("wfi");
        }
    }
}

pub struct ArchCpuState {
    kernel_stack: u64,
    kernel_trap: u64,
    ttbr0: u64,
}

impl ArchCpuState {
    pub fn save(cpu: &Aarch64) -> Self {
        ArchCpuState {
            kernel_stack: cpu.kernel_stack,
            kernel_trap: cpu.kernel_trap,
            ttbr0: cpu.ttbr0,
        }
    }

    pub fn restore(&self, cpu: &mut Aarch64) {
        cpu.kernel_stack = self.kernel_stack;
        cpu.kernel_trap = self.kernel_trap;
        cpu.ttbr0 = self.ttbr0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test architecture-specific features for AArch64
    #[test_case]
    fn test_aarch64_specific_features() {
        use crate::arch::Mode;

        // Test mode switching
        set_next_mode(Mode::Kernel);
        set_next_mode(Mode::User);

        // Test AArch64-specific CPU ID retrieval
        let cpu_id = get_cpu().get_cpuid();
        assert!(
            cpu_id < crate::environment::MAX_NUM_CPUS,
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
