use core::arch::{asm, naked_asm};
use core::mem::MaybeUninit;

use limine::mp::MpInfo;

use crate::boot::limine::{
    DTB_REQUEST, EXECUTABLE_ADDRESS_REQUEST, FRAMEBUFFER_REQUEST, HHDM_REQUEST, MEMMAP_REQUEST,
    MODULE_REQUEST, MP_REQUEST, boot_cmdline, ensure_base_revision_supported, framebuffer_area,
    hhdm_physical_span, module_area, reserve_front, response, select_usable_region,
};
use crate::device::fdt::{FdtManager, init_fdt, relocate_fdt};
use crate::environment::STACK_SIZE;
use crate::mem::{KERNEL_STACK, init_bss};
use crate::vm::addr::{init_boot_direct_map_range, init_limine_addressing, phys_to_virt};
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
        "ldr x8, [x0, #32]",        // x8 = mp_info.extra_argument = logical cpu_id
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

    for (cpu_id, cpu) in mp_resp.cpus().iter().copied().enumerate() {
        if cpu.mpidr == bsp_mpidr {
            continue;
        }
        early_println!(
            "[aarch64] Bootstrapping CPU {} mpidr={:#x}...",
            cpu_id,
            cpu.mpidr
        );
        cpu.bootstrap(limine_ap_entry, cpu_id as u64);
    }
}

fn register_cpu_topology_from_fdt() {
    let Some(fdt) = FdtManager::get_manager().get_fdt() else {
        return;
    };
    let Some(cpus) = fdt.find_node("/cpus") else {
        return;
    };

    let mut min_capacity = u32::MAX;
    let mut max_capacity = 0u32;
    for cpu in cpus.children() {
        if !is_enabled_cpu_node(&cpu) {
            continue;
        }
        if let Some(capacity) = cpu_capacity_dmips_mhz(&cpu) {
            min_capacity = min_capacity.min(capacity);
            max_capacity = max_capacity.max(capacity);
        }
    }

    let has_capacity_range = min_capacity != u32::MAX && max_capacity > min_capacity;
    let mut fallback_cpu_id = 0usize;
    for cpu in cpus.children() {
        if !is_enabled_cpu_node(&cpu) {
            continue;
        }

        let cpu_id = cpu_logical_id_from_fdt(&cpu, fallback_cpu_id);
        fallback_cpu_id += 1;

        let raw_capacity = cpu_capacity_dmips_mhz(&cpu);
        let core_class = classify_cpu_node(&cpu, raw_capacity, min_capacity, max_capacity);
        let scheduler_capacity = if has_capacity_range {
            raw_capacity
                .map(|capacity| {
                    capacity.saturating_mul(
                        crate::sched::scheduler::CpuCoreClass::Balanced.default_capacity(),
                    ) / max_capacity
                })
                .unwrap_or(0)
        } else {
            0
        };

        match crate::sched::scheduler::register_cpu_topology(cpu_id, core_class, scheduler_capacity)
        {
            Ok(()) => early_println!(
                "[aarch64] CPU topology: cpu={} class={:?} capacity={}",
                cpu_id,
                core_class,
                crate::sched::scheduler::cpu_topology(cpu_id)
                    .map(|topology| topology.capacity)
                    .unwrap_or(0)
            ),
            Err(err) => early_println!(
                "[aarch64] Failed to register CPU topology for cpu={}: {}",
                cpu_id,
                err
            ),
        }

        if let Some(phandle) = cpu_performance_domain_phandle(&cpu) {
            crate::device::cpufreq::register_cpu_performance_domain(cpu_id, phandle);
        }
    }
}

fn is_enabled_cpu_node(cpu: &fdt::node::FdtNode) -> bool {
    if let Some(dev_type) = cpu.property("device_type") {
        if bytes_to_cstr(dev_type.value)
            .map(|value| value != "cpu")
            .unwrap_or(false)
        {
            return false;
        }
    } else if cpu.name != "cpu" && !cpu.name.starts_with("cpu@") {
        return false;
    }

    if let Some(status) = cpu.property("status") {
        if bytes_to_cstr(status.value)
            .map(|value| value == "disabled")
            .unwrap_or(false)
        {
            return false;
        }
    }

    true
}

fn cpu_capacity_dmips_mhz(cpu: &fdt::node::FdtNode) -> Option<u32> {
    let prop = cpu.property("capacity-dmips-mhz")?;
    read_be_u32(prop.value)
}

fn cpu_performance_domain_phandle(cpu: &fdt::node::FdtNode) -> Option<u32> {
    let prop = cpu.property("performance-domains")?;
    read_be_u32(prop.value)
}

fn classify_cpu_node(
    cpu: &fdt::node::FdtNode,
    raw_capacity: Option<u32>,
    min_capacity: u32,
    max_capacity: u32,
) -> crate::sched::scheduler::CpuCoreClass {
    use crate::sched::scheduler::CpuCoreClass;

    if compatible_contains_any(cpu, &[b"icestorm", b"blizzard", b"efficiency", b"e-core"]) {
        return CpuCoreClass::Efficiency;
    }
    if compatible_contains_any(
        cpu,
        &[
            b"firestorm",
            b"avalanche",
            b"everest",
            b"performance",
            b"p-core",
        ],
    ) {
        return CpuCoreClass::Performance;
    }

    if min_capacity != u32::MAX
        && max_capacity > min_capacity
        && let Some(capacity) = raw_capacity
    {
        if capacity == min_capacity {
            return CpuCoreClass::Efficiency;
        }
        if capacity == max_capacity {
            return CpuCoreClass::Performance;
        }
    }

    CpuCoreClass::Balanced
}

fn compatible_contains_any(cpu: &fdt::node::FdtNode, needles: &[&[u8]]) -> bool {
    let Some(prop) = cpu.property("compatible") else {
        return false;
    };

    needles
        .iter()
        .any(|needle| bytes_contains_ascii_case_insensitive(prop.value, needle))
}

fn bytes_contains_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }

    haystack.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle.iter())
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
    })
}

fn cpu_logical_id_from_fdt(cpu: &fdt::node::FdtNode, fallback_cpu_id: usize) -> usize {
    let Some(mpidr) = cpu_reg(cpu) else {
        return fallback_cpu_id;
    };

    if let Some(mp_resp) = MP_REQUEST.response() {
        for (cpu_id, cpu) in mp_resp.cpus().iter().copied().enumerate() {
            if cpu.mpidr == mpidr {
                return cpu_id;
            }
        }
    }

    fallback_cpu_id
}

fn cpu_reg(cpu: &fdt::node::FdtNode) -> Option<u64> {
    let prop = cpu.property("reg")?;
    match prop.value.len() {
        0..=3 => None,
        4..=7 => read_be_u32(prop.value).map(|value| value as u64),
        _ => Some(u64::from_be_bytes(prop.value[0..8].try_into().ok()?)),
    }
}

fn read_be_u32(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < 4 {
        return None;
    }
    Some(u32::from_be_bytes(bytes[0..4].try_into().ok()?))
}

fn bytes_to_cstr(bytes: &[u8]) -> Option<&str> {
    let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..len]).ok()
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

    let (_el, vhe) = detect_el();
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
    let fdt_cmdline = fdt_manager
        .get_fdt()
        .and_then(|fdt| fdt.chosen().bootargs());
    let cmdline = boot_cmdline(fdt_cmdline);
    let cpu_id = logical_cpu_id_from_mpidr(current_mpidr());
    let framebuffer_paddr = framebuffer_area(FRAMEBUFFER_REQUEST.response());

    // Cache the wall-clock epoch now; the Limine response pointer is invalid
    // after the page-table switch in start_kernel.
    crate::boot::limine::capture_date_at_boot();

    bootstrap_aps();
    register_cpu_topology_from_fdt();

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

fn logical_cpu_id_from_mpidr(mpidr: u64) -> usize {
    if let Some(mp_resp) = MP_REQUEST.response() {
        for (cpu_id, cpu) in mp_resp.cpus().iter().copied().enumerate() {
            if cpu.mpidr == mpidr {
                return cpu_id;
            }
        }
    }

    (mpidr & 0xff) as usize
}

#[inline(always)]
fn current_mpidr() -> u64 {
    let mpidr: u64;
    unsafe {
        asm!("mrs {0}, mpidr_el1", out(reg) mpidr, options(nostack));
    }
    mpidr
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
