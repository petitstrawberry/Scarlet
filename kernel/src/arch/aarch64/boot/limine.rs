use core::arch::{asm, naked_asm};
use core::mem::MaybeUninit;

use limine::mp::MpInfo;

use crate::boot::limine::{
    DTB_REQUEST, EXECUTABLE_ADDRESS_REQUEST, FRAMEBUFFER_REQUEST, HHDM_REQUEST, MEMMAP_REQUEST,
    MODULE_REQUEST, MP_REQUEST, ensure_base_revision_supported, framebuffer_area,
    hhdm_physical_span, module_area, reserve_front, response, select_usable_region,
};
use crate::device::fdt::{FdtManager, init_fdt, relocate_fdt};
use crate::environment::STACK_SIZE;
use crate::mem::{KERNEL_STACK, init_bss};
use crate::vm::addr::{init_boot_direct_map_range, init_limine_addressing, phys_to_virt};
use crate::vm::vmem::MemoryArea;
use crate::{BootInfo, DeviceSource, early_println, start_ap, start_kernel, wait_for_ap_release};
use core::sync::atomic::compiler_fence;

static mut EARLY_BOOTINFO: MaybeUninit<BootInfo> = MaybeUninit::uninit();

static mut VHE_ENABLED: bool = false;
static mut HV_AVAILABLE: bool = false;

pub fn is_vhe_enabled() -> bool {
    unsafe { VHE_ENABLED }
}

pub fn is_hv_available() -> bool {
    unsafe { HV_AVAILABLE }
}

#[unsafe(naked)]
unsafe extern "C" fn limine_ap_entry(_info: &MpInfo) -> ! {
    naked_asm!(
        // x0 = &MpInfo (from Limine)
        "ldr x8, [x0, #8]",         // x8 = mp_info.mpidr (offset 8)
        "and x8, x8, #0xff",        // x8 = cpu_id
        // Compute stack_top = &KERNEL_STACK + STACK_SIZE * (cpu_id + 1)
        "adrp x9, {kernel_stack}",
        "add  x9, x9, #:lo12:{kernel_stack}",
        "mov  x10, {stack_size}",
        "add  x11, x8, #1",         // x11 = cpu_id + 1
        "mul  x11, x11, x10",       // x11 = STACK_SIZE * (cpu_id + 1)
        "add  x9, x9, x11",         // x9 = &KERNEL_STACK + offset = stack_top
        // Switch to SP_EL1 and set stack
        "msr spsel, #1",
        "mov sp, x9",
        "isb",
        // x0 = cpu_id, jump to ap_entry_wait
        "mov x0, x8",
        "b {ap_wait}",
        kernel_stack = sym KERNEL_STACK,
        stack_size = const STACK_SIZE,
        ap_wait = sym ap_entry_wait,
    );
}

fn ap_entry_wait(cpu_id: usize) -> ! {
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
            early_println!("[aarch64] No Limine MP response, single-CPU mode");
            return;
        }
    };

    let bsp_mpidr = mp_resp.bsp_mpidr;
    early_println!(
        "[aarch64] BSP mpidr={:#x}, {} CPU(s) detected by Limine",
        bsp_mpidr,
        mp_resp.cpus().len()
    );

    for cpu in mp_resp.cpus() {
        if cpu.mpidr == bsp_mpidr {
            continue;
        }
        early_println!("[aarch64] Bootstrapping CPU mpidr={:#x}...", cpu.mpidr);
        cpu.bootstrap(limine_ap_entry, cpu.mpidr);
    }
}

#[inline(always)]
fn current_el() -> u64 {
    let el: u64;
    unsafe {
        asm!("mrs {}, CurrentEL", out(reg) el, options(nostack));
    }
    (el >> 2) & 0x3
}

fn detect_el() -> (u64, bool) {
    let el = current_el();
    let vhe = if el == 2 {
        let hcr_el2: u64;
        unsafe {
            asm!("mrs {}, HCR_EL2", out(reg) hcr_el2, options(nostack));
        }
        (hcr_el2 & (1u64 << 34)) != 0
    } else {
        false
    };
    (el, vhe)
}

#[unsafe(link_section = ".init")]
#[unsafe(no_mangle)]
pub extern "C" fn limine_entry() -> ! {
    init_bss();
    mask_exceptions();

    let (el, vhe) = detect_el();
    unsafe {
        VHE_ENABLED = vhe;
        HV_AVAILABLE = vhe;
    }

    prepare_el1_runtime();

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
    crate::arch::aarch64::early_console_init();

    compiler_fence(core::sync::atomic::Ordering::SeqCst);

    if executable.virtual_base as usize != kernel_start {
        panic!(
            "kernel virtual base mismatch: limine={:#x} linker={:#x}",
            executable.virtual_base, kernel_start
        );
    }

    let dtb_ptr = dtb.dtb_ptr as usize;
    init_fdt(dtb_ptr);

    let usable_region = select_usable_region(memmap.entries());
    let hhdm_phys_span = hhdm_physical_span(memmap.entries());
    init_boot_direct_map_range(hhdm_phys_span.start, hhdm_phys_span.end);
    let hhdm_offset = hhdm.offset as usize;
    let relocated_fdt = relocate_fdt(phys_to_virt(usable_region.start) as *mut u8);
    let relocated_fdt_paddr = usable_region.start;
    let reserved_bytes = relocated_fdt.size();
    let usable_memory_paddr = reserve_front(usable_region, reserved_bytes);
    let direct_map_paddr = hhdm_phys_span;
    let initramfs_paddr = module_area(MODULE_REQUEST.response());
    let fdt_manager = FdtManager::get_manager();
    let cpu_count = fdt_manager.get_cpu_count().unwrap_or(1);
    let cmdline = fdt_manager
        .get_fdt()
        .and_then(|fdt| fdt.chosen().bootargs());
    let cpu_id = current_cpu_id();
    let framebuffer_paddr = framebuffer_area(FRAMEBUFFER_REQUEST.response());

    bootstrap_aps();

    let bootinfo = BootInfo::new(
        cpu_id,
        cpu_count,
        usable_memory_paddr,
        direct_map_paddr,
        initramfs_paddr,
        hhdm_offset,
        cmdline,
        DeviceSource::Fdt(relocated_fdt_paddr),
        framebuffer_paddr,
        Some(start_secondary_cpus),
    );

    crate::arch::init_user_context_from_fdt();
    crate::arch::aarch64::init_arch(cpu_id);

    let current_el = unsafe {
        let el: usize;
        asm!("mrs {0}, CurrentEL", out(reg) el, options(nostack));
        el >> 2
    };
    if vhe {
        early_println!("Current EL: EL{} (VHE enabled)", current_el);
    } else {
        early_println!("Current EL: EL{}", current_el);
    }

    unsafe {
        let stack_top = (&raw const KERNEL_STACK) as *const _ as usize + STACK_SIZE * (cpu_id + 1);
        (&raw mut EARLY_BOOTINFO).write(MaybeUninit::new(bootinfo));
        let bootinfo_ptr = (&raw const EARLY_BOOTINFO).cast::<BootInfo>();
        switch_stack_and_jump(
            start_kernel as *const () as usize,
            bootinfo_ptr as usize,
            stack_top,
        )
    }
}

#[inline(always)]
fn current_cpu_id() -> usize {
    let mpidr: usize;
    unsafe {
        asm!("mrs {0}, mpidr_el1", out(reg) mpidr, options(nostack));
    }
    mpidr & 0xff
}

#[inline(always)]
fn mask_exceptions() {
    unsafe {
        asm!("msr daifset, #0xf", options(nostack));
    }
}

#[inline(always)]
fn prepare_el1_runtime() {
    unsafe {
        asm!(
            "mov {tmp}, sp",
            "msr spsel, #1",
            "mov sp, {tmp}",
            "isb",
            tmp = lateout(reg) _,
            options(nostack)
        );
    }
}

#[unsafe(naked)]
pub unsafe extern "C" fn switch_stack_and_jump(
    _entry: usize,
    _arg0: usize,
    _stack_top: usize,
) -> ! {
    naked_asm!("mov x9, x0", "mov x0, x1", "mov sp, x2", "br x9",);
}
