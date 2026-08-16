//! Linux arm64 Image boot protocol entry.
//!
//! The bootloader supplies only the standard register/FDT contract. Scarlet
//! builds its own temporary page table and HHDM before entering common kernel
//! initialization; no Limine response or bootloader-owned direct map is used.

mod page_table;

use core::arch::naked_asm;
use core::mem::MaybeUninit;

use crate::device::fdt::{FdtManager, init_fdt, relocate_fdt};
use crate::environment::{PAGE_SIZE, SCARLET_HHDM_BASE};
use crate::mem::init_bss;
use crate::vm::addr::{init_boot_addressing, init_bootloader_direct_map_bound, phys_to_virt};
use crate::vm::direct_map::DirectMapRegions;
use crate::vm::vmem::{MemoryArea, MemoryAttribute};
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
        "mov x19, x0",
        "msr daifset, #0xf",
        "mrs x1, CurrentEL",
        "lsr x1, x1, #2",
        "cmp x1, #1",
        "b.eq 1f",
        "cmp x1, #2",
        "b.ne 2f",
        "mov x1, #(1 << 31)",
        "msr hcr_el2, x1",
        "mov x1, #3",
        "msr cnthctl_el2, x1",
        "msr cntvoff_el2, xzr",
        "msr cptr_el2, xzr",
        "mov x1, #0x3c5",
        "msr spsr_el2, x1",
        "adr x1, 1f",
        "msr elr_el2, x1",
        "eret",
        "1:",
        // Establish SCTLR_EL1's architectural RES1 baseline without enabling
        // the MMU or caches; do not inherit unknown firmware policy bits.
        "movz x1, #0x0800",
        "movk x1, #0x30d0, lsl #16",
        "msr sctlr_el1, x1",
        "isb",
        "msr spsel, #1",
        "movz x1, #0x30, lsl #16",
        "msr cpacr_el1, x1",
        "msr tpidr_el1, xzr",
        "isb",
        "adrp x2, BOOT_STACK",
        "add x2, x2, :lo12:BOOT_STACK",
        "add x2, x2, {boot_stack_size}",
        "and sp, x2, #~0xf",
        "mov x0, x19",
        "b {rust_entry}",
        "2:",
        "wfe",
        "b 2b",
        boot_stack_size = const 64 * 1024,
        rust_entry = sym linux_image_entry,
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
    let dtb_area = MemoryArea::new(
        dtb_paddr,
        dtb_paddr
            .checked_add(early_fdt.total_size() - 1)
            .expect("Linux boot FDT range overflows"),
    );
    let original_initramfs = initramfs_area(&early_fdt);
    let (direct_map_regions, usable_memory, early_uart) = build_memory_map(&early_fdt)
        .unwrap_or_else(|error| panic!("Linux boot memory map: {}", error));
    let direct_map_bounds = direct_map_regions
        .bounding_area()
        .expect("Linux boot direct map must not be empty");

    page_table::install(
        &direct_map_regions,
        kernel_area,
        dtb_area,
        original_initramfs,
    )
    .unwrap_or_else(|error| panic!("Linux boot page-table setup: {}", error));
    if let Some(uart_paddr) = early_uart {
        crate::arch::aarch64::earlycon::register_linux_boot_pl011(uart_paddr);
    }

    init_boot_addressing(
        SCARLET_HHDM_BASE,
        kernel_area.start,
        kernel_area.start,
        kernel_area.size(),
    );
    init_bootloader_direct_map_bound(direct_map_bounds.start, direct_map_bounds.end);

    // Formatting and FDT initialization both acquire IRQ/preemption guards.
    // Publish the boot CPU's per-CPU identity before either path can log.
    crate::arch::aarch64::init_arch(0);
    crate::early_println!(
        "[linux-boot] temporary identity/HHDM page table active; DTB at {:#x}",
        dtb_paddr
    );
    init_fdt(dtb_paddr);

    let fdt_destination_paddr = unsafe { &__FDT_RESERVED_START as *const usize as usize };
    let relocated_fdt = relocate_fdt(phys_to_virt(fdt_destination_paddr) as *mut u8);
    let relocated_fdt_end = align_up(
        fdt_destination_paddr
            .checked_add(relocated_fdt.size())
            .expect("relocated FDT range overflows"),
        PAGE_SIZE,
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

    let bootinfo = BootInfo::new(
        0,
        1,
        usable_memory,
        direct_map_regions,
        initramfs_paddr,
        SCARLET_HHDM_BASE,
        cmdline,
        DeviceSource::Fdt(fdt_destination_paddr),
        None,
        None,
    );
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
) -> Result<(DirectMapRegions, MemoryArea, Option<usize>), &'static str> {
    let mut regions = DirectMapRegions::new();
    let mut best_usable: Option<MemoryArea> = None;

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
            let start = region.starting_address as usize;
            let end = start
                .checked_add(size - 1)
                .ok_or("FDT RAM region overflows")?;
            let area = MemoryArea::new(start, end);
            regions.insert(area, MemoryAttribute::Normal)?;

            let candidate = largest_usable_area(fdt, area);
            best_usable = match (best_usable, candidate) {
                (Some(current), Some(next)) if next.size() > current.size() => Some(next),
                (None, Some(next)) => Some(next),
                (current, _) => current,
            };
        }
    }

    let usable = best_usable.ok_or("no usable FDT RAM remains after the kernel image")?;
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
            MemoryArea::new(paddr, paddr + PAGE_SIZE - 1),
            MemoryAttribute::Device,
        )?;
    }

    Ok((regions, usable, early_uart))
}

fn largest_usable_area(fdt: &fdt::Fdt<'_>, ram: MemoryArea) -> Option<MemoryArea> {
    let mut reserved = [None; MAX_EARLY_RESERVED_AREAS];
    let mut reserved_len = 0;

    push_reserved(&mut reserved, &mut reserved_len, linked_kernel_area())?;
    for reservation in fdt.memory_reservations() {
        let size = reservation.size();
        if size == 0 {
            continue;
        }
        let reserved_start = reservation.address() as usize;
        let reserved_end = reserved_start.checked_add(size - 1)?;
        push_reserved(
            &mut reserved,
            &mut reserved_len,
            MemoryArea::new(reserved_start, reserved_end),
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
                let start = region.starting_address as usize;
                let end = start.checked_add(size - 1)?;
                push_reserved(
                    &mut reserved,
                    &mut reserved_len,
                    MemoryArea::new(start, end),
                )?;
            }
        }
    }

    reserved[..reserved_len].sort_unstable_by_key(|area| area.expect("reserved slot").start);
    let mut cursor = align_up(ram.start, PAGE_SIZE);
    let ram_end_exclusive = align_down(ram.end.checked_add(1)?, PAGE_SIZE);
    let mut best = None;
    for area in reserved[..reserved_len].iter().flatten().copied() {
        if area.end < ram.start || area.start > ram.end {
            continue;
        }
        let reserved_start = align_down(area.start.max(ram.start), PAGE_SIZE);
        let reserved_end_exclusive = align_up(area.end.min(ram.end).checked_add(1)?, PAGE_SIZE);
        if cursor < reserved_start {
            choose_larger(
                &mut best,
                MemoryArea::new(cursor, reserved_start.checked_sub(1)?),
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
            MemoryArea::new(cursor, ram_end_exclusive.checked_sub(1)?),
        );
    }
    best
}

fn linked_kernel_area() -> MemoryArea {
    let start = unsafe { &__KERNEL_SPACE_START as *const usize as usize };
    let end_exclusive = unsafe { &__KERNEL_SPACE_END as *const usize as usize };
    MemoryArea::new(
        start,
        end_exclusive
            .checked_sub(1)
            .expect("linked kernel range is empty"),
    )
}

fn push_reserved(
    reserved: &mut [Option<MemoryArea>; MAX_EARLY_RESERVED_AREAS],
    len: &mut usize,
    area: MemoryArea,
) -> Option<()> {
    if *len == reserved.len() {
        return None;
    }
    reserved[*len] = Some(area);
    *len += 1;
    Some(())
}

fn choose_larger(best: &mut Option<MemoryArea>, candidate: MemoryArea) {
    if best
        .map(|area| candidate.size() > area.size())
        .unwrap_or(true)
    {
        *best = Some(candidate);
    }
}

fn initramfs_area(fdt: &fdt::Fdt<'_>) -> Option<MemoryArea> {
    let chosen = fdt.find_node("/chosen")?;
    let start = chosen
        .property("linux,initrd-start")
        .or_else(|| chosen.property("initrd-start"))
        .and_then(property_address)?;
    let end = chosen
        .property("linux,initrd-end")
        .or_else(|| chosen.property("initrd-end"))
        .and_then(property_address)?;
    (end > start).then_some(MemoryArea::new(start, end - 1))
}

fn property_address(property: fdt::node::NodeProperty<'_>) -> Option<usize> {
    match property.value.len() {
        4 => Some(u32::from_be_bytes(property.value.try_into().ok()?) as usize),
        8 => Some(u64::from_be_bytes(property.value.try_into().ok()?) as usize),
        _ => None,
    }
}

fn is_pl011(node: &fdt::node::FdtNode<'_, '_>) -> bool {
    node.compatible()
        .map(|compatible| compatible.all().any(|value| value == "arm,pl011"))
        .unwrap_or(false)
}

const fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

const fn align_down(value: usize, alignment: usize) -> usize {
    value & !(alignment - 1)
}
