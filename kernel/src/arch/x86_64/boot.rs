//! x86_64 boot code

use core::arch::asm;

use crate::arch::x86_64::earlycon::init_earlycon;
use crate::early_println;

#[repr(C, align(8))]
struct Multiboot2Header {
    magic: u32,
    architecture: u32,
    header_length: u32,
    checksum: u32,
    end_tag: [u32; 2],
}

#[unsafe(link_section = ".head.text")]
#[used]
static MULTIBOOT2_HEADER: Multiboot2Header = Multiboot2Header {
    magic: 0xE85250D6,
    architecture: 0,
    header_length: core::mem::size_of::<Multiboot2Header>() as u32,
    checksum: !(0xE85250D6u32
        .wrapping_add(0)
        .wrapping_add(core::mem::size_of::<Multiboot2Header>() as u32))
    .wrapping_add(1),
    end_tag: [0, 8],
};

/// GDT entry structure
#[derive(Clone, Copy)]
#[repr(C, packed)]
struct GdtEntry {
    limit_low: u16,
    base_low: u16,
    base_mid: u8,
    access: u8,
    flags_limit_high: u8,
    base_high: u8,
}

/// GDT pointer structure
#[repr(C, packed)]
struct GdtPointer {
    limit: u16,
    base: u64,
}

/// TSS structure for x86_64
#[repr(C, packed)]
struct TaskStateSegment {
    reserved0: u32,
    rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    reserved1: u64,
    ist1: u64,
    ist2: u64,
    ist3: u64,
    ist4: u64,
    ist5: u64,
    ist6: u64,
    ist7: u64,
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
}

/// Global GDT (6 entries)
static mut GDT: [GdtEntry; 6] = [GdtEntry::new(); 6];

/// Global TSS
static mut TSS: TaskStateSegment = TaskStateSegment::new();

impl GdtEntry {
    const fn new() -> Self {
        GdtEntry {
            limit_low: 0,
            base_low: 0,
            base_mid: 0,
            access: 0,
            flags_limit_high: 0,
            base_high: 0,
        }
    }

    fn set_kernel_code(&mut self) {
        self.limit_low = 0xFFFF;
        self.base_low = 0;
        self.base_mid = 0;
        self.access = 0x9A; // Present, Ring 0, Code, Executable, Readable
        self.flags_limit_high = 0xAF; // 64-bit, 4KB granular
        self.base_high = 0;
    }

    fn set_kernel_data(&mut self) {
        self.limit_low = 0xFFFF;
        self.base_low = 0;
        self.base_mid = 0;
        self.access = 0x92; // Present, Ring 0, Data, Writable
        self.flags_limit_high = 0xCF; // 64-bit, 4KB granular
        self.base_high = 0;
    }

    fn set_user_code(&mut self) {
        self.limit_low = 0xFFFF;
        self.base_low = 0;
        self.base_mid = 0;
        self.access = 0xFA; // Present, Ring 3, Code, Executable, Readable
        self.flags_limit_high = 0xAF; // 64-bit, 4KB granular
        self.base_high = 0;
    }

    fn set_user_data(&mut self) {
        self.limit_low = 0xFFFF;
        self.base_low = 0;
        self.base_mid = 0;
        self.access = 0xF2; // Present, Ring 3, Data, Writable
        self.flags_limit_high = 0xCF; // 64-bit, 4KB granular
        self.base_high = 0;
    }
}

impl TaskStateSegment {
    const fn new() -> Self {
        TaskStateSegment {
            reserved0: 0,
            rsp0: 0,
            rsp1: 0,
            rsp2: 0,
            reserved1: 0,
            ist1: 0,
            ist2: 0,
            ist3: 0,
            ist4: 0,
            ist5: 0,
            ist6: 0,
            ist7: 0,
            reserved2: 0,
            reserved3: 0,
            iomap_base: 0,
        }
    }
}

/// Initialize the GDT
fn init_gdt() {
    unsafe {
        // Null descriptor
        GDT[0] = GdtEntry::new();

        // Kernel code segment (0x08)
        GDT[1].set_kernel_code();

        // Kernel data segment (0x10)
        GDT[2].set_kernel_data();

        // User code segment (0x1B)
        GDT[3].set_user_code();

        // User data segment (0x23)
        GDT[4].set_user_data();

        // TSS descriptor (would be filled in with TSS address)
        GDT[5].limit_low = (core::mem::size_of::<TaskStateSegment>() - 1) as u16;
        GDT[5].base_low = (&raw const TSS as u64) as u16;
        GDT[5].base_mid = ((&raw const TSS as u64) >> 16) as u8;
        GDT[5].access = 0x89; // Present, Ring 0, TSS, Available
        GDT[5].flags_limit_high = 0x00;
        GDT[5].base_high = ((&raw const TSS as u64) >> 24) as u8;

        let gdt_ptr = GdtPointer {
            limit: (core::mem::size_of::<[GdtEntry; 6]>() - 1) as u16,
            base: &raw const GDT as u64,
        };

        asm!(
            "lgdt [{}]",
            in(reg) &gdt_ptr,
            options(nostack)
        );

        // Reload segment registers
        asm!(
            "mov ax, 0x10",
            "mov ds, ax",
            "mov es, ax",
            "mov fs, ax",
            "mov gs, ax",
            "mov ss, ax",
            options(nostack)
        );
    }
}

/// Early boot entry point
///
/// This is called from the bootloader (e.g., GRUB, Limine) in long mode.
/// The bootloader should have already:
/// - Set up 64-bit long mode
/// - Set up identity-mapped page tables
/// - Loaded the kernel at the correct address
///
/// # Safety
/// This function must only be called once during boot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    // Disable interrupts
    asm!("cli", options(nostack));

    // Initialize early console
    init_earlycon();
    early_println!("[x86_64] Booting Scarlet kernel...");

    // Initialize GDT
    init_gdt();
    early_println!("[x86_64] GDT initialized");

    // Initialize kernel heap and other subsystems...
    // (This would be done in the main kernel initialization)

    // Jump to kernel main
    early_println!("[x86_64] Jumping to kernel main...");

    // For now, just halt
    loop {
        asm!("hlt", options(nostack));
    }
}

/// Get the BSP (Bootstrap Processor) CPU ID
pub fn get_bsp_id() -> u8 {
    // On x86_64, the BSP always starts with APIC ID 0 in simple configurations
    0
}

/// Initialize the kernel stack for the current CPU
pub fn init_kernel_stack(cpu_id: usize, stack_top: usize) {
    unsafe {
        // Update TSS RSP0 with kernel stack top
        TSS.rsp0 = stack_top as u64;
    }
    let _ = cpu_id; // Suppress unused warning
}
