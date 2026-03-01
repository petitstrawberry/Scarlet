//! x86_64 boot code using Limine protocol

use core::arch::asm;
use core::mem::transmute;

use crate::arch::x86_64::earlycon::init_earlycon;
use crate::arch::x86_64::{CPUS, X86_64, trap_init};
use crate::mem::init_bss;
use crate::vm::vmem::MemoryArea;
use crate::{BootInfo, start_kernel};
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

static mut BOOT_INFO_STORAGE: [u64; 16] = [0; 16];

#[unsafe(no_mangle)]
pub extern "C" fn kmain() -> ! {
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

    fn print_hex(mut val: u64) {
        const HEX_CHARS: &[u8] = b"0123456789abcdef";
        serial_puts(b"0x");
        for i in (0..16).rev() {
            let nibble = ((val >> (i * 4)) & 0xf) as usize;
            serial_putc(HEX_CHARS[nibble]);
        }
    }

    if !BASE_REVISION.is_supported() {
        serial_puts(b"ERROR: Limine revision not supported\n");
        loop {
            core::hint::spin_loop();
        }
    }

    init_bss();
    init_earlycon();

    init_gdt();

    let cr3: u64;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nostack));
    }
    serial_puts(b"[x86_64] Current CR3: ");
    print_hex(cr3);
    serial_puts(b"\n");

    let hhdm_offset = HHDM_REQUEST.get_response().map(|r| r.offset()).unwrap_or(0);
    serial_puts(b"[x86_64] HHDM offset: ");
    print_hex(hhdm_offset);
    serial_puts(b"\n");

    serial_puts(b"[x86_64] Checking memory layout...\n");
    let pml4_virt = (cr3 + hhdm_offset) as *const u64;
    let pml4_256 = unsafe { pml4_virt.add(256).read_volatile() };
    serial_puts(b"[x86_64] PML4[256] = ");
    print_hex(pml4_256);
    serial_puts(b"\n");

    let pdpt_phys = pml4_256 & 0x000ffffffffff000;
    let pdpt_virt = (pdpt_phys + hhdm_offset) as *const u64;
    let pdpt_0 = unsafe { pdpt_virt.read_volatile() };
    serial_puts(b"[x86_64] PDPT[0] = ");
    print_hex(pdpt_0);
    serial_puts(b"\n");

    let pd_phys = pdpt_0 & 0x000ffffffffff000;
    let pd_virt = (pd_phys + hhdm_offset) as *const u64;
    let pd_0 = unsafe { pd_virt.read_volatile() };
    serial_puts(b"[x86_64] PD[0] = ");
    print_hex(pd_0);
    serial_puts(b"\n");

    let pd_72 = unsafe { pd_virt.add(72).read_volatile() };
    serial_puts(b"[x86_64] PD[72] (heap area) = ");
    print_hex(pd_72);
    serial_puts(b"\n");

    if pd_72 & 1 != 0 {
        serial_puts(b"[x86_64] PD[72] present, flags: ");
        print_hex(pd_72 & 0xFFF);
        serial_puts(b"\n");
    } else {
        serial_puts(b"[x86_64] ERROR: PD[72] not present!\n");
    }

    let pd_100 = unsafe { pd_virt.add(100).read_volatile() };
    serial_puts(b"[x86_64] PD[100] (end of usable) = ");
    print_hex(pd_100);
    serial_puts(b"\n");
    if pd_100 & 1 == 0 {
        serial_puts(b"[x86_64] WARNING: PD[100] not mapped!\n");
    }

    let (usable_start, usable_end) = find_usable_memory();
    let initramfs = find_initramfs();

    let cpu_id = 0;
    let x86_64: &mut X86_64 = unsafe { transmute(&CPUS[cpu_id] as *const _ as usize) };
    x86_64.cpuid = cpu_id as u64;
    trap_init(x86_64);

    let hhdm = hhdm_offset as usize;
    let usable_mem = MemoryArea::new(usable_start + hhdm, usable_end + hhdm);

    serial_puts(b"[x86_64] Physical usable: ");
    print_hex(usable_start as u64);
    serial_puts(b" - ");
    print_hex(usable_end as u64);
    serial_puts(b"\n");
    serial_puts(b"[x86_64] Virtual usable: ");
    print_hex(usable_mem.start as u64);
    serial_puts(b" - ");
    print_hex(usable_mem.end as u64);
    serial_puts(b"\n");
    if let Some(ref initram) = initramfs {
        serial_puts(b"[x86_64] Initramfs: ");
        print_hex(initram.start as u64);
        serial_puts(b" - ");
        print_hex(initram.end as u64);
        serial_puts(b"\n");
    }

    let boot_info_ptr: *mut BootInfo =
        unsafe { core::ptr::addr_of_mut!(BOOT_INFO_STORAGE) as *mut u64 as *mut BootInfo };

    unsafe {
        let storage_ptr = boot_info_ptr as *mut u8;
        for i in 0..core::mem::size_of::<BootInfo>() {
            storage_ptr.add(i).write_volatile(0);
        }

        let storage = boot_info_ptr as *mut usize;
        *storage.add(0) = cpu_id;
        *storage.add(1) = 1;
        *storage.add(2) = usable_mem.start;
        *storage.add(3) = usable_mem.end;

        let initramfs_storage = storage.add(4) as *mut u64;
        initramfs_storage.write_volatile(1);

        if let Some(ref initram) = initramfs {
            let area_start = storage.add(5);
            let area_end = storage.add(6);
            area_start.write_volatile(initram.start);
            area_end.write_volatile(initram.end);
        }

        let ds_storage = storage.add(9) as *mut u8;
        ds_storage.write_volatile(1);

        start_kernel(&*boot_info_ptr);
    }
}

fn find_usable_memory() -> (usize, usize) {
    let memmap = match MEMORY_MAP_REQUEST.get_response() {
        Some(r) => r,
        None => return (0x100000, 0x8000000),
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
        return (0x100000, 0x8000000);
    }

    (best_start as usize, (best_start + best_size - 1) as usize)
}

fn find_initramfs() -> Option<MemoryArea> {
    let response = match MODULE_REQUEST.get_response() {
        Some(r) => r,
        None => return None,
    };

    let modules = response.modules();
    if modules.is_empty() {
        return None;
    }

    let module = &modules[0];
    Some(MemoryArea::new(
        module.addr() as usize,
        module.addr() as usize + module.size() as usize - 1,
    ))
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
