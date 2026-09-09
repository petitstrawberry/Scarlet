//! SBI/FDT entry for an RV32 supervisor kernel with Sv32 paging.

mod page_table;

use core::arch::global_asm;
use core::mem::MaybeUninit;

use crate::boot::fdt_memory::FdtMemory;
use crate::environment::{MAX_NUM_CPUS, STACK_SIZE};
use crate::vm::addr::{PhysAddr, VirtAddr, init_boot_addressing};
use crate::vm::direct_map::DirectMapWindow;
use crate::vm::vmem::PhysicalMemoryArea;
use crate::{BootInfo, DeviceSource};

global_asm!(include_str!("entry.S"), stack_size = const STACK_SIZE, max_cpus = const MAX_NUM_CPUS,
    rust_entry = sym boot_entry, secondary_entry = sym secondary_entry);

unsafe extern "C" {
    /// Physical-entry code; the BSP linker supplies its physical ELF entry.
    pub fn _sbi_start() -> !;
    fn _sbi_start_secondary() -> !;
}

static mut EARLY_BOOTINFO: MaybeUninit<BootInfo> = MaybeUninit::uninit();

/// Enter Rust only after the image and stack have their linked virtual aliases.
extern "C" fn boot_entry(hart_id: usize, dtb_va: usize, dtb_pa: usize, image_pa: usize) -> ! {
    crate::mem::init_bss();
    super::init_cpu(0);
    crate::println!(
        "[sbi-boot] RV32 hart={} image PA={:#x}; Sv32 image mapping active",
        hart_id,
        image_pa
    );

    let image_start = (&raw const crate::mem::__KERNEL_SPACE_START) as usize;
    let image_end = (&raw const crate::mem::__KERNEL_SPACE_END) as usize;
    let image_size = image_end
        .checked_sub(image_start)
        .expect("invalid linked image");
    let image_area =
        PhysicalMemoryArea::new(image_pa as u64, image_pa as u64 + image_size as u64 - 1);

    assert!(
        dtb_pa != 0 && dtb_pa & 7 == 0,
        "invalid FDT physical pointer"
    );
    // SAFETY: The firmware supplies an accessible FDT header. Entry assembly
    // established an 8 MiB read-only window at the physical superpage boundary.
    let header = unsafe { core::slice::from_raw_parts(dtb_va as *const u8, 40) };
    assert_eq!(&header[..4], &[0xd0, 0x0d, 0xfe, 0xed], "invalid FDT magic");
    let size = u32::from_be_bytes(header[4..8].try_into().unwrap()) as usize;
    let destination = (&raw const crate::mem::__FDT_RESERVED_START) as usize;
    let destination_end = (&raw const crate::mem::__FDT_RESERVED_END) as usize;
    assert!(
        (40..=destination_end - destination).contains(&size),
        "FDT exceeds reserved image storage"
    );
    let dtb_end = dtb_pa
        .checked_add(size - 1)
        .expect("FDT physical range exceeds entry address width");
    let dtb_area = PhysicalMemoryArea::new(dtb_pa as u64, dtb_end as u64);
    assert!(
        dtb_area.end < image_area.start || dtb_area.start > image_area.end,
        "firmware FDT overlaps the loaded image"
    );
    // SAFETY: The whole bounded FDT is covered by the temporary window, and the
    // destination is disjoint, linker-reserved, writable kernel-image memory.
    unsafe {
        core::ptr::copy_nonoverlapping(dtb_va as *const u8, destination as *mut u8, size);
    }
    let blob = unsafe { core::slice::from_raw_parts(destination as *const u8, size) };
    let memory = FdtMemory::parse(blob, image_area, dtb_area).expect("invalid SBI boot memory map");
    let window = DirectMapWindow::for_kernel(&memory.direct_map, memory.initramfs)
        .expect("RAM cannot fit the kernel direct-map window");
    init_boot_addressing(
        window,
        PhysAddr::new(image_pa as u64),
        VirtAddr::new(image_start),
        image_size,
    );
    // SAFETY: Ordered SBI boot has released only this hart. The table storage
    // is image-owned and no direct-map reference has been accessed yet.
    unsafe {
        page_table::map_direct_map(window, &memory.direct_map).expect("SBI boot RAM mapping");
    }
    crate::device::fdt::init_fdt(destination);
    crate::arch::riscv::cpu::init_from_fdt(hart_id).expect("invalid SBI CPU inventory");
    crate::arch::init_user_context_from_fdt();
    let fdt = crate::device::fdt::FdtManager::get_manager()
        .get_fdt()
        .expect("boot FDT");
    let bootinfo = BootInfo::new(
        0,
        crate::arch::riscv::cpu::count(),
        memory.primary_usable(),
        memory.direct_map,
        memory.initramfs,
        window,
        fdt.chosen().bootargs(),
        DeviceSource::Fdt(image_pa as u64 + (destination - image_start) as u64),
        None,
        Some(start_secondary_cpus),
    )
    .with_usable_memory_regions(memory.usable);
    // SAFETY: Sole BSP publication; the image alias survives both page-table
    // handoffs, unlike firmware stacks or the temporary FDT window.
    unsafe {
        (&raw mut EARLY_BOOTINFO).write(MaybeUninit::new(bootinfo));
        crate::start_kernel(&*(&raw const EARLY_BOOTINFO).cast::<BootInfo>())
    }
}

fn start_secondary_cpus() {
    let entry = PhysAddr::new(crate::vm::addr::virt_to_phys(_sbi_start_secondary as usize));
    // Publish the complete boot root, CPU inventory and global kernel state
    // before the first target hart can enter supervisor mode.
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    for cpu_id in 1..crate::arch::riscv::cpu::count() {
        let hart = crate::arch::riscv::cpu::hart_id(cpu_id).expect("secondary hart ID");
        crate::arch::riscv::instruction::sbi::hart_start(hart, entry, cpu_id).unwrap_or_else(
            |error| panic!("SBI start CPU {} (hart {}): {:?}", cpu_id, hart, error),
        );
    }
}

extern "C" fn secondary_entry(hart_id: usize, cpu_id: usize) -> ! {
    assert_eq!(
        crate::arch::riscv::cpu::hart_id(cpu_id),
        Some(hart_id),
        "SBI secondary identity mismatch"
    );
    crate::start_ap(cpu_id)
}
