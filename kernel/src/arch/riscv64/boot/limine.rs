use core::arch::naked_asm;
use core::mem::{MaybeUninit, offset_of};

use limine::mp::MpInfo;

use crate::boot::limine::{
    DTB_REQUEST, EXECUTABLE_ADDRESS_REQUEST, HHDM_REQUEST, MEMMAP_REQUEST, MODULE_REQUEST,
    MP_REQUEST, boot_cmdline, bootloader_hhdm_physical_bound, ensure_base_revision_supported,
    module_area, reserve_front, response, runtime_direct_map_regions, select_usable_region,
    usable_memory_regions,
};
use crate::device::fdt::{FdtManager, init_fdt, relocate_fdt};
use crate::environment::STACK_SIZE;
use crate::mem::{KERNEL_STACK, init_bss};
use crate::vm::addr::{init_bootloader_direct_map_bound, init_limine_addressing, phys_to_virt};
use crate::{BootInfo, DeviceSource, println, start_ap, start_kernel, wait_for_ap_release};
use limine::paging;
use limine::request::{BspHartidRequest, PagingModeRequest};

static mut EARLY_BOOTINFO: MaybeUninit<BootInfo> = MaybeUninit::uninit();

#[unsafe(link_section = ".limine_requests")]
#[used]
static RISCV_BSP_HARTID_REQUEST: BspHartidRequest = BspHartidRequest::new();

#[unsafe(link_section = ".limine_requests")]
#[used]
static PAGING_MODE_REQUEST: PagingModeRequest = PagingModeRequest::new(
    paging::PagingMode::RISCV_SV48,
    paging::PagingMode::RISCV_SV48,
    paging::PagingMode::RISCV_SV48,
);

#[unsafe(naked)]
unsafe extern "C" fn limine_ap_entry(_info: &MpInfo) -> ! {
    naked_asm!(
        ".option push",
        ".option norelax",
        ".option arch, +m",
        "csrci sstatus, 0x2",
        "csrw sscratch, zero",
        "ld a0, {hartid_offset}(a0)",
        // Limine's stack is not mapped by Scarlet's runtime page table.
        // Select this hart's permanent stack before entering any Rust frame.
        "la t0, {kernel_stack}",
        "li t1, {stack_size}",
        "addi t2, a0, 1",
        "mul t1, t1, t2",
        "add sp, t0, t1",
        "tail {ap_wait}",
        ".option pop",
        hartid_offset = const offset_of!(MpInfo, hartid),
        kernel_stack = sym KERNEL_STACK,
        stack_size = const STACK_SIZE,
        ap_wait = sym secondary_cpu_entry,
    );
}

extern "C" fn secondary_cpu_entry(cpu_id: usize) -> ! {
    wait_for_ap_release();
    start_ap(cpu_id)
}

fn start_secondary_cpus() {
    crate::release_aps();
}

fn bootstrap_aps() {
    let mp_resp = match MP_REQUEST.response() {
        Some(resp) => resp,
        None => {
            println!("[riscv64] No Limine MP response, single-CPU mode");
            return;
        }
    };

    let bsp_hartid = mp_resp.bsp_hartid;
    println!(
        "[riscv64] BSP hart={}, {} CPU(s) detected by Limine",
        bsp_hartid,
        mp_resp.cpus().len()
    );

    for cpu in mp_resp.cpus() {
        if cpu.hartid == bsp_hartid {
            continue;
        }
        println!("[riscv64] Bootstrapping hart {}...", cpu.hartid);
        cpu.bootstrap(limine_ap_entry, cpu.hartid);
    }
}

#[unsafe(no_mangle)]
pub fn limine_entry() -> ! {
    // SAFETY: sscratch holds whatever firmware left; explicitly clear it so
    // try_get_cpuid() can deterministically treat 0 as "uninitialized"
    // until init_cpu publishes the per-CPU pointer.
    unsafe {
        core::arch::asm!("csrw sscratch, zero");
    }
    init_bss();

    let bsp = response(RISCV_BSP_HARTID_REQUEST.response(), "riscv-bsp-hartid");
    crate::arch::riscv64::boot::init_cpu(bsp.bsp_hartid as usize);

    let hhdm = response(HHDM_REQUEST.response(), "hhdm");
    let executable = response(EXECUTABLE_ADDRESS_REQUEST.response(), "executable-address");
    let memmap = response(MEMMAP_REQUEST.response(), "memmap");
    let dtb = response(DTB_REQUEST.response(), "dtb");

    ensure_base_revision_supported();

    unsafe extern "C" {
        static __KERNEL_SPACE_START: usize;
        static __KERNEL_SPACE_END: usize;
    }

    let kernel_start = unsafe { &__KERNEL_SPACE_START as *const usize as usize };
    let kernel_end = unsafe { &__KERNEL_SPACE_END as *const usize as usize };
    init_limine_addressing(
        hhdm.offset as usize,
        executable.physical_base as usize,
        executable.virtual_base as usize,
        kernel_end - kernel_start,
    );

    if executable.virtual_base as usize != kernel_start {
        panic!(
            "kernel virtual base mismatch: limine={:#x} linker={:#x}",
            executable.virtual_base, kernel_start
        );
    }

    init_fdt(dtb.dtb_ptr as usize);

    let usable_region = select_usable_region(memmap.entries());
    let bootloader_hhdm_bound = bootloader_hhdm_physical_bound(memmap.entries());
    init_bootloader_direct_map_bound(bootloader_hhdm_bound.start, bootloader_hhdm_bound.end);
    let hhdm_offset = hhdm.offset as usize;
    let direct_map_regions = runtime_direct_map_regions(memmap.entries(), None)
        .unwrap_or_else(|error| panic!("failed to build runtime direct map: {}", error));
    let relocated_fdt = relocate_fdt(phys_to_virt(usable_region.start) as *mut u8);
    let relocated_fdt_paddr = usable_region.start;
    let reserved_bytes = relocated_fdt.size();
    let usable_memory_paddr = reserve_front(usable_region, reserved_bytes);
    let initramfs_paddr = module_area(MODULE_REQUEST.response());
    let fdt_manager = FdtManager::get_manager();
    let cpu_count = fdt_manager.get_cpu_count().unwrap_or(1);
    let fdt_cmdline = fdt_manager
        .get_fdt()
        .and_then(|fdt| fdt.chosen().bootargs());
    let cmdline = boot_cmdline(fdt_cmdline);
    // Cache the wall-clock epoch now; the Limine response pointer is invalid
    // after the page-table switch in start_kernel.
    crate::boot::limine::capture_date_at_boot();
    let usable_memory_regions =
        usable_memory_regions(memmap.entries(), usable_region, usable_memory_paddr, None)
            .unwrap_or_else(|error| panic!("failed to build PMM memory regions: {}", error));
    let bootinfo = BootInfo::new(
        bsp.bsp_hartid as usize,
        cpu_count,
        usable_memory_paddr,
        direct_map_regions,
        initramfs_paddr,
        hhdm_offset,
        cmdline,
        DeviceSource::Fdt(relocated_fdt_paddr),
        None,
        Some(start_secondary_cpus),
    )
    .with_usable_memory_regions(usable_memory_regions);

    crate::arch::init_user_context_from_fdt();
    bootstrap_aps();

    unsafe {
        // The BSP may be any hart. Do not share hart 0's stack with AP 0.
        let stack_top = (&raw const KERNEL_STACK) as *const _ as usize
            + STACK_SIZE * (bsp.bsp_hartid as usize + 1);
        (&raw mut EARLY_BOOTINFO).write(MaybeUninit::new(bootinfo));
        let bootinfo_ptr = (&raw const EARLY_BOOTINFO).cast::<BootInfo>();
        crate::arch::riscv64::switch_stack_and_jump(
            start_kernel as *const () as usize,
            bootinfo_ptr as usize,
            stack_top,
        )
    }
}
