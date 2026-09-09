use core::arch::naked_asm;
use core::mem::MaybeUninit;

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
use crate::vm::addr::{PhysAddr, VirtAddr, init_boot_addressing, phys_to_virt};
use crate::vm::direct_map::DirectMapWindow;
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
        "call {logical_cpu_id}",
        // Limine's stack is not mapped by Scarlet's runtime page table.
        // The short ID accessor has returned; select the permanent stack
        // before waiting or entering Rust frames that span a page-table switch.
        "la t0, {kernel_stack}",
        "li t1, {stack_size}",
        "addi t2, a0, 1",
        "mul t1, t1, t2",
        "add sp, t0, t1",
        "tail {ap_wait}",
        ".option pop",
        logical_cpu_id = sym logical_cpu_for_ap,
        kernel_stack = sym KERNEL_STACK,
        stack_size = const STACK_SIZE,
        ap_wait = sym secondary_cpu_entry,
    );
}

extern "C" fn logical_cpu_for_ap(info: &MpInfo) -> usize {
    usize::try_from(info.extra_argument()).expect("Limine CPU slot exceeds pointer width")
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
        let cpu_id = crate::arch::riscv::cpu::logical_id(cpu.hartid as usize)
            .expect("Limine hart is absent from CPU inventory");
        cpu.bootstrap(limine_ap_entry, cpu_id as u64);
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
    crate::arch::riscv::boot::init_cpu(0);
    if let Some(mp) = MP_REQUEST.response() {
        crate::arch::riscv::cpu::init_harts(
            bsp.bsp_hartid as usize,
            mp.cpus().iter().map(|cpu| cpu.hartid as usize),
        )
        .expect("invalid Limine CPU inventory");
    } else {
        crate::arch::riscv::cpu::init_harts(
            bsp.bsp_hartid as usize,
            core::iter::once(bsp.bsp_hartid as usize),
        )
        .expect("invalid Limine boot hart");
    }

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
    let bootloader_hhdm_bound = bootloader_hhdm_physical_bound(memmap.entries());
    let boot_direct_map = DirectMapWindow::from_offset(
        usize::try_from(hhdm.offset).expect("Limine HHDM offset exceeds pointer width"),
        bootloader_hhdm_bound,
    )
    .expect("invalid Limine direct-map window");
    init_boot_addressing(
        boot_direct_map,
        PhysAddr::new(executable.physical_base),
        VirtAddr::new(
            usize::try_from(executable.virtual_base).expect("kernel VA exceeds pointer width"),
        ),
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
    let direct_map_regions = runtime_direct_map_regions(memmap.entries(), None)
        .unwrap_or_else(|error| panic!("failed to build runtime direct map: {}", error));
    let relocated_fdt = relocate_fdt(phys_to_virt(usable_region.start) as *mut u8);
    let relocated_fdt_paddr = usable_region.start;
    let reserved_bytes = relocated_fdt.size();
    let usable_memory_paddr = reserve_front(usable_region, reserved_bytes);
    let initramfs_paddr = module_area(MODULE_REQUEST.response());
    let fdt_manager = FdtManager::get_manager();
    let cpu_count = crate::arch::riscv::cpu::count();
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
        0,
        cpu_count,
        usable_memory_paddr,
        direct_map_regions,
        initramfs_paddr,
        boot_direct_map,
        cmdline,
        DeviceSource::Fdt(relocated_fdt_paddr),
        None,
        Some(start_secondary_cpus),
    )
    .with_usable_memory_regions(usable_memory_regions);

    crate::arch::init_user_context_from_fdt();
    bootstrap_aps();

    unsafe {
        // The boot hart owns logical slot 0 regardless of its physical ID.
        let stack_top = (&raw const KERNEL_STACK) as *const _ as usize + STACK_SIZE;
        (&raw mut EARLY_BOOTINFO).write(MaybeUninit::new(bootinfo));
        let bootinfo_ptr = (&raw const EARLY_BOOTINFO).cast::<BootInfo>();
        crate::arch::riscv::switch_stack_and_jump(
            start_kernel as *const () as usize,
            bootinfo_ptr as usize,
            stack_top,
        )
    }
}
