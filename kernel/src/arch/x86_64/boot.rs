//! x86_64 boot code using Limine protocol

use core::arch::asm;
use core::mem::transmute;

use crate::arch::x86_64::earlycon::init_earlycon;
use crate::arch::x86_64::{CPUS, X86_64, trap_init};
use crate::early_println;
use crate::mem::init_bss;
use crate::vm::vmem::MemoryArea;
use crate::{BootInfo, DeviceSource, start_kernel};
use limine::BaseRevision;
use limine::request::{
    FramebufferRequest, HhdmRequest, KernelAddressRequest, MemoryMapRequest, ModuleRequest,
    RequestsEndMarker, RequestsStartMarker,
};

/// Define the start and end markers for Limine requests.
#[used]
#[unsafe(link_section = ".requests_start_marker")]
static _START_MARKER: RequestsStartMarker = RequestsStartMarker::new();

/// Sets the base revision to the latest revision supported by the crate.
#[used]
#[unsafe(link_section = ".requests")]
static BASE_REVISION: BaseRevision = BaseRevision::new();

#[used]
#[unsafe(link_section = ".requests")]
static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static MEMORY_MAP_REQUEST: MemoryMapRequest = MemoryMapRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static MODULE_REQUEST: ModuleRequest = ModuleRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static KERNEL_ADDRESS_REQUEST: KernelAddressRequest = KernelAddressRequest::new();

#[used]
#[unsafe(link_section = ".requests_end_marker")]
static _END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

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

#[repr(C, packed)]
struct GdtPointer {
    limit: u16,
    base: u64,
}

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

static mut GDT: [GdtEntry; 6] = [GdtEntry::new(); 6];
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
        self.access = 0x9A;
        self.flags_limit_high = 0xAF;
    }

    fn set_kernel_data(&mut self) {
        self.limit_low = 0xFFFF;
        self.access = 0x92;
        self.flags_limit_high = 0xCF;
    }

    fn set_user_code(&mut self) {
        self.limit_low = 0xFFFF;
        self.access = 0xFA;
        self.flags_limit_high = 0xAF;
    }

    fn set_user_data(&mut self) {
        self.limit_low = 0xFFFF;
        self.access = 0xF2;
        self.flags_limit_high = 0xCF;
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

fn init_gdt() {
    unsafe {
        GDT[0] = GdtEntry::new();
        GDT[1].set_kernel_code();
        GDT[2].set_kernel_data();
        GDT[3].set_user_code();
        GDT[4].set_user_data();

        GDT[5].limit_low = (core::mem::size_of::<TaskStateSegment>() - 1) as u16;
        GDT[5].base_low = (&raw const TSS as u64) as u16;
        GDT[5].base_mid = ((&raw const TSS as u64) >> 16) as u8;
        GDT[5].access = 0x89;
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

#[unsafe(no_mangle)]
pub extern "C" fn kmain() -> ! {
    // Direct serial output to verify kmain is being called
    unsafe {
        const SERIAL_THR: u16 = 0x3F8;
        const SERIAL_LSR: u16 = 0x3FD;
        const SERIAL_LSR_THRE: u8 = 0x20;

        // Wait for transmit ready
        while {
            let lsr: u8;
            core::arch::asm!(
                "in al, dx",
                in("dx") SERIAL_LSR,
                out("al") lsr,
                options(nostack, nomem)
            );
            (lsr & SERIAL_LSR_THRE) == 0
        } {
            core::hint::spin_loop();
        }

        // Output 'K' to indicate kmain started
        core::arch::asm!(
            "out dx, al",
            in("dx") SERIAL_THR,
            in("al") b'K',
            options(nostack, nomem)
        );
    }

    // All limine requests must also be referenced, otherwise they may be removed by the linker.
    assert!(BASE_REVISION.is_supported());

    init_bss();

    init_earlycon();
    early_println!("[x86_64] Scarlet kernel starting via Limine...");

    init_gdt();
    early_println!("[x86_64] GDT initialized");

    let hhdm_offset = HHDM_REQUEST.get_response().map(|r| r.offset()).unwrap_or(0);
    early_println!("[x86_64] HHDM offset: {:#x}", hhdm_offset);

    if let Some(addr_resp) = KERNEL_ADDRESS_REQUEST.get_response() {
        early_println!(
            "[x86_64] Kernel physical: {:#x}, virtual: {:#x}",
            addr_resp.physical_base(),
            addr_resp.virtual_base()
        );
    }

    let (usable_start, usable_end) = find_usable_memory();
    early_println!(
        "[x86_64] Usable memory: {:#x} - {:#x}",
        usable_start,
        usable_end
    );

    let initramfs = find_initramfs();

    let cpu_id = 0;
    early_println!("[x86_64] CPU {}: Initializing...", cpu_id);
    let x86_64: &mut X86_64 = unsafe { transmute(&CPUS[cpu_id] as *const _ as usize) };
    x86_64.cpuid = cpu_id as u64;
    trap_init(x86_64);

    let boot_info = BootInfo::new(
        cpu_id,
        1,
        MemoryArea::new(usable_start, usable_end),
        initramfs,
        None,
        DeviceSource::None,
    );

    early_println!("[x86_64] Calling start_kernel...");
    start_kernel(&boot_info);
}

fn find_usable_memory() -> (usize, usize) {
    let memmap = match MEMORY_MAP_REQUEST.get_response() {
        Some(r) => r,
        None => {
            early_println!("[x86_64] Warning: No memory map from Limine, using fallback");
            return (0x100000, 0x8000000);
        }
    };

    let mut best_start: u64 = 0;
    let mut best_size: u64 = 0;

    for entry in memmap.entries() {
        if entry.entry_type == limine::memory_map::EntryType::USABLE && entry.length > best_size {
            best_start = entry.base;
            best_size = entry.length;
        }
    }

    if best_size == 0 {
        early_println!("[x86_64] Warning: No usable memory found, using fallback");
        return (0x100000, 0x8000000);
    }

    (best_start as usize, (best_start + best_size - 1) as usize)
}

fn find_initramfs() -> Option<MemoryArea> {
    let response = MODULE_REQUEST.get_response()?;
    let modules = response.modules();

    for module in modules {
        let cmdline = module.string().to_str().unwrap_or("");

        if cmdline.contains("initramfs") {
            early_println!(
                "[x86_64] Found initramfs module at {:#x}, size {} bytes",
                module.addr() as usize,
                module.size()
            );
            return Some(MemoryArea::new(
                module.addr() as usize,
                module.addr() as usize + module.size() as usize - 1,
            ));
        }
    }

    early_println!("[x86_64] No initramfs module found");
    None
}

pub fn get_bsp_id() -> u8 {
    0
}

pub fn init_kernel_stack(cpu_id: usize, stack_top: usize) {
    unsafe {
        TSS.rsp0 = stack_top as u64;
    }
    let _ = cpu_id;
}
