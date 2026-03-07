use core::arch::{asm, naked_asm};
use core::mem::MaybeUninit;

use crate::boot::limine::{
    DTB_REQUEST, EXECUTABLE_ADDRESS_REQUEST, HHDM_REQUEST, MEMMAP_REQUEST, MODULE_REQUEST,
    ensure_base_revision_supported, module_area, reserve_front, response, select_usable_region,
};
use crate::device::fdt::{FdtManager, init_fdt, relocate_fdt};
use crate::environment::STACK_SIZE;
use crate::mem::{KERNEL_STACK, init_bss};
use crate::vm::addr::{init_limine_addressing, phys_to_virt};
use crate::vm::vmem::MemoryArea;
use crate::{BootInfo, DeviceSource, start_ap, start_kernel};

static mut EARLY_BOOTINFO: MaybeUninit<BootInfo> = MaybeUninit::uninit();

#[unsafe(link_section = ".init")]
#[unsafe(no_mangle)]
pub extern "C" fn arch_start_kernel() -> ! {
    init_bss();
    mask_exceptions();
    prepare_el1_runtime();

    let hhdm = response(HHDM_REQUEST.get_response(), "hhdm");
    let executable = response(
        EXECUTABLE_ADDRESS_REQUEST.get_response(),
        "executable-address",
    );
    let memmap = response(MEMMAP_REQUEST.get_response(), "memmap");
    let dtb = response(DTB_REQUEST.get_response(), "dtb");

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

    let dtb_ptr = dtb.dtb_ptr() as usize;
    init_fdt(dtb_ptr);

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
    let cpu_id = current_cpu_id();

    let bootinfo = BootInfo::new(
        cpu_id,
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
    crate::arch::aarch64::init_arch(cpu_id);

    unsafe {
        let stack_top = (&raw const KERNEL_STACK) as *const _ as usize + STACK_SIZE * (cpu_id + 1);
        EARLY_BOOTINFO.write(bootinfo);
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
    let current_el = {
        let value: u64;
        unsafe {
            asm!("mrs {0}, CurrentEL", out(reg) value, options(nostack));
        }
        ((value >> 2) & 0x3) as usize
    };
    if current_el == 2 {
        unsafe {
            asm!(
                "mov x0, #(1 << 31)",
                "orr x0, x0, #(1 << 4)",
                "orr x0, x0, #(1 << 3)",
                "msr hcr_el2, x0",
                "isb",
                "mov x0, #(1 << 0)",
                "orr x0, x0, #(1 << 1)",
                "orr x0, x0, #(1 << 8)",
                "orr x0, x0, #(1 << 9)",
                "msr cnthctl_el2, x0",
                "msr cntvoff_el2, xzr",
                "mov x0, #0x3c5",
                "msr spsr_el2, x0",
                "adr x0, 2f",
                "msr elr_el2, x0",
                "eret",
                "2:",
                out("x0") _,
                options(nostack)
            );
        }
    } else if current_el != 1 {
        panic!(
            "Limine entered AArch64 kernel at unsupported EL{}",
            current_el
        );
    }

    unsafe {
        asm!(
            "mov x0, #(3 << 20)",
            "msr cpacr_el1, x0",
            "mov x0, #1",
            "msr spsel, x0",
            "isb",
            out("x0") _,
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

#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_entry_ap")]
#[unsafe(naked)]
pub extern "C" fn _entry_ap() {
    unsafe {
        naked_asm!(
            "mrs x4, MPIDR_EL1",
            "and x4, x4, #0xFF",
            "mov x2, {stack_size}",
            "adrp x3, KERNEL_STACK",
            "add x3, x3, :lo12:KERNEL_STACK",
            "add x5, x4, #1",
            "mul x5, x5, x2",
            "add x5, x3, x5",
            "and sp, x5, #~0xF",
            "mov x0, x4",
            "bl {start_ap}",
            "1:",
            "wfi",
            "b 1b",
            stack_size = const STACK_SIZE,
            start_ap = sym start_ap,
        );
    }
}
