//! x86_64 boot code using Limine protocol

use core::arch::asm;
use core::mem::transmute;
use core::ptr;

use crate::arch::x86_64::earlycon::init_earlycon;
use crate::arch::x86_64::{CPUS, X86_64, trap_init};
use crate::mem::init_bss;
use crate::vm::vmem::MemoryArea;
use crate::{BootInfo, DeviceSource, start_kernel};
use limine::BaseRevision;
use limine::request::{
    HhdmRequest, KernelAddressRequest, MemoryMapRequest, ModuleRequest, RequestsEndMarker,
    RequestsStartMarker,
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

static mut BOOT_INFO_STORAGE: BootInfo = unsafe { core::mem::zeroed() };
static mut HHDM_OFFSET: usize = 0;

fn serial_putc(c: u8) {
    unsafe {
        const SERIAL_THR: u16 = 0x3F8;
        const SERIAL_LSR: u16 = 0x3FD;
        const SERIAL_LSR_THRE: u8 = 0x20;

        loop {
            let lsr: u8;
            core::arch::asm!(
                "in al, dx",
                in("dx") SERIAL_LSR,
                out("al") lsr,
                options(nostack, nomem)
            );
            if (lsr & SERIAL_LSR_THRE) != 0 {
                break;
            }
            core::hint::spin_loop();
        }

        core::arch::asm!(
            "out dx, al",
            in("dx") SERIAL_THR,
            in("al") c,
            options(nostack, nomem)
        );
    }
}

fn serial_puts(s: &[u8]) {
    for &c in s {
        if c == b'\n' {
            serial_putc(b'\r');
        }
        serial_putc(c);
    }
}

fn print_hex(val: u64) {
    const HEX_CHARS: &[u8] = b"0123456789abcdef";
    serial_puts(b"0x");
    for i in (0..16).rev() {
        let nibble = ((val >> (i * 4)) & 0xf) as usize;
        serial_putc(HEX_CHARS[nibble]);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kmain() -> ! {
    if !BASE_REVISION.is_supported() {
        serial_puts(b"ERROR: Limine revision not supported\n");
        loop {
            core::hint::spin_loop();
        }
    }

    init_bss();
    init_earlycon();

    init_gdt();

    // Get HHDM offset (physical → HHDM virtual: virt = phys + hhdm)
    let hhdm_offset = HHDM_REQUEST.get_response().map(|r| r.offset()).unwrap_or(0);
    let hhdm = hhdm_offset as usize;
    unsafe {
        HHDM_OFFSET = hhdm;
    }
    serial_puts(b"[x86_64] HHDM offset: ");
    print_hex(hhdm_offset);
    serial_puts(b"\n");

    // Get kernel physical/virtual base from Limine
    let kernel_addr = KERNEL_ADDRESS_REQUEST
        .get_response()
        .expect("KernelAddressRequest not answered by bootloader");
    let kernel_phys_base = kernel_addr.physical_base() as usize;
    let kernel_virt_base = kernel_addr.virtual_base() as usize;

    serial_puts(b"[x86_64] Kernel physical base: ");
    print_hex(kernel_phys_base as u64);
    serial_puts(b"\n");
    serial_puts(b"[x86_64] Kernel virtual base: ");
    print_hex(kernel_virt_base as u64);
    serial_puts(b"\n");

    // Compute kernel physical extent using linker symbols
    let kernel_virt_start = unsafe { &crate::mem::__KERNEL_SPACE_START as *const usize as usize };
    let kernel_virt_end = unsafe { &crate::mem::__KERNEL_SPACE_END as *const usize as usize };
    let kernel_size = kernel_virt_end - kernel_virt_start;
    // Align kernel physical end to page boundary
    let kernel_phys_end = (kernel_phys_base + kernel_size + 0xFFF) & !0xFFF;

    serial_puts(b"[x86_64] Kernel size: ");
    print_hex(kernel_size as u64);
    serial_puts(b"\n");
    serial_puts(b"[x86_64] Kernel physical end (aligned): ");
    print_hex(kernel_phys_end as u64);
    serial_puts(b"\n");

    // Find usable memory region after the kernel's physical footprint
    let mut usable_memory = find_usable_memory_after_kernel(kernel_phys_end, hhdm);

    serial_puts(b"[x86_64] Usable memory (HHDM): ");
    print_hex(usable_memory.start as u64);
    serial_puts(b" - ");
    print_hex(usable_memory.end as u64);
    serial_puts(b"\n");

    // Relocate initramfs into usable_memory (same strategy as RISC-V/AArch64)
    let relocated_initramfs = relocate_initramfs_from_module(&mut usable_memory);

    if let Some(ref area) = relocated_initramfs {
        serial_puts(b"[x86_64] Relocated initramfs: ");
        print_hex(area.start as u64);
        serial_puts(b" - ");
        print_hex(area.end as u64);
        serial_puts(b"\n");
    }

    serial_puts(b"[x86_64] Usable memory after initramfs: ");
    print_hex(usable_memory.start as u64);
    serial_puts(b" - ");
    print_hex(usable_memory.end as u64);
    serial_puts(b"\n");

    let heap_hhdm = usable_memory;

    // Initialize CPU and traps
    let cpu_id = 0;
    let x86_64: &mut X86_64 = unsafe { transmute(&CPUS[cpu_id] as *const _ as usize) };
    x86_64.cpuid = cpu_id as u64;

    trap_init(x86_64);
    serial_puts(b"[x86_64] trap_init: done\n");

    // Enable SSE for kernel use: trap_init sets CR0.TS which traps SSE instructions.
    // The compiler may generate SSE (movups, etc.) so we must clear TS.
    unsafe {
        let mut cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0);
        cr0 &= !(1u64 << 2); // Clear TS
        core::arch::asm!("mov cr0, {}", in(reg) cr0);
        crate::arch::x86_64::fpu::init_fpu();
    }

    serial_puts(b"[x86_64] Building BootInfo...\n");

    serial_puts(b"[x86_64] cpu_id=");
    print_hex(cpu_id as u64);
    serial_puts(b"\n");

    serial_puts(b"[x86_64] usable_memory.start=");
    print_hex(usable_memory.start as u64);
    serial_puts(b" end=");
    print_hex(usable_memory.end as u64);
    serial_puts(b"\n");

    serial_puts(b"[x86_64] relocated_initramfs is_some=");
    if relocated_initramfs.is_some() {
        serial_puts(b"true\n");
    } else {
        serial_puts(b"false\n");
    }

    serial_puts(b"[x86_64] About to construct BootInfo struct directly...\n");

    unsafe {
        let storage = &raw mut BOOT_INFO_STORAGE;
        serial_puts(b"[x86_64] BOOT_INFO_STORAGE addr=");
        print_hex(storage as u64);
        serial_puts(b"\n");

        serial_puts(b"[x86_64] Writing cpu_id...\n");
        let cpu_id_ptr = &raw mut (*storage).cpu_id;
        serial_puts(b"[x86_64]   field addr=");
        print_hex(cpu_id_ptr as u64);
        serial_puts(b"\n");
        core::ptr::write_volatile(cpu_id_ptr, cpu_id);

        serial_puts(b"[x86_64] Writing cpu_count...\n");
        let cpu_count_ptr = &raw mut (*storage).cpu_count;
        serial_puts(b"[x86_64]   field addr=");
        print_hex(cpu_count_ptr as u64);
        serial_puts(b"\n");
        core::ptr::write_volatile(cpu_count_ptr, 1);

        serial_puts(b"[x86_64] Writing usable_memory...\n");
        let usable_memory_ptr = &raw mut (*storage).usable_memory;
        serial_puts(b"[x86_64]   field addr=");
        print_hex(usable_memory_ptr as u64);
        serial_puts(b"\n");
        core::ptr::write_volatile(usable_memory_ptr, heap_hhdm);

        serial_puts(b"[x86_64] Writing initramfs...\n");
        let initramfs_ptr = &raw mut (*storage).initramfs;
        serial_puts(b"[x86_64]   field addr=");
        print_hex(initramfs_ptr as u64);
        serial_puts(b"\n");
        core::ptr::write_volatile(initramfs_ptr, relocated_initramfs);

        serial_puts(b"[x86_64] Writing cmdline...\n");
        core::ptr::write_volatile(&raw mut (*storage).cmdline, None);

        serial_puts(b"[x86_64] Writing device_source...\n");
        core::ptr::write_volatile(&raw mut (*storage).device_source, DeviceSource::None);

        serial_puts(b"[x86_64] BootInfo stored. Calling start_kernel...\n");
        start_kernel(&*storage);
    }
}

/// Find the best usable memory region that starts at or after `kernel_phys_end`.
///
/// Scans Limine's memory map for USABLE entries. If a USABLE entry contains
/// `kernel_phys_end`, the region is trimmed to start after the kernel.
/// Returns the largest available region as a MemoryArea in HHDM virtual space.
fn find_usable_memory_after_kernel(kernel_phys_end: usize, hhdm: usize) -> MemoryArea {
    let memmap = MEMORY_MAP_REQUEST
        .get_response()
        .expect("MemoryMapRequest not answered by bootloader");

    let mut best_start: usize = 0;
    let mut best_end: usize = 0;
    let mut best_size: usize = 0;

    for entry in memmap.entries() {
        if entry.entry_type != limine::memory_map::EntryType::USABLE {
            continue;
        }

        let entry_start = entry.base as usize;
        let entry_end = (entry.base + entry.length) as usize; // exclusive

        // Determine the effective start: skip over the kernel if it overlaps
        let effective_start = if kernel_phys_end > entry_start && kernel_phys_end < entry_end {
            // Kernel ends within this entry; start after the kernel
            kernel_phys_end
        } else if entry_start >= kernel_phys_end {
            // Entry is entirely after the kernel
            entry_start
        } else {
            // Entry is entirely before kernel_phys_end; skip it
            continue;
        };

        let size = entry_end - effective_start;
        if size > best_size {
            best_start = effective_start;
            best_end = entry_end - 1; // inclusive
            best_size = size;
        }
    }

    if best_size == 0 {
        serial_puts(b"[x86_64] FATAL: No usable memory found after kernel\n");
        loop {
            core::hint::spin_loop();
        }
    }

    // Convert physical addresses to HHDM virtual addresses
    MemoryArea::new(best_start + hhdm, best_end + hhdm)
}

/// Relocate the initramfs (Limine module) to the start of usable_memory.
///
/// This mirrors the logic of `relocate_initramfs()` in `initramfs.rs`, but
/// reads the source from Limine's MODULE_REQUEST instead of FdtManager,
/// since x86_64 uses Limine protocol rather than FDT.
///
/// After relocation, `usable_memory.start` is advanced past the copied data.
fn relocate_initramfs_from_module(usable_memory: &mut MemoryArea) -> Option<MemoryArea> {
    let response = MODULE_REQUEST.get_response()?;
    let modules = response.modules();
    if modules.is_empty() {
        serial_puts(b"[x86_64] No modules (initramfs) found\n");
        return None;
    }

    let module = &modules[0];
    let src_addr = module.addr() as usize; // Already an HHDM virtual address
    let size = module.size() as usize;

    if size == 0 || size > 0x10000000 {
        serial_puts(b"[x86_64] Invalid initramfs size\n");
        return None;
    }
    if src_addr == 0 {
        serial_puts(b"[x86_64] Invalid initramfs source address\n");
        return None;
    }

    serial_puts(b"[x86_64] Original initramfs at: ");
    print_hex(src_addr as u64);
    serial_puts(b", size: ");
    print_hex(size as u64);
    serial_puts(b"\n");

    // Ensure proper 8-byte alignment for destination
    let aligned_addr = (usable_memory.start + 7) & !7;

    // Validate destination memory bounds
    if aligned_addr + size > usable_memory.end {
        serial_puts(b"[x86_64] Insufficient memory for initramfs relocation\n");
        return None;
    }

    // Create the new memory area
    let new_area = MemoryArea::new(aligned_addr, aligned_addr + size - 1);

    // Copy in 4KB chunks (same approach as initramfs.rs)
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);

    let chunk_size = 4096;
    let mut src = src_addr as *const u8;
    let mut dst = aligned_addr as *mut u8;
    let mut remaining = size;

    unsafe {
        while remaining > 0 {
            let copy_size = if remaining > chunk_size {
                chunk_size
            } else {
                remaining
            };
            ptr::copy_nonoverlapping(src, dst, copy_size);

            src = src.add(copy_size);
            dst = dst.add(copy_size);
            remaining -= copy_size;

            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        }
    }

    // Advance usable_memory past the relocated initramfs (8-byte aligned)
    usable_memory.start = (aligned_addr + size + 7) & !7;

    Some(new_area)
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

pub fn hhdm_phys_to_virt(phys: usize) -> usize {
    unsafe { phys + HHDM_OFFSET }
}

pub fn hhdm_offset() -> usize {
    unsafe { HHDM_OFFSET }
}
