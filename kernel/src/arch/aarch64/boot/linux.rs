//! Linux arm64 Image boot protocol entry.
//!
//! The bootloader supplies only the standard register/FDT contract. Scarlet
//! builds its own temporary page table and HHDM before entering common kernel
//! initialization; no Limine response or bootloader-owned direct map is used.

mod framebuffer;
mod page_table;
mod smp;

pub use smp::secondary_image_entry;

use core::arch::naked_asm;
use core::mem::MaybeUninit;

use crate::device::fdt::{FdtManager, init_fdt, relocate_fdt};
use crate::environment::{PAGE_SIZE, SCARLET_HHDM_BASE};
use crate::mem::init_bss;
use crate::vm::addr::{PhysAddr, VirtAddr, init_boot_addressing, phys_to_virt};
use crate::vm::direct_map::DirectMapRegions;
use crate::vm::direct_map::DirectMapWindow;
use crate::vm::vmem::{MemoryAttribute, PhysicalMemoryArea};
use crate::{BootInfo, DeviceSource, start_kernel};

const FDT_MAGIC: u32 = 0xd00d_feed;
const MAX_FDT_SIZE: usize = 2 * 1024 * 1024;
const MAX_EARLY_RESERVED_AREAS: usize = 64;

static mut EARLY_BOOTINFO: MaybeUninit<BootInfo> = MaybeUninit::uninit();

unsafe extern "C" {
    static __KERNEL_SPACE_START: usize;
    static __KERNEL_SPACE_END: usize;
    static __FDT_RESERVED_START: usize;
}

#[repr(C, align(16))]
struct BootStack([u8; 64 * 1024]);

#[unsafe(link_section = ".data.boot_stack")]
#[unsafe(export_name = "BOOT_STACK")]
static mut BOOT_STACK: BootStack = BootStack([0xa5; 64 * 1024]);

/// Linux arm64 Image header and entry branch.
///
/// The linker resolves the effective image size. The page-size flag declares
/// a 4 KiB kernel. The current physical-link BSP keeps the placement flag clear
/// and must be loaded at the address selected by its linker script.
#[unsafe(link_section = ".head.text.header")]
#[unsafe(export_name = "_head")]
#[unsafe(naked)]
pub extern "C" fn image_head() -> ! {
    naked_asm!(
        "b _linux_image_entry",
        ".word 0",
        ".quad 0x200000",
        ".quad __KERNEL_IMAGE_SIZE",
        ".quad 0x2",
        ".quad 0",
        ".quad 0",
        ".quad 0",
        ".word 0x644d5241",
        ".word 0",
    );
}

/// Raw Linux arm64 entry stub.
///
/// x0 carries the physical DTB address. x1-x3 are reserved by the protocol.
/// The bootloader has already disabled the MMU and masked all exceptions.
#[unsafe(link_section = ".head.text.entry")]
#[unsafe(export_name = "_linux_image_entry")]
#[unsafe(naked)]
pub extern "C" fn image_entry() -> ! {
    naked_asm!(
        "adrp x1, BOOT_STACK",
        "add x1, x1, :lo12:BOOT_STACK",
        "add x1, x1, {boot_stack_size}",
        "adrp x2, {rust_entry}",
        "add x2, x2, :lo12:{rust_entry}",
        "b {prepare}",
        boot_stack_size = const 64 * 1024,
        rust_entry = sym linux_image_entry,
        prepare = sym prepare_el1_entry,
    );
}

/// Establish the same EL1 runtime for the Image entry and PSCI CPU_ON entry.
///
/// No memory is accessed until the caller's CPU-private stack is installed.
/// x0 carries the continuation argument, x1 its stack top, and x2 its address.
#[unsafe(naked)]
extern "C" fn prepare_el1_entry(_argument: usize, _stack: usize, _continuation: usize) -> ! {
    naked_asm!(
        "msr daifset, #0xf",
        "mov x19, x0",
        "mov x20, x1",
        "mov x21, x2",
        "mrs x3, CurrentEL",
        "lsr x3, x3, #2",
        "cmp x3, #1",
        "b.eq 1f",
        "cmp x3, #2",
        "b.ne 2f",
        "mov x3, #(1 << 31)",
        "msr hcr_el2, x3",
        "isb",
        "mov x3, #3",
        "msr cnthctl_el2, x3",
        "msr cntvoff_el2, xzr",
        "mov x3, #2",
        "msr cntp_ctl_el0, x3",
        "mov x3, #0x33ff",
        "msr cptr_el2, x3",
        "msr hstr_el2, xzr",
        "msr mdcr_el2, xzr",
        "msr vttbr_el2, xzr",
        "movz x3, #0x0800",
        "movk x3, #0x30d0, lsl #16",
        "msr sctlr_el1, x3",
        "mov x3, #0x3c5",
        "msr spsr_el2, x3",
        "adr x3, 1f",
        "msr elr_el2, x3",
        "eret",
        "1:",
        // Establish SCTLR_EL1's architectural RES1 baseline without enabling
        // the MMU or caches; do not inherit unknown firmware policy bits.
        "movz x3, #0x0800",
        "movk x3, #0x30d0, lsl #16",
        "msr sctlr_el1, x3",
        "isb",
        "msr spsel, #1",
        "and sp, x20, #~0xf",
        "movz x3, #0x30, lsl #16",
        "msr cpacr_el1, x3",
        "msr tpidr_el1, xzr",
        // Firmware timer compares must not fire while the AP installs its
        // translation regime, vectors and banked GIC interrupt state.
        "mov x3, #2",
        "msr cntv_ctl_el0, x3",
        "isb",
        "mov x0, x19",
        "br x21",
        "2:",
        "wfe",
        "b 2b",
    );
}

/// Enters Scarlet from the standard Linux arm64 Image register contract.
///
/// # Arguments
///
/// * `dtb_paddr` - Physical address passed in x0 by the bootloader.
///
/// # Returns
///
/// This function never returns.
pub extern "C" fn linux_image_entry(dtb_paddr: usize) -> ! {
    init_bss();
    validate_dtb(dtb_paddr);

    // SAFETY: validate_dtb checked the fixed header and total size. The Linux
    // boot contract keeps the DTB in accessible system RAM.
    let early_fdt = unsafe { fdt::Fdt::from_ptr(dtb_paddr as *const u8) }
        .unwrap_or_else(|error| panic!("Linux boot FDT parse failed: {:?}", error));
    let kernel_area = linked_kernel_area();
    let dtb_area = PhysicalMemoryArea::new(
        dtb_paddr as u64,
        (dtb_paddr as u64)
            .checked_add(early_fdt.total_size() as u64 - 1)
            .expect("Linux boot FDT range overflows"),
    );
    let original_initramfs = initramfs_area(&early_fdt);
    let early_framebuffer =
        framebuffer::BootFramebuffer::parse(&early_fdt, kernel_area, dtb_area, original_initramfs);
    let (direct_map_regions, usable_memory, early_uart) =
        build_memory_map(&early_fdt, early_framebuffer.as_ref())
            .unwrap_or_else(|error| panic!("Linux boot memory map: {}", error));
    let direct_map_bounds = direct_map_regions
        .bounding_area()
        .expect("Linux boot direct map must not be empty");

    let boot_direct_map = DirectMapWindow::from_offset(SCARLET_HHDM_BASE, direct_map_bounds)
        .expect("invalid Linux boot direct-map window");
    page_table::install(
        boot_direct_map,
        &direct_map_regions,
        kernel_area,
        dtb_area,
        original_initramfs,
    )
    .unwrap_or_else(|error| panic!("Linux boot page-table setup: {}", error));
    if let Some(uart_paddr) = early_uart {
        crate::arch::aarch64::earlycon::register_linux_boot_pl011(uart_paddr, boot_direct_map);
    }

    init_boot_addressing(
        boot_direct_map,
        PhysAddr::new(kernel_area.start),
        VirtAddr::new(
            usize::try_from(kernel_area.start)
                .expect("identity kernel address exceeds pointer width"),
        ),
        kernel_area.size(),
    );

    // Formatting and FDT initialization both acquire IRQ/preemption guards.
    // Publish the boot CPU's per-CPU identity before either path can log.
    crate::arch::aarch64::init_arch(0);
    if let Some(fb) = early_framebuffer {
        let vaddr = boot_direct_map
            .phys_to_virt(PhysAddr::new(fb.paddr))
            .expect("early framebuffer is outside the direct map")
            .as_usize();
        crate::earlyfb::init_linux_framebuffer(
            vaddr, fb.width, fb.height, fb.stride, fb.red_low, fb.rotated,
        );
        crate::println!(
            "[linux-boot] framebuffer console active; {}x{} stride={} rotation={}",
            fb.width,
            fb.height,
            fb.stride,
            if fb.rotated { 3 } else { 0 }
        );
    }
    crate::println!(
        "[linux-boot] temporary identity/HHDM page table active; DTB at {:#x}",
        dtb_paddr
    );
    init_fdt(dtb_paddr);

    let fdt_destination_paddr = unsafe { &__FDT_RESERVED_START as *const usize as usize as u64 };
    let relocated_fdt = relocate_fdt(phys_to_virt(fdt_destination_paddr) as *mut u8);
    let relocated_fdt_end = align_up(
        fdt_destination_paddr
            .checked_add(relocated_fdt.size() as u64)
            .expect("relocated FDT range overflows"),
        PAGE_SIZE as u64,
    );
    assert!(
        relocated_fdt_end <= kernel_area.end + 1,
        "relocated FDT exceeds the linker-reserved kernel buffer"
    );
    let mut usable_memory = usable_memory;

    let initramfs_paddr = if fdt_manager_has_initramfs() {
        Some(
            crate::fs::vfs_v2::drivers::initramfs::relocate_initramfs(&mut usable_memory)
                .unwrap_or_else(|error| panic!("Linux boot initramfs relocation: {}", error)),
        )
    } else {
        None
    };
    let fdt_manager = FdtManager::get_manager();
    let cmdline = fdt_manager
        .get_fdt()
        .and_then(|fdt| fdt.chosen().bootargs());

    let cpu_count = smp::initialize(
        fdt_manager
            .get_fdt()
            .expect("relocated FDT must be available"),
        cmdline.unwrap_or(""),
    );
    let bootinfo = BootInfo::new(
        0,
        cpu_count,
        usable_memory,
        direct_map_regions,
        initramfs_paddr,
        boot_direct_map,
        cmdline,
        DeviceSource::Fdt(fdt_destination_paddr),
        None,
        (cpu_count > 1).then_some(smp::start_secondary_cpus as fn()),
    )
    .with_probe_secondary_cpus_hook(smp::probe_secondary_cpus);
    crate::arch::init_user_context_from_fdt();

    // SAFETY: The boot CPU owns this static handoff slot. Its stack and the
    // referenced BootInfo remain mapped until start_kernel installs Scarlet's
    // allocator-backed page table.
    unsafe {
        (&raw mut EARLY_BOOTINFO).write(MaybeUninit::new(bootinfo));
        let bootinfo_ptr = (&raw const EARLY_BOOTINFO).cast::<BootInfo>();
        start_kernel(&*bootinfo_ptr)
    }
}

fn fdt_manager_has_initramfs() -> bool {
    FdtManager::get_manager().get_initramfs().is_some()
}

fn validate_dtb(dtb_paddr: usize) {
    if dtb_paddr == 0 {
        panic!("Linux arm64 boot protocol supplied a null DTB pointer");
    }
    if dtb_paddr & 7 != 0 {
        panic!("Linux arm64 DTB pointer is not 8-byte aligned");
    }

    // SAFETY: The boot contract guarantees that x0 references an accessible
    // FDT header. Only the fixed header words are read before full parsing.
    let magic = unsafe { (dtb_paddr as *const u32).read_volatile() };
    if u32::from_be(magic) != FDT_MAGIC {
        panic!("Linux arm64 DTB has an invalid magic value");
    }
    let raw_size = unsafe { ((dtb_paddr + 4) as *const u32).read_volatile() };
    let size = u32::from_be(raw_size) as usize;
    if size < 40 || size > MAX_FDT_SIZE {
        panic!("Linux arm64 DTB size is outside the protocol bounds");
    }
}

fn build_memory_map(
    fdt: &fdt::Fdt<'_>,
    framebuffer: Option<&framebuffer::BootFramebuffer>,
) -> Result<(DirectMapRegions, PhysicalMemoryArea, Option<usize>), &'static str> {
    let mut regions = DirectMapRegions::new();
    let mut best_usable: Option<PhysicalMemoryArea> = None;

    for node in fdt.all_nodes() {
        let is_memory = node.name == "memory"
            || node.name.starts_with("memory@")
            || node
                .property("device_type")
                .and_then(|property| property.as_str())
                == Some("memory");
        if !is_memory {
            continue;
        }
        let Some(node_regions) = node.reg() else {
            continue;
        };
        for region in node_regions {
            let Some(size) = region.size else {
                continue;
            };
            if size == 0 {
                continue;
            }
            let start = region.starting_address as usize as u64;
            let end = start
                .checked_add(size as u64 - 1)
                .ok_or("FDT RAM region overflows")?;
            let area = PhysicalMemoryArea::new(start, end);
            regions.insert(area, MemoryAttribute::Normal)?;

            let candidate = largest_usable_area(fdt, area, framebuffer.map(|fb| fb.area));
            best_usable = match (best_usable, candidate) {
                (Some(current), Some(next)) if next.size() > current.size() => Some(next),
                (None, Some(next)) => Some(next),
                (current, _) => current,
            };
        }
    }

    let usable = best_usable.ok_or("no usable FDT RAM remains after the kernel image")?;
    if let Some(fb) = framebuffer {
        if regions.contains_area_with_attribute(fb.area, MemoryAttribute::Normal) {
            regions.retag(fb.area, MemoryAttribute::NonCacheable)?;
        } else {
            regions.insert(fb.area, MemoryAttribute::NonCacheable)?;
        }
    }
    let early_uart = fdt
        .chosen()
        .stdout()
        .filter(is_pl011)
        .and_then(|node| node.reg())
        .and_then(|mut regs| regs.next())
        .map(|region| region.starting_address as usize)
        .or_else(|| {
            fdt.all_nodes()
                .find(is_pl011)
                .and_then(|node| node.reg())
                .and_then(|mut regs| regs.next())
                .map(|region| region.starting_address as usize)
        });
    if let Some(paddr) = early_uart {
        regions.insert(
            PhysicalMemoryArea::new(paddr as u64, paddr as u64 + PAGE_SIZE as u64 - 1),
            MemoryAttribute::Device,
        )?;
    }

    Ok((regions, usable, early_uart))
}

fn largest_usable_area(
    fdt: &fdt::Fdt<'_>,
    ram: PhysicalMemoryArea,
    framebuffer: Option<PhysicalMemoryArea>,
) -> Option<PhysicalMemoryArea> {
    let mut reserved = [None; MAX_EARLY_RESERVED_AREAS];
    let mut reserved_len = 0;

    push_reserved(&mut reserved, &mut reserved_len, linked_kernel_area())?;
    if let Some(area) = framebuffer {
        push_reserved(&mut reserved, &mut reserved_len, area)?;
    }
    for reservation in fdt.memory_reservations() {
        let size = reservation.size();
        if size == 0 {
            continue;
        }
        let reserved_start = reservation.address() as usize as u64;
        let reserved_end = reserved_start.checked_add(size as u64 - 1)?;
        push_reserved(
            &mut reserved,
            &mut reserved_len,
            PhysicalMemoryArea::new(reserved_start, reserved_end),
        )?;
    }
    if let Some(initramfs) = initramfs_area(fdt) {
        push_reserved(&mut reserved, &mut reserved_len, initramfs)?;
    }
    if let Some(reserved_memory) = fdt.find_node("/reserved-memory") {
        for node in reserved_memory.children() {
            let Some(regions) = node.reg() else {
                continue;
            };
            for region in regions {
                let Some(size) = region.size else {
                    continue;
                };
                if size == 0 {
                    continue;
                }
                let start = region.starting_address as usize as u64;
                let end = start.checked_add(size as u64 - 1)?;
                push_reserved(
                    &mut reserved,
                    &mut reserved_len,
                    PhysicalMemoryArea::new(start, end),
                )?;
            }
        }
    }

    reserved[..reserved_len].sort_unstable_by_key(|area| area.expect("reserved slot").start);
    let mut cursor = align_up(ram.start, PAGE_SIZE as u64);
    let ram_end_exclusive = align_down(ram.end.checked_add(1)?, PAGE_SIZE as u64);
    let mut best = None;
    for area in reserved[..reserved_len].iter().flatten().copied() {
        if area.end < ram.start || area.start > ram.end {
            continue;
        }
        let reserved_start = align_down(area.start.max(ram.start), PAGE_SIZE as u64);
        let reserved_end_exclusive =
            align_up(area.end.min(ram.end).checked_add(1)?, PAGE_SIZE as u64);
        if cursor < reserved_start {
            choose_larger(
                &mut best,
                PhysicalMemoryArea::new(cursor, reserved_start.checked_sub(1)?),
            );
        }
        cursor = cursor.max(reserved_end_exclusive);
        if cursor >= ram_end_exclusive {
            break;
        }
    }
    if cursor < ram_end_exclusive {
        choose_larger(
            &mut best,
            PhysicalMemoryArea::new(cursor, ram_end_exclusive.checked_sub(1)?),
        );
    }
    best
}

fn linked_kernel_area() -> PhysicalMemoryArea {
    let start = unsafe { &__KERNEL_SPACE_START as *const usize as usize as u64 };
    let end_exclusive = unsafe { &__KERNEL_SPACE_END as *const usize as usize as u64 };
    PhysicalMemoryArea::new(
        start,
        end_exclusive
            .checked_sub(1)
            .expect("linked kernel range is empty"),
    )
}

fn push_reserved(
    reserved: &mut [Option<PhysicalMemoryArea>; MAX_EARLY_RESERVED_AREAS],
    len: &mut usize,
    area: PhysicalMemoryArea,
) -> Option<()> {
    if *len == reserved.len() {
        return None;
    }
    reserved[*len] = Some(area);
    *len += 1;
    Some(())
}

fn choose_larger(best: &mut Option<PhysicalMemoryArea>, candidate: PhysicalMemoryArea) {
    if best
        .map(|area| candidate.size() > area.size())
        .unwrap_or(true)
    {
        *best = Some(candidate);
    }
}

fn initramfs_area(fdt: &fdt::Fdt<'_>) -> Option<PhysicalMemoryArea> {
    let chosen = fdt.find_node("/chosen")?;
    let start = chosen
        .property("linux,initrd-start")
        .or_else(|| chosen.property("initrd-start"))
        .and_then(property_address)?;
    let end = chosen
        .property("linux,initrd-end")
        .or_else(|| chosen.property("initrd-end"))
        .and_then(property_address)?;
    (end > start).then_some(PhysicalMemoryArea::new(start, end - 1))
}

fn property_address(property: fdt::node::NodeProperty<'_>) -> Option<u64> {
    match property.value.len() {
        4 => Some(u32::from_be_bytes(property.value.try_into().ok()?) as u64),
        8 => Some(u64::from_be_bytes(property.value.try_into().ok()?) as u64),
        _ => None,
    }
}

fn is_pl011(node: &fdt::node::FdtNode<'_, '_>) -> bool {
    node.compatible()
        .map(|compatible| compatible.all().any(|value| value == "arm,pl011"))
        .unwrap_or(false)
}

const fn align_up(value: u64, alignment: u64) -> u64 {
    (value + alignment - 1) & !(alignment - 1)
}

const fn align_down(value: u64, alignment: u64) -> u64 {
    value & !(alignment - 1)
}
