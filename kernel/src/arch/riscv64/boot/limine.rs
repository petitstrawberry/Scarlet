use core::arch::naked_asm;

use crate::environment::STACK_SIZE;

use limine::paging;
use limine::request::{BspHartidRequest, PagingModeRequest};

use crate::boot::limine::{
    DTB_REQUEST, EXECUTABLE_ADDRESS_REQUEST, HHDM_REQUEST, MEMMAP_REQUEST, MODULE_REQUEST,
    ensure_base_revision_supported, module_area, reserve_front, response, select_usable_region,
};
use crate::device::fdt::{FdtManager, init_fdt, relocate_fdt};
use crate::mem::{KERNEL_STACK, init_bss};
use crate::vm::addr::{init_limine_addressing, phys_to_virt};
use crate::vm::vmem::MemoryArea;
use crate::{BootInfo, DeviceSource, start_kernel};

static mut EARLY_BOOTINFO: Option<BootInfo> = None;

#[unsafe(link_section = ".limine_requests")]
#[used]
static RISCV_BSP_HARTID_REQUEST: BspHartidRequest = BspHartidRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
static PAGING_MODE_REQUEST: PagingModeRequest = PagingModeRequest::new()
    .with_mode(paging::Mode::SV48)
    .with_max_mode(paging::Mode::SV48)
    .with_min_mode(paging::Mode::SV48);

#[unsafe(no_mangle)]
pub fn limine_entry() -> ! {
    init_bss();

    let hhdm = response(HHDM_REQUEST.get_response(), "hhdm");
    let executable = response(
        EXECUTABLE_ADDRESS_REQUEST.get_response(),
        "executable-address",
    );
    let memmap = response(MEMMAP_REQUEST.get_response(), "memmap");
    let dtb = response(DTB_REQUEST.get_response(), "dtb");
    let bsp = response(RISCV_BSP_HARTID_REQUEST.get_response(), "riscv-bsp-hartid");

    ensure_base_revision_supported();

    unsafe extern "C" {
        static __KERNEL_SPACE_START: usize;
        static __KERNEL_SPACE_END: usize;
    }

    let kernel_start = unsafe { &__KERNEL_SPACE_START as *const usize as usize };
    let kernel_end = unsafe { &__KERNEL_SPACE_END as *const usize as usize };
    init_limine_addressing(
        hhdm.offset() as usize,
        executable.physical_base() as usize,
        executable.virtual_base() as usize,
        kernel_end - kernel_start,
    );

    if executable.virtual_base() as usize != kernel_start {
        panic!(
            "kernel virtual base mismatch: limine={:#x} linker={:#x}",
            executable.virtual_base(),
            kernel_start
        );
    }

    init_fdt(dtb.dtb_ptr() as usize);

    let usable_region = select_usable_region(memmap.entries());
    let relocated_fdt = relocate_fdt(phys_to_virt(usable_region.start) as *mut u8);
    let reserved_bytes = relocated_fdt.size();
    let usable_memory_phys = reserve_front(usable_region, reserved_bytes);
    let usable_memory = MemoryArea::new(
        phys_to_virt(usable_memory_phys.start),
        phys_to_virt(usable_memory_phys.end),
    );
    let direct_map_area = MemoryArea::new(
        phys_to_virt(usable_region.start),
        phys_to_virt(usable_region.end),
    );
    let initramfs_phys = module_area(MODULE_REQUEST.get_response());
    let initramfs = initramfs_phys
        .map(|area| MemoryArea::new(phys_to_virt(area.start), phys_to_virt(area.end)));
    let fdt_manager = FdtManager::get_manager();
    let cpu_count = fdt_manager.get_cpu_count().unwrap_or(1);
    let cmdline = fdt_manager
        .get_fdt()
        .and_then(|fdt| fdt.chosen().bootargs());

    crate::early_println!(
        "[limine] bootinfo usable_memory={:#x}..={:#x}",
        usable_memory.start,
        usable_memory.end
    );
    crate::early_println!("[limine] before init_user_context_from_fdt");
    let bootinfo = BootInfo::new(
        bsp.bsp_hartid() as usize,
        cpu_count,
        usable_memory,
        direct_map_area,
        usable_memory_phys,
        usable_region,
        initramfs,
        initramfs_phys,
        cmdline,
        DeviceSource::Fdt(relocated_fdt.start),
    );

    crate::arch::init_user_context_from_fdt();
    crate::early_println!("[limine] before init_boot_cpu");
    crate::arch::riscv64::boot::init_boot_cpu(bootinfo.cpu_id);
    crate::early_println!("[limine] before stack handoff");

    unsafe {
        let stack_top = (&raw const KERNEL_STACK) as *const _ as usize + STACK_SIZE;
        EARLY_BOOTINFO = Some(bootinfo);
        let bootinfo_ptr =
            (&raw const EARLY_BOOTINFO) as *const Option<BootInfo> as *const BootInfo;
        crate::arch::riscv64::switch_stack_and_jump(
            start_kernel as *const () as usize,
            bootinfo_ptr as usize,
            stack_top,
        )
    }
}
