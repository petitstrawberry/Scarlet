//! x86_64 trap/interrupt handling
//!
//! Provides IDT setup and trap entry/exit handlers for kernel and user mode

pub mod kernel;
pub mod user;

use core::arch::{asm, naked_asm};
use core::sync::atomic::{AtomicU32, Ordering};

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
        IDT[0].set_handler(exc_divide_error as u64, 0x08, 0x8E);
        IDT[1].set_handler(exc_debug as u64, 0x08, 0x8E);
        IDT[2].set_handler(exc_nmi as u64, 0x08, 0x8E);
        IDT[3].set_handler(exc_breakpoint as u64, 0x08, 0x8E);
        IDT[4].set_handler(exc_overflow as u64, 0x08, 0x8E);
        IDT[5].set_handler(exc_bound_range as u64, 0x08, 0x8E);
        IDT[6].set_handler(exc_invalid_opcode as u64, 0x08, 0x8E);
        IDT[7].set_handler(exc_device_not_available as u64, 0x08, 0x8E);
        IDT[8].set_handler(exc_double_fault as u64, 0x08, 0x8E);
        IDT[9].set_handler(exc_coprocessor_overrun as u64, 0x08, 0x8E);
        IDT[10].set_handler(exc_invalid_tss as u64, 0x08, 0x8E);
        IDT[11].set_handler(exc_segment_not_present as u64, 0x08, 0x8E);
        IDT[12].set_handler(exc_stack_segment as u64, 0x08, 0x8E);
        IDT[13].set_handler(exc_general_protection as u64, 0x08, 0x8E);
        IDT[14].set_handler(exc_page_fault as u64, 0x08, 0x8E);
        IDT[15].set_handler(exc_spurious_interrupt as u64, 0x08, 0x8E);
        IDT[16].set_handler(exc_x87_fpu_error as u64, 0x08, 0x8E);
        IDT[17].set_handler(exc_alignment_check as u64, 0x08, 0x8E);
        IDT[18].set_handler(exc_machine_check as u64, 0x08, 0x8E);
        IDT[19].set_handler(exc_simd_fpu as u64, 0x08, 0x8E);
        IDT[20].set_handler(exc_virtualization as u64, 0x08, 0x8E);
        IDT[21].set_handler(exc_control_protection as u64, 0x08, 0x8E);
        IDT[22].set_handler(exc_reserved_22 as u64, 0x08, 0x8E);
        IDT[23].set_handler(exc_reserved_23 as u64, 0x08, 0x8E);
        IDT[24].set_handler(exc_reserved_24 as u64, 0x08, 0x8E);
        IDT[25].set_handler(exc_reserved_25 as u64, 0x08, 0x8E);
        IDT[26].set_handler(exc_reserved_26 as u64, 0x08, 0x8E);
        IDT[27].set_handler(exc_reserved_27 as u64, 0x08, 0x8E);
        IDT[28].set_handler(exc_hypervisor_injection as u64, 0x08, 0x8E);
        IDT[29].set_handler(exc_vmm_communication as u64, 0x08, 0x8E);
        IDT[30].set_handler(exc_security_exception as u64, 0x08, 0x8E);
        IDT[31].set_handler(exc_reserved_31 as u64, 0x08, 0x8E);

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
            base: &raw const IDT as u64,
        };

        asm!(
            "lidt [{}]",
            in(reg) &idt_ptr,
            options(nostack)
        );
    }
}

macro_rules! exception_handler_no_code {
    ($name:ident, $vec:expr) => {
        extern "x86-interrupt" fn $name(frame: &mut ExceptionStackFrame) {
            handle_exception($vec, frame, None);
        }
    };
}

macro_rules! exception_handler_with_code {
    ($name:ident, $vec:expr) => {
        extern "x86-interrupt" fn $name(frame: &mut ExceptionStackFrame, error_code: u64) {
            handle_exception($vec, frame, Some(error_code));
        }
    };
}

exception_handler_no_code!(exc_divide_error, 0);
exception_handler_no_code!(exc_debug, 1);
exception_handler_no_code!(exc_nmi, 2);
exception_handler_no_code!(exc_breakpoint, 3);
exception_handler_no_code!(exc_overflow, 4);
exception_handler_no_code!(exc_bound_range, 5);
exception_handler_no_code!(exc_invalid_opcode, 6);
exception_handler_no_code!(exc_device_not_available, 7);
exception_handler_with_code!(exc_double_fault, 8);
exception_handler_no_code!(exc_coprocessor_overrun, 9);
exception_handler_with_code!(exc_invalid_tss, 10);
exception_handler_with_code!(exc_segment_not_present, 11);
exception_handler_with_code!(exc_stack_segment, 12);
exception_handler_with_code!(exc_general_protection, 13);
exception_handler_with_code!(exc_page_fault, 14);
exception_handler_no_code!(exc_spurious_interrupt, 15);
exception_handler_no_code!(exc_x87_fpu_error, 16);
exception_handler_with_code!(exc_alignment_check, 17);
exception_handler_no_code!(exc_machine_check, 18);
exception_handler_no_code!(exc_simd_fpu, 19);
exception_handler_no_code!(exc_virtualization, 20);
exception_handler_with_code!(exc_control_protection, 21);
exception_handler_no_code!(exc_reserved_22, 22);
exception_handler_no_code!(exc_reserved_23, 23);
exception_handler_no_code!(exc_reserved_24, 24);
exception_handler_no_code!(exc_reserved_25, 25);
exception_handler_no_code!(exc_reserved_26, 26);
exception_handler_no_code!(exc_reserved_27, 27);
exception_handler_no_code!(exc_hypervisor_injection, 28);
exception_handler_with_code!(exc_vmm_communication, 29);
exception_handler_no_code!(exc_security_exception, 30);
exception_handler_no_code!(exc_reserved_31, 31);

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
fn handle_exception(vector: u8, frame: &mut ExceptionStackFrame, error_code: Option<u64>) {
    unsafe {
        let serial_putc_trap = |c: u8| {
            loop {
                let lsr: u8;
                asm!(
                    "in al, dx",
                    in("dx") 0x3FDu16,
                    out("al") lsr,
                    options(nostack, nomem)
                );
                if (lsr & 0x20) != 0 {
                    break;
                }
            }
            asm!(
                "out dx, al",
                in("dx") 0x3F8u16,
                in("al") c,
                options(nostack, nomem)
            );
        };
        let puts = |s: &[u8]| {
            for &b in s {
                if b == b'\n' {
                    serial_putc_trap(b'\r');
                }
                serial_putc_trap(b);
            }
        };
        let print_hex_val = |val: u64| {
            let hex = b"0123456789abcdef";
            let mut buf = [b'0'; 16];
            let mut v = val;
            for i in (0..16).rev() {
                buf[i] = hex[(v & 0xf) as usize];
                v >>= 4;
            }
            puts(b"0x");
            puts(&buf);
        };

        puts(b"\n!!! EXCEPTION !!!\n");
        puts(b"  vector=");
        print_hex_val(vector as u64);
        puts(b"\n  RIP=");
        print_hex_val(frame.rip);
        puts(b"\n  CS=");
        print_hex_val(frame.cs);
        puts(b"\n  RSP=");
        print_hex_val(frame.rsp);
        puts(b"\n  SS=");
        print_hex_val(frame.ss);
        if let Some(code) = error_code {
            puts(b"\n  error_code=");
            print_hex_val(code);
        }
        puts(b"\n");

        if vector == exception::PAGE_FAULT {
            let cr2: u64;
            asm!("mov {}, cr2", out(reg) cr2);
            puts(b"  CR2=");
            print_hex_val(cr2);
            puts(b"\n");
        }
    }

    // Halt
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
#[unsafe(no_mangle)]
#[used]
pub static _kernel_trap_entry: u64 = 0;

#[unsafe(no_mangle)]
#[used]
pub static _user_trap_entry: u64 = 0;
