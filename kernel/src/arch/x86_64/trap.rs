//! x86_64 trap/interrupt handling
//!
//! Provides IDT setup and trap entry/exit handlers for kernel and user mode

pub mod kernel;
pub mod user;

use core::arch::{asm, naked_asm};
use core::sync::atomic::{AtomicU32, Ordering};

use crate::arch::x86_64::registers::IntRegisters;

/// IDT entry structure
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const fn new() -> Self {
        IdtEntry {
            offset_low: 0,
            selector: 0,
            ist: 0,
            type_attr: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    fn set_handler(&mut self, handler: u64, selector: u16, type_attr: u8) {
        self.offset_low = (handler & 0xFFFF) as u16;
        self.selector = selector;
        self.ist = 0;
        self.type_attr = type_attr;
        self.offset_mid = ((handler >> 16) & 0xFFFF) as u16;
        self.offset_high = ((handler >> 32) & 0xFFFFFFFF) as u32;
        self.reserved = 0;
    }
}

/// IDT pointer structure
#[repr(C, packed)]
struct IdtPointer {
    limit: u16,
    base: u64,
}

/// Maximum number of IDT entries
const IDT_ENTRIES: usize = 256;

/// Global IDT
static mut IDT: [IdtEntry; IDT_ENTRIES] = [IdtEntry::new(); IDT_ENTRIES];

/// Exception handler function type
type ExceptionHandler = extern "x86-interrupt" fn(&mut ExceptionStackFrame);

/// Exception stack frame pushed by x86_64 on exceptions
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ExceptionStackFrame {
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

/// External interrupt handler function type
type InterruptHandler = extern "x86-interrupt" fn(&mut ExceptionStackFrame);

// Global exception handlers (to be filled in by kernel initialization)
static mut EXCEPTION_HANDLERS: [Option<ExceptionHandler>; 32] = [None; 32];
static mut INTERRUPT_HANDLERS: [Option<InterruptHandler>; 256] = [None; 256];

/// Initialize the IDT with trap handlers
pub fn init_idt() {
    unsafe {
        // Set up exception handlers (0-31)
        for i in 0..32 {
            let handler = exception_handler as u64;
            IDT[i].set_handler(
                handler, 0x08, // Kernel code segment
                0x8E, // Present, DPL=0, Type=Interrupt Gate
            );
        }

        // Set up interrupt handlers (32-255)
        for i in 32..256 {
            let handler = interrupt_handler as u64;
            IDT[i].set_handler(
                handler, 0x08, // Kernel code segment
                0x8E, // Present, DPL=0, Type=Interrupt Gate
            );
        }

        // Load IDT
        let idt_ptr = IdtPointer {
            limit: (core::mem::size_of::<[IdtEntry; IDT_ENTRIES]>() - 1) as u16,
            base: &IDT as *const _ as u64,
        };

        asm!(
            "lidt [{}]",
            in(reg) &idt_ptr,
            options(nostack)
        );
    }
}

/// Common exception handler entry point
#[unsafe(naked)]
extern "x86-interrupt" fn exception_handler(_frame: &mut ExceptionStackFrame) {
    naked_asm!(
        // Save all general purpose registers
        "push rax",
        "push rbx",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push rbp",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",

        // Save error code from stack (pushed by CPU for some exceptions)
        "mov rsi, [rsp + 15*8]",

        // Call the actual handler
        "mov rdi, rsp",
        "call {handler}",

        // Restore registers
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rbp",
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop rcx",
        "pop rbx",
        "pop rax",

        // Clean up error code if present (6 bytes for error code + 8 bytes for RIP)
        "add rsp, 8",

        "iretq",
        handler = sym handle_exception,
    );
}

/// Common interrupt handler entry point
#[unsafe(naked)]
extern "x86-interrupt" fn interrupt_handler(_frame: &mut ExceptionStackFrame) {
    naked_asm!(
        "push rax",
        "push rbx",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push rbp",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",

        "call {handler}",

        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rbp",
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop rcx",
        "pop rbx",
        "pop rax",

        "iretq",
        handler = sym handle_interrupt,
    );
}

/// Actual exception handling logic
extern "C" fn handle_exception(_regs: &mut IntRegisters) {
    // In a real implementation, this would:
    // 1. Determine which exception occurred
    // 2. Call the appropriate handler
    // 3. Possibly terminate the task or fix the issue

    // For now, just halt
    loop {
        unsafe {
            asm!("hlt");
        }
    }
}

/// Actual interrupt handling logic
extern "C" fn handle_interrupt() {
    // Acknowledge the interrupt
    super::interrupt::eoi();

    // In a real implementation, this would:
    // 1. Determine which interrupt occurred
    // 2. Call the appropriate handler (timer, keyboard, etc.)
    // 3. Return from interrupt
}

pub mod exception {
    //! x86_64 exception types and handling

    /// Exception numbers (vectors)
    pub const DIVIDE_ERROR: u8 = 0;
    pub const DEBUG: u8 = 1;
    pub const NMI: u8 = 2;
    pub const BREAKPOINT: u8 = 3;
    pub const OVERFLOW: u8 = 4;
    pub const BOUND_RANGE: u8 = 5;
    pub const INVALID_OPCODE: u8 = 6;
    pub const DEVICE_NOT_AVAILABLE: u8 = 7;
    pub const DOUBLE_FAULT: u8 = 8;
    pub const INVALID_TSS: u8 = 10;
    pub const SEGMENT_NOT_PRESENT: u8 = 11;
    pub const STACK_SEGMENT: u8 = 12;
    pub const GENERAL_PROTECTION: u8 = 13;
    pub const PAGE_FAULT: u8 = 14;
    pub const X87_FPU_ERROR: u8 = 16;
    pub const ALIGNMENT_CHECK: u8 = 17;
    pub const MACHINE_CHECK: u8 = 18;
    pub const SIMD_FPU: u8 = 19;
}

/// A spinlock for serializing access to the kernel trap entry point
static KERNEL_TRAP_LOCK: AtomicU32 = AtomicU32::new(0);

/// Acquire the kernel trap lock
pub fn lock_kernel_trap() {
    while KERNEL_TRAP_LOCK
        .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

/// Release the kernel trap lock
pub fn unlock_kernel_trap() {
    KERNEL_TRAP_LOCK.store(0, Ordering::Release);
}

// Dummy symbols for trampoline linking
#[no_mangle]
#[used]
pub static _kernel_trap_entry: u64 = 0;

#[no_mangle]
#[used]
pub static _user_trap_entry: u64 = 0;
