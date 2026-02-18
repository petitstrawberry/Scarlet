use super::Mode;
use core::arch::asm;
use core::mem::transmute;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use instruction::sbi::sbi_system_reset;
use trap::kernel::_kernel_trap_entry;
use trap::kernel::arch_kernel_trap_handler;
use trap::user::_user_trap_entry;
use trap::user::arch_user_trap_handler;

use crate::arch::instruction::Instruction;
use crate::arch::vm::get_root_pagetable;
use crate::early_println;
use crate::environment::MAX_NUM_CPUS;
use crate::environment::STACK_SIZE;
use crate::mem::KERNEL_STACK;
use crate::task::Task;

pub mod boot;
pub mod context;
pub mod earlycon;
pub mod fdt;
pub mod fpu;
pub mod hv;
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

pub use earlycon::*;
pub use registers::IntRegisters;

use crate::vm::vmem::MemoryArea;

pub type Arch = Riscv64;

/// Per-hart ownership of the live Vector register file.
///
/// When a task that used the V extension is rescheduled on the same hart, we can
/// skip restoring vregs if it still owns the live state. This removes a very
/// expensive per-timeslice illegal-instruction trap for vector-heavy workloads.
const NO_VECTOR_OWNER: usize = usize::MAX;
static VECTOR_OWNER: [AtomicUsize; MAX_NUM_CPUS] =
    [const { AtomicUsize::new(NO_VECTOR_OWNER) }; MAX_NUM_CPUS];

/// Whether the live vector register file contains state that is newer than the
/// saved per-task context of `VECTOR_OWNER`.
///
/// This is needed because we sometimes keep vregs live across timeslices while
/// forcing sstatus.VS to Clean/Off to avoid mis-attributing Dirtiness to another
/// task.
static VECTOR_OWNER_DIRTY: [AtomicBool; MAX_NUM_CPUS] =
    [const { AtomicBool::new(false) }; MAX_NUM_CPUS];

#[inline]
pub(crate) fn get_vector_owner(cpu_id: usize) -> usize {
    VECTOR_OWNER[cpu_id].load(Ordering::Relaxed)
}

#[inline]
pub(crate) fn set_vector_owner(cpu_id: usize, owner: usize) {
    VECTOR_OWNER[cpu_id].store(owner, Ordering::Relaxed)
}

#[inline]
pub(crate) fn get_vector_owner_dirty(cpu_id: usize) -> bool {
    VECTOR_OWNER_DIRTY[cpu_id].load(Ordering::Relaxed)
}

#[inline]
pub(crate) fn set_vector_owner_dirty(cpu_id: usize, dirty: bool) {
    VECTOR_OWNER_DIRTY[cpu_id].store(dirty, Ordering::Relaxed)
}

/// Apply user-entry options for the upcoming `sret`.
///
/// This does not enable interrupts in the kernel immediately; it only controls the
/// sstatus.SPIE bit which is copied into SIE by the `sret` instruction.
pub fn configure_user_entry(_trapframe: &mut Trapframe, options: crate::arch::UserEntryOptions) {
    use crate::arch::UserReturnIrqPolicy;

    // Reflect into sstatus.SPIE for the next `sret`.
    const SPIE: usize = 1 << 5;
    match options.irq_policy {
        UserReturnIrqPolicy::Inherit => {}
        UserReturnIrqPolicy::Enable => unsafe {
            let mut sstatus: usize;
            asm!("csrr {0}, sstatus", out(reg) sstatus);
            sstatus |= SPIE;
            asm!("csrw sstatus, {0}", in(reg) sstatus);
        },
        UserReturnIrqPolicy::Disable => unsafe {
            let mut sstatus: usize;
            asm!("csrr {0}, sstatus", out(reg) sstatus);
            sstatus &= !SPIE;
            asm!("csrw sstatus, {0}", in(reg) sstatus);
        },
    }

    // Lazy FPU/Vector: trap on first use.
    // If the task has never used FPU/Vector, keep them disabled for user mode.
    // When an illegal-instruction trap is raised by a FP/Vector instruction,
    // the trap handler will mark the task as used and re-enable the extension.
    let cpu_id = crate::arch::get_cpu().get_cpuid();
    let (current_task_ptr, current_task_id, owner_task_ptr, owner_id, owner_dirty) = {
        let sched = crate::sched::scheduler::get_scheduler();
        let Some(current_task_id) = sched.get_current_task_id(cpu_id) else {
            return;
        };
        let Some(current_task_ptr) = sched
            .get_task_by_id(current_task_id)
            .map(|t| t as *mut Task)
        else {
            return;
        };

        let owner_id = get_vector_owner(cpu_id);
        let owner_dirty = get_vector_owner_dirty(cpu_id);
        let owner_task_ptr =
            if owner_dirty && owner_id != NO_VECTOR_OWNER && owner_id != current_task_id {
                sched.get_task_by_id(owner_id).map(|t| t as *mut Task)
            } else {
                None
            };

        (
            current_task_ptr,
            current_task_id,
            owner_task_ptr,
            owner_id,
            owner_dirty,
        )
    };

    let task = unsafe { &mut *current_task_ptr };

    if !crate::arch::user_fpu_enabled() || !task.vcpu.lock().fpu_used {
        crate::arch::riscv64::fpu::disable_fpu();
    }

    if !crate::arch::user_vector_enabled() || !task.vcpu.lock().vector_used {
        crate::arch::riscv64::fpu::disable_vector();
        return;
    }

    // Ensure the task has a backing context (allocated lazily).
    if task.vcpu.lock().vector.is_none() {
        task.vcpu.lock().vector = Some(alloc::boxed::Box::new(
            crate::arch::riscv64::fpu::VectorContext::new(),
        ));
    }

    // If another task currently owns the live vregs and its live state hasn't
    // been saved, save it now before we clobber vregs with our restore.
    if owner_dirty && owner_id != NO_VECTOR_OWNER && owner_id != current_task_id {
        if let Some(owner_ptr) = owner_task_ptr {
            let owner_task = unsafe { &mut *owner_ptr };
            if owner_task.vcpu.lock().vector.is_none() {
                owner_task.vcpu.lock().vector = Some(alloc::boxed::Box::new(
                    crate::arch::riscv64::fpu::VectorContext::new(),
                ));
                owner_task.vcpu.lock().vector_used = true;
            }
            crate::arch::riscv64::fpu::enable_vector();
            unsafe { owner_task.vcpu.lock().vector.as_mut().unwrap().save() };
            crate::arch::riscv64::fpu::mark_vector_clean();
            set_vector_owner_dirty(cpu_id, false);
        } else {
            // Owner task disappeared; drop the dirty flag to avoid repeated work.
            set_vector_owner_dirty(cpu_id, false);
        }
    }

    // Vector hot-path:
    // - Restore only when ownership changed on this hart.
    // - Otherwise just re-enable access without a full restore.
    if owner_id != current_task_id {
        crate::arch::riscv64::fpu::enable_vector();
        unsafe { task.vcpu.lock().vector.as_ref().unwrap().restore() };
        crate::arch::riscv64::fpu::mark_vector_clean();
        set_vector_owner(cpu_id, current_task_id);
        set_vector_owner_dirty(cpu_id, false);
    } else if !crate::arch::riscv64::fpu::is_vector_enabled() {
        crate::arch::riscv64::fpu::enable_vector();
        crate::arch::riscv64::fpu::mark_vector_clean();
        // Preserve owner-dirty: if we kept live unsaved state, it stays dirty.
    }
}

/// RISC-V: perform the very first transition into a runnable user task.
///
/// This avoids bootstrapping the first user entry via a timer IRQ.
/// The function prepares trampoline-visible per-CPU state and then
/// jumps to the trampoline exit path which performs `sret` into user mode.
pub fn first_switch_to_user(task: &mut Task) -> ! {
    // Prefer the high-VA kernel stack window if available.
    let kernel_sp = if let Some((_slot, base)) = task.get_kernel_stack_window_base() {
        (base + crate::environment::PAGE_SIZE + crate::environment::TASK_KERNEL_STACK_SIZE) as u64
    } else {
        panic!("Task has no kernel stack window");
    };

    crate::early_println!(
        "[riscv64] CPU {}: First switch to user task PID {} with kernel SP {:#x}",
        crate::arch::get_cpu().get_cpuid(),
        task.get_id(),
        kernel_sp,
    );

    // Switch sscratch to the trampoline-visible per-CPU struct.
    let cpu_id = crate::arch::get_cpu().get_cpuid();
    set_arch(crate::vm::get_trampoline_arch(cpu_id));

    // Update trampoline-visible CPU struct.
    let cpu = crate::arch::get_cpu();
    cpu.set_kernel_stack(kernel_sp);
    cpu.set_trap_handler(get_user_trap_handler());
    cpu.set_next_address_space(task.vm_manager.get_asid());

    // Populate the trapframe from the task VCPU state.
    let task_ptr = task as *mut Task;
    unsafe {
        let trapframe = (*task_ptr).get_trapframe();
        (*task_ptr).vcpu.lock().switch(trapframe);
    }

    // Ensure the next return is to the correct privilege mode.
    set_next_mode(task.vcpu.lock().get_mode());

    // Program trampoline trap vector right before the jump.
    set_trapvector(crate::vm::get_trampoline_trap_vector());

    // Final transition via trampoline exit path.
    crate::arch::riscv64::trap::user::arch_switch_to_user(task.get_trapframe())
}

/// Returns the device memory areas for RISC-V QEMU virt platform.
/// These areas contain memory-mapped I/O devices and should be mapped
/// with device memory attributes (non-cacheable, no speculation).
pub fn get_device_memory_areas() -> alloc::vec::Vec<MemoryArea> {
    alloc::vec![
        // QEMU virt: MMIO devices are in the low 2GB
        MemoryArea {
            start: 0x0000_0000,
            end: 0x7fff_ffff,
        },
    ]
}

#[unsafe(link_section = ".trampoline.data")]
static mut CPUS: [Riscv64; MAX_NUM_CPUS] = [const { Riscv64::new(0) }; MAX_NUM_CPUS];

#[repr(align(4))]
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Riscv64 {
    scratch: u64,             // offeset: 0
    pub hartid: u64,          // offset: 8
    satp: u64,                // offset: 16
    kernel_stack: u64,        // offset: 24
    kernel_trap: u64,         // offset: 32
    guest_trapframe_ptr: u64, // offset: 40
}

impl Riscv64 {
    pub const fn new(cpu_id: usize) -> Self {
        Riscv64 {
            scratch: 0,
            hartid: cpu_id as u64,
            kernel_stack: 0,
            kernel_trap: 0,
            satp: 0,
            guest_trapframe_ptr: 0,
        }
    }

    pub fn get_cpuid(&self) -> usize {
        self.hartid as usize
    }

    pub fn get_trapframe_paddr(&self) -> usize {
        /* Get pointer of the trapframe, which is located at the top of the kernel stack */
        let addr = self.kernel_stack as usize - core::mem::size_of::<Trapframe>();
        addr
    }

    pub fn get_kernel_stack(&self) -> u64 {
        self.kernel_stack
    }

    pub fn set_kernel_stack(&mut self, initial_top: u64) {
        self.kernel_stack = initial_top;
    }

    // pub fn get_satp(&self) -> u64 {
    //     self.satp
    // }

    // pub fn set_satp(&mut self, val: u64) {
    //     self.satp = val;
    // }

    pub fn set_trap_handler(&mut self, addr: usize) {
        self.kernel_trap = addr as u64;
    }

    pub fn set_next_address_space(&mut self, asid: u16) {
        let root_pagetable = get_root_pagetable(asid).expect("No root page table found for ASID");

        let satp = root_pagetable.get_val_for_satp(asid);
        self.satp = satp as u64;
    }

    pub fn as_paddr_cpu(&mut self) -> &mut Riscv64 {
        unsafe { &mut CPUS[self.hartid as usize] }
    }
}

pub struct ArchCpuState {
    kernel_stack: u64,
    trap_handler: u64,
    satp: u64,
    guest_trapframe_ptr: u64,
}

impl ArchCpuState {
    pub fn save(cpu: &Riscv64) -> Self {
        ArchCpuState {
            kernel_stack: cpu.kernel_stack,
            trap_handler: cpu.kernel_trap,
            satp: cpu.satp,
            guest_trapframe_ptr: cpu.guest_trapframe_ptr,
        }
    }

    pub fn restore(&self, cpu: &mut Riscv64) {
        cpu.kernel_stack = self.kernel_stack;
        cpu.kernel_trap = self.trap_handler;
        cpu.satp = self.satp;
        cpu.guest_trapframe_ptr = self.guest_trapframe_ptr;
    }
}

#[repr(C, align(16))]
#[derive(Debug, Clone)]
pub struct Trapframe {
    pub regs: IntRegisters,
    pub epc: u64,
    pub _padding: u64,
}

impl Trapframe {
    pub fn new() -> Self {
        Trapframe {
            regs: IntRegisters::new(),
            epc: 0,
            _padding: 0xdeadbeefdeadbeef,
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

    pub fn get_current_pc(&self) -> u64 {
        self.epc
    }

    /// Increment the program counter (epc) to the next instruction
    /// This is typically used after handling a trap or syscall to continue execution.
    ///
    pub fn increment_pc_next(&mut self, task: &Task) {
        let instruction =
            Instruction::fetch(task.vm_manager.translate_vaddr(self.epc as usize).unwrap());
        let len = instruction.len();
        if len == 0 {
            debug_assert!(len > 0, "Invalid instruction length: {}", len);
            early_println!(
                "Warning: Invalid instruction length encountered. Defaulting to 4 bytes."
            );
            self.epc += 4; // Default to 4 bytes for invalid instruction length
        } else {
            self.epc += len as u64;
        }
    }
}

pub fn get_user_trapvector_paddr() -> usize {
    _user_trap_entry as usize
}

pub fn get_guest_trapvector_paddr() -> usize {
    trap::user::_guest_trap_entry as usize
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
    let stack_size = STACK_SIZE;

    let trap_stack = trap_stack_start + stack_size * (riscv.hartid + 1) as usize;
    riscv.kernel_stack = trap_stack as u64;
    riscv.kernel_trap = arch_kernel_trap_handler as u64;
    let scratch_addr = riscv as *const _ as usize;

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

    // Enable FPU for user-space and kernel access
    fpu::enable_fpu();

    // Enable Vector extension for user-space and kernel access
    fpu::enable_vector();

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

pub fn get_trapvector() -> usize {
    let stvec: usize;
    unsafe {
        asm!("csrr {}, stvec", out(reg) stvec);
    }
    stvec
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
        asm!(
            "
        csrsi sstatus, 0x2
        "
        );
    }
}

pub fn disable_interrupt() {
    unsafe {
        asm!(
            "
        csrci sstatus, 0x2
        "
        );
    }
}

/// Full memory barrier for normal memory (RAM).
///
/// This orders previous reads/writes before subsequent reads/writes.
/// For device/MMIO ordering, prefer [`io_mb`].
#[inline(always)]
pub fn mb() {
    unsafe {
        asm!("fence rw, rw", options(nostack));
    }
}

/// Read memory barrier for normal memory (RAM).
#[inline(always)]
pub fn rmb() {
    unsafe {
        asm!("fence r, r", options(nostack));
    }
}

/// Write memory barrier for normal memory (RAM).
#[inline(always)]
pub fn wmb() {
    unsafe {
        asm!("fence w, w", options(nostack));
    }
}

/// Full barrier for device/MMIO (I/O) operations.
///
/// RISC-V requires an explicit I/O fence to order device register accesses.
#[inline(always)]
pub fn io_mb() {
    unsafe {
        asm!("fence iorw, iorw", options(nostack));
    }
}

/// Read barrier for device/MMIO (I/O) operations.
#[inline(always)]
pub fn io_rmb() {
    unsafe {
        asm!("fence ir, ir", options(nostack));
    }
}

/// Write barrier for device/MMIO (I/O) operations.
#[inline(always)]
pub fn io_wmb() {
    unsafe {
        asm!("fence ow, ow", options(nostack));
    }
}

/// Backward-compatible alias for a full device/MMIO barrier.
#[inline(always)]
pub fn mmio_fence() {
    io_mb()
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
        Mode::User => unsafe {
            let mut sstatus: usize;
            asm!(
                "csrr {sstatus}, sstatus",
                sstatus = out(reg) sstatus,
            );
            sstatus &= !(1 << 8);
            asm!(
                "csrw sstatus, {sstatus}",
                sstatus = in(reg) sstatus,
            );
            asm!("csrc hstatus, {0}", in(reg) (1u64 << 7));
        },
        Mode::Kernel => unsafe {
            let mut sstatus: usize;
            asm!(
                "csrr {sstatus}, sstatus",
                sstatus = out(reg) sstatus,
            );
            sstatus |= 1 << 8;
            asm!(
                "csrw sstatus, {sstatus}",
                sstatus = in(reg) sstatus,
            );
            asm!("csrc hstatus, {0}", in(reg) (1u64 << 7));
        },
        Mode::GuestUser => unsafe {
            let mut sstatus: usize;
            asm!(
                "csrr {sstatus}, sstatus",
                sstatus = out(reg) sstatus,
            );
            sstatus &= !(1 << 8);
            asm!(
                "csrw sstatus, {sstatus}",
                sstatus = in(reg) sstatus,
            );
            asm!("csrs hstatus, {0}", in(reg) (1u64 << 7));
        },
        Mode::GuestKernel => unsafe {
            let mut sstatus: usize;
            asm!(
                "csrr {sstatus}, sstatus",
                sstatus = out(reg) sstatus,
            );
            sstatus |= 1 << 8;
            asm!(
                "csrw sstatus, {sstatus}",
                sstatus = in(reg) sstatus,
            );
            asm!("csrs hstatus, {0}", in(reg) (1u64 << 7));
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
