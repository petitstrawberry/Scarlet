use core::arch::{asm, naked_asm};
use core::mem::MaybeUninit;
use core::sync::atomic::compiler_fence;

use limine::mp::MpInfo;

use crate::boot::limine::{
    DTB_REQUEST, EXECUTABLE_ADDRESS_REQUEST, FRAMEBUFFER_REQUEST, HHDM_REQUEST, MEMMAP_REQUEST,
    MODULE_REQUEST, MP_REQUEST, boot_cmdline, bootloader_hhdm_physical_bound,
    ensure_base_revision_supported, framebuffer_area, module_area, reserve_front, response,
    runtime_direct_map_regions, select_usable_region,
};
use crate::device::fdt::{FdtManager, init_fdt, relocate_fdt};
use crate::environment::STACK_SIZE;
use crate::mem::{KERNEL_STACK, init_bss};
use crate::vm::addr::{init_bootloader_direct_map_bound, init_limine_addressing, phys_to_virt};
use crate::{BootInfo, DeviceSource, early_println, start_ap, start_kernel, wait_for_ap_release};

static mut EARLY_BOOTINFO: MaybeUninit<BootInfo> = MaybeUninit::uninit();

static mut VHE_ENABLED: bool = false;
static mut HV_AVAILABLE: bool = false;

const HCR_EL2_FMO: u64 = 1 << 3;
const HCR_EL2_IMO: u64 = 1 << 4;
const HCR_EL2_AMO: u64 = 1 << 5;
const HCR_EL2_TGE: u64 = 1 << 27;
const HCR_EL2_RW: u64 = 1 << 31;
const HCR_EL2_E2H: u64 = 1 << 34;
const HCR_EL2_HOST_INTERRUPT_ROUTING: u64 = HCR_EL2_FMO | HCR_EL2_IMO | HCR_EL2_AMO;
const HCR_EL2_VHE_HOST_REQUIRED: u64 =
    HCR_EL2_RW | HCR_EL2_TGE | HCR_EL2_E2H | HCR_EL2_HOST_INTERRUPT_ROUTING;
const CPTR_EL2_VHE_FPEN_FULL: u64 = 0b11 << 20;
#[allow(dead_code)]
const SCTLR_M: u64 = 1;
#[allow(dead_code)]
const SCTLR_C: u64 = 1 << 2;
#[allow(dead_code)]
const SCTLR_I: u64 = 1 << 12;
#[allow(dead_code)]
const LIMINE_MAIR_ATTR_INDEX_NORMAL_WRITE_BACK: u64 = 0xff;

pub fn is_vhe_enabled() -> bool {
    unsafe { VHE_ENABLED }
}

pub fn is_hv_available() -> bool {
    unsafe { HV_AVAILABLE }
}

const fn should_drop_el2_to_el1(el: u64, hcr_el2: u64) -> bool {
    el == 2 && hcr_el2 & HCR_EL2_E2H == 0
}

const fn vhe_host_hcr(hcr_el2: u64) -> Option<u64> {
    if hcr_el2 & HCR_EL2_E2H == 0 {
        None
    } else {
        Some(hcr_el2 | HCR_EL2_VHE_HOST_REQUIRED)
    }
}

fn maybe_drop_el2_to_el1(continuation: usize, arg0: usize) {
    if current_el() != 2 {
        return;
    }

    let hcr_el2: u64;
    unsafe {
        asm!("mrs {}, hcr_el2", out(reg) hcr_el2, options(nostack));
    }
    if !should_drop_el2_to_el1(2, hcr_el2) {
        return;
    }

    // SAFETY: The EL2 and non-VHE checks above establish that this one-way
    // transition is required before continuing in the real EL1 register bank.
    unsafe {
        drop_el2_to_el1(continuation, arg0);
    }
}

#[inline(always)]
fn read_active_sctlr() -> u64 {
    let sctlr: u64;
    unsafe {
        asm!("mrs {sctlr}, sctlr_el1", sctlr = out(reg) sctlr, options(nostack));
    }
    sctlr
}

#[unsafe(naked)]
unsafe extern "C" fn drop_el2_to_el1(_continuation: usize, _arg0: usize) -> ! {
    naked_asm!(
        ".arch armv8.1-a",
        // Preserve the continuation, its argument, and the active EL2 stack.
        "mov x16, x0",
        "mov x17, x1",
        "mov x18, sp",
        "mrs x15, hcr_el2",
        "tbz x15, #34, 1f",
        // In VHE host mode these EL1 names access the active EL2 host bank.
        "mrs x3, ttbr0_el1",
        "mrs x4, ttbr1_el1",
        "mrs x5, tcr_el1",
        "mrs x6, mair_el1",
        "mrs x7, vbar_el1",
        "mrs x8, sctlr_el1",
        "b 2f",
        // Without VHE, capture the active EL2 translation regime. EL2 has no
        // TTBR1 regime, and TCR_EL2.PS must be moved to TCR_EL1.IPS.
        "1:",
        "mrs x3, ttbr0_el2",
        "mov x4, xzr",
        "mrs x5, tcr_el2",
        "and x9, x5, #0xffff",
        "ubfx x10, x5, #16, #3",
        "lsl x10, x10, #32",
        "orr x9, x9, x10",
        // Disable the unused TTBR1 walk regime.
        "orr x9, x9, #0x800000",
        // TCR_EL2.TBI becomes TCR_EL1.TBI0.
        "tbz x5, #20, 7f",
        "mov x10, #1",
        "lsl x10, x10, #37",
        "orr x9, x9, x10",
        "7:",
        "mov x5, x9",
        "mrs x6, mair_el2",
        "mrs x7, vbar_el2",
        "mrs x8, sctlr_el2",
        "2:",
        "mov x14, x8",
        // Keep Limine's live translation regime, but do not inherit an
        // uncached execution state. APs spin on the release barrier before
        // Scarlet installs its own page tables, so leaving C clear here would
        // turn every waiter into uncached memory traffic.
        "bic x8, x8, #2",
        "orr x8, x8, #1",
        "orr x8, x8, #4",
        "orr x8, x8, #0x1000",
        "msr sp_el1, x18",
        "tbz x15, #34, 3f",
        // Program the real EL1 bank through the EL12 aliases while E2H is
        // still active. EL1 starts with its MMU disabled until all state is set.
        "bic x9, x8, #1",
        "msr sctlr_el12, x9",
        "isb",
        "msr ttbr0_el12, x3",
        "msr ttbr1_el12, x4",
        "msr tcr_el12, x5",
        "msr mair_el12, x6",
        "msr vbar_el12, x7",
        // Keep FP/SIMD available while Rust continues at EL1. Normal per-CPU
        // initialization applies the final user-access policy later.
        "movz x9, #0x30, lsl #16",
        "msr cpacr_el12, x9",
        "msr cntkctl_el12, xzr",
        "b 4f",
        // In conventional EL2 mode, the ordinary EL1 names address the real
        // EL1 bank directly.
        "3:",
        "bic x9, x8, #1",
        "msr sctlr_el1, x9",
        "isb",
        "msr ttbr0_el1, x3",
        "msr ttbr1_el1, x4",
        "msr tcr_el1, x5",
        "msr mair_el1, x6",
        "msr vbar_el1, x7",
        "movz x9, #0x30, lsl #16",
        "msr cpacr_el1, x9",
        "msr cntkctl_el1, xzr",
        "4:",
        // CPTR_EL2 has different layouts under VHE and non-VHE. Zero would
        // select FPEN=0b00 while E2H is set and leave FP/SIMD trapped at EL2.
        // This bridge value enables VHE FPEN while keeping non-VHE TFP clear.
        "tbz x15, #34, 8f",
        "mov x9, {cptr_vhe_fpen_full}",
        "msr cptr_el2, x9",
        "b 9f",
        "8:",
        "msr cptr_el2, xzr",
        "9:",
        "isb",
        "msr hstr_el2, xzr",
        "msr mdcr_el2, xzr",
        // Quarantine both timer banks visible through the current EL2 regime.
        // With VHE active these are the EL2 host banks; without VHE they are
        // the real EL1 banks.
        "mrs x9, cntp_ctl_el0",
        "orr x9, x9, #2",
        "msr cntp_ctl_el0, x9",
        "mrs x9, cntv_ctl_el0",
        "orr x9, x9, #2",
        "msr cntv_ctl_el0, x9",
        // In VHE host mode the real EL1 timer banks are exposed through the
        // EL02 aliases. Mask them before clearing E2H, otherwise an inherited
        // EL1 physical timer can become an unhandled FIQ immediately after
        // ERET. Scarlet initializes and unmasks CNTV later at EL1.
        "tbz x15, #34, 10f",
        "mrs x9, cntp_ctl_el02",
        "orr x9, x9, #2",
        "msr cntp_ctl_el02, x9",
        "mrs x9, cntv_ctl_el02",
        "orr x9, x9, #2",
        "msr cntv_ctl_el02, x9",
        "10:",
        "dsb sy",
        "isb",
        // Install a known non-trapping EL1 timer policy instead of preserving
        // firmware trap controls. Scarlet uses CNTV after detecting EL1.
        "mov x9, #3",
        "msr cnthctl_el2, x9",
        "msr cntvoff_el2, xzr",
        // Invalidate stale entries from any earlier use of the real EL1&0
        // translation regime before enabling the copied tables.
        "dsb ishst",
        "tlbi alle1is",
        "dsb ish",
        "ic iallu",
        "dsb sy",
        "isb",
        "tbz x15, #34, 5f",
        "msr sctlr_el12, x8",
        "b 6f",
        "5:",
        "msr sctlr_el1, x8",
        "6:",
        "isb",
        // Return to EL1h with every exception class masked. HCR_EL2.RW is the
        // only required bit; clearing E2H/TGE/FMO/IMO/AMO routes exceptions to
        // EL1. Do not execute anything between the HCR write and ERET.
        "msr elr_el2, x16",
        "mov x9, #0x3c5",
        "msr spsr_el2, x9",
        "mov x0, x17",
        "mov x1, x14",
        "mov x9, {hcr_rw}",
        "msr hcr_el2, x9",
        "eret",
        hcr_rw = const HCR_EL2_RW,
        cptr_vhe_fpen_full = const CPTR_EL2_VHE_FPEN_FULL,
    );
}

#[unsafe(naked)]
unsafe extern "C" fn limine_ap_entry(_info: &MpInfo) -> ! {
    naked_asm!(
        // Clear TPIDR_EL1 so try_get_cpuid() treats this CPU as uninitialized
        // until trap_init publishes the per-CPU pointer.
        "msr tpidr_el1, xzr",
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
        // x0 = cpu_id, jump to secondary_cpu_entry
        "mov x0, x8",
        "b {ap_wait}",
        kernel_stack = sym KERNEL_STACK,
        stack_size = const STACK_SIZE,
        ap_wait = sym secondary_cpu_entry,
    );
}

/// Prepare a secondary CPU's exception level and enter the common AP startup.
///
/// # Arguments
///
/// * `cpu_id` - Logical CPU ID assigned by the platform boot code.
///
/// # Returns
///
/// This function does not return.
pub extern "C" fn secondary_cpu_entry(cpu_id: usize) -> ! {
    mask_exceptions();
    maybe_drop_el2_to_el1(
        secondary_cpu_entry_after_el_drop as *const () as usize,
        cpu_id,
    );
    secondary_cpu_entry_after_el_drop(cpu_id, read_active_sctlr())
}

extern "C" fn secondary_cpu_entry_after_el_drop(cpu_id: usize, inherited_sctlr: u64) -> ! {
    prepare_el1_runtime();
    let _ = configure_vhe_host_control();
    // log_el1_memory_state("handoff", cpu_id, inherited_sctlr);
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
        if mpidr_affinity(cpu.mpidr) == mpidr_affinity(bsp_mpidr) {
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
            if let Err(err) = crate::sched::scheduler::register_cpu_topology_domain(cpu_id, phandle)
            {
                early_println!(
                    "[aarch64] Failed to register CPU topology domain for cpu={}: {}",
                    cpu_id,
                    err
                );
            }
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

fn cpu_logical_id_from_fdt(cpu: &fdt::node::FdtNode, fallback_cpu_id: usize) -> usize {
    let Some(mpidr) = cpu_reg(cpu) else {
        return fallback_cpu_id;
    };

    if let Some(mp_resp) = MP_REQUEST.response() {
        for (cpu_id, cpu) in mp_resp.cpus().iter().copied().enumerate() {
            if mpidr_affinity(cpu.mpidr) == mpidr_affinity(mpidr) {
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

#[inline(always)]
fn configure_vhe_host_control() -> Option<(u64, u64)> {
    if current_el() != 2 {
        return None;
    }

    let old_hcr: u64;
    // SAFETY: CurrentEL is EL2, so HCR_EL2 is accessible.
    unsafe {
        asm!("mrs {0}, hcr_el2", out(reg) old_hcr, options(nostack));
    }
    // A VHE kernel and its EL0 processes are one host. TGE selects the EL2&0
    // translation regime for EL0 and routes its exceptions to EL2; RW keeps
    // that lower level in AArch64 state. E2H alone is not a complete host
    // configuration.
    let new_hcr = vhe_host_hcr(old_hcr)?;
    // SAFETY: The CPU is still at EL2 and exceptions are masked during boot.
    // The barriers make the translation/exception-routing change complete
    // before any lower-EL execution can begin.
    unsafe {
        asm!(
            "dsb sy",
            "msr hcr_el2, {0}",
            "isb",
            in(reg) new_hcr,
            options(nostack)
        );
    }

    Some((old_hcr, new_hcr))
}

#[unsafe(link_section = ".init")]
#[unsafe(no_mangle)]
pub extern "C" fn limine_entry() -> ! {
    // SAFETY: TPIDR_EL1 holds whatever firmware left; explicitly clear it
    // so try_get_cpuid() can deterministically treat 0 as "uninitialized"
    // until trap_init publishes the per-CPU pointer.
    unsafe {
        core::arch::asm!("msr tpidr_el1, xzr");
    }
    init_bss();
    mask_exceptions();

    maybe_drop_el2_to_el1(limine_entry_after_el_drop as *const () as usize, 0);
    limine_entry_after_el_drop(0, read_active_sctlr())
}

extern "C" fn limine_entry_after_el_drop(_arg0: usize, inherited_sctlr: u64) -> ! {
    let (_el, vhe) = detect_el();
    unsafe {
        VHE_ENABLED = vhe;
        HV_AVAILABLE = vhe;
    }

    let hcr_transition = configure_vhe_host_control();

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
    let cpu_id = bsp_logical_cpu_id();
    crate::arch::aarch64::init_arch(cpu_id);
    crate::arch::aarch64::early_console_init();

    if let Some((old_hcr, new_hcr)) = hcr_transition {
        early_println!(
            "[aarch64] BSP: HCR_EL2 VHE host control {:#x} -> {:#x}",
            old_hcr,
            new_hcr
        );
    }
    // log_el1_memory_state("handoff", _arg0, inherited_sctlr);
    // diagnose_limine_mair_compatibility();

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
    let bootloader_hhdm_bound = bootloader_hhdm_physical_bound(memmap.entries());
    init_bootloader_direct_map_bound(bootloader_hhdm_bound.start, bootloader_hhdm_bound.end);
    let hhdm_offset = hhdm.offset as usize;
    let framebuffer_paddr = framebuffer_area(FRAMEBUFFER_REQUEST.response());
    let direct_map_regions = runtime_direct_map_regions(memmap.entries(), framebuffer_paddr)
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

    bootstrap_aps();
    register_cpu_topology_from_fdt();

    let bootinfo = BootInfo::new(
        cpu_id,
        cpu_count,
        usable_memory_paddr,
        direct_map_regions,
        initramfs_paddr,
        hhdm_offset,
        cmdline,
        DeviceSource::Fdt(relocated_fdt_paddr),
        framebuffer_paddr,
        Some(start_secondary_cpus),
    );

    crate::arch::init_user_context_from_fdt();

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

#[cfg(test)]
mod el_drop_tests {
    use super::*;

    #[test_case]
    fn el2_vhe_stays_at_el2_and_non_vhe_drops() {
        assert!(!should_drop_el2_to_el1(2, HCR_EL2_E2H));
        assert!(should_drop_el2_to_el1(2, 0));
        assert!(!should_drop_el2_to_el1(1, HCR_EL2_E2H));
    }

    #[test_case]
    fn vhe_host_control_includes_el0_translation_and_exception_routing() {
        assert_eq!(vhe_host_hcr(HCR_EL2_RW), None);
        let hcr = vhe_host_hcr(HCR_EL2_E2H).expect("E2H must select VHE host mode");
        assert_eq!(hcr & HCR_EL2_TGE, HCR_EL2_TGE);
        assert_eq!(hcr & HCR_EL2_RW, HCR_EL2_RW);
        assert_eq!(
            hcr & HCR_EL2_HOST_INTERRUPT_ROUTING,
            HCR_EL2_HOST_INTERRUPT_ROUTING
        );
    }

    #[test_case]
    fn el2_drop_quarantines_el1_timer_banks_before_eret() {
        let source = include_str!("limine.rs");
        let physical_timer_mask = source
            .find("msr cntp_ctl_el02, x9")
            .expect("EL2 drop must mask the real EL1 physical timer bank");
        let virtual_timer_mask = source
            .find("msr cntv_ctl_el02, x9")
            .expect("EL2 drop must mask the real EL1 virtual timer bank");
        let hcr_drop = source
            .find("msr hcr_el2, x9")
            .expect("EL2 drop must install the EL1 routing policy");

        assert!(physical_timer_mask < hcr_drop);
        assert!(virtual_timer_mask < hcr_drop);
    }
    #[test_case]
    fn mpidr_matching_ignores_non_affinity_bits() {
        let affinity = (0x12_u64 << 32) | (0x34 << 16) | (0x56 << 8) | 0x78;
        let register_value = affinity | (1 << 31) | (1 << 30) | (1 << 24) | (0xab << 40);

        assert_eq!(mpidr_affinity(register_value), affinity);
        assert_eq!(mpidr_affinity(register_value), mpidr_affinity(affinity));
    }

    #[test_case]
    fn bsp_uses_the_limine_cpu_array_index() {
        let mpidrs = [0x000, 0x100, 0x200, 0x300, 0x400, 0x500, 0x600, 0x700];

        assert_eq!(logical_cpu_id_in_mpidrs(0x700, mpidrs), Some(7));
        assert_eq!(logical_cpu_id_in_mpidrs(0x8000_0700, mpidrs), Some(7));
    }
}

fn bsp_logical_cpu_id() -> usize {
    if let Some(mp_resp) = MP_REQUEST.response() {
        return logical_cpu_id_in_mpidrs(
            mp_resp.bsp_mpidr,
            mp_resp.cpus().iter().copied().map(|cpu| cpu.mpidr),
        )
        .unwrap_or_else(|| {
            panic!(
                "Limine MP response omitted BSP mpidr={:#x}",
                mp_resp.bsp_mpidr
            )
        });
    }

    // Keep a register-based fallback for bootloaders that provide no MP data.
    logical_cpu_id_from_mpidr(current_mpidr())
}

fn logical_cpu_id_from_mpidr(mpidr: u64) -> usize {
    (mpidr_affinity(mpidr) & 0xff) as usize
}

fn logical_cpu_id_in_mpidrs(mpidr: u64, mpidrs: impl IntoIterator<Item = u64>) -> Option<usize> {
    let affinity = mpidr_affinity(mpidr);
    mpidrs
        .into_iter()
        .position(|candidate| mpidr_affinity(candidate) == affinity)
}

/// Return only the affinity fields that identify a processing element.
///
/// `MPIDR_EL1` also exposes non-affinity state such as the architecturally
/// fixed RES1 bit and the U/MT bits. Compare only Aff3:Aff0 when matching the
/// live register, Limine response, and firmware topology descriptions.
const fn mpidr_affinity(mpidr: u64) -> u64 {
    // Aff3 is [39:32], while Aff2:Aff0 occupy [23:0].
    mpidr & 0x0000_00ff_00ff_ffff
}

#[inline(always)]
fn current_mpidr() -> u64 {
    let mpidr: u64;
    unsafe {
        asm!("mrs {0}, mpidr_el1", out(reg) mpidr, options(nostack));
    }
    mpidr
}

#[allow(dead_code)]
fn log_el1_memory_state(stage: &str, cpu_id: usize, inherited_sctlr: u64) {
    let sctlr: u64;
    let tcr: u64;
    let mair: u64;
    unsafe {
        asm!(
            "mrs {sctlr}, sctlr_el1",
            "mrs {tcr}, tcr_el1",
            "mrs {mair}, mair_el1",
            sctlr = out(reg) sctlr,
            tcr = out(reg) tcr,
            mair = out(reg) mair,
            options(nomem, nostack, preserves_flags),
        );
    }
    early_println!(
        "[aarch64] CPU {} {} memory state: EL={} inherited_SCTLR={:#x} SCTLR={:#x} M={} C={} I={} TCR={:#x} MAIR={:#x}",
        cpu_id,
        stage,
        current_el(),
        inherited_sctlr,
        sctlr,
        usize::from(sctlr & SCTLR_M != 0),
        usize::from(sctlr & SCTLR_C != 0),
        usize::from(sctlr & SCTLR_I != 0),
        tcr,
        mair,
    );
}

/// Diagnoses whether Limine's live AttrIndx 0 preserves the Normal WB contract.
///
/// The diagnostic only reads MAIR_EL1. Scarlet later installs the same Normal
/// WB encoding at AttrIndx 0 before replacing Limine's page tables, preventing
/// an attribute reinterpretation while those old TTBRs remain live.
#[allow(dead_code)]
fn diagnose_limine_mair_compatibility() {
    let mair: u64;
    unsafe {
        asm!("mrs {mair}, mair_el1", mair = out(reg) mair, options(nostack));
    }

    let attr_index_zero = mair & 0xff;
    if attr_index_zero == LIMINE_MAIR_ATTR_INDEX_NORMAL_WRITE_BACK {
        early_println!(
            "[aarch64] Limine MAIR AttrIndx0={:#x} Normal WB; Scarlet preserves it during TTBR handoff",
            attr_index_zero,
        );
    } else {
        early_println!(
            "[aarch64] WARNING: Limine MAIR AttrIndx0={:#x}, expected Normal WB {:#x}; Scarlet will preserve AttrIndx0 as Normal WB before replacing TTBRs",
            attr_index_zero,
            LIMINE_MAIR_ATTR_INDEX_NORMAL_WRITE_BACK,
        );
    }
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
