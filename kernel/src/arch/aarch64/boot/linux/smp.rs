//! Firmware-assisted SMP for the Linux arm64 Image boot contract.
//!
//! CPU nodes provide MPIDR affinities; PSCI CPU_ON receives a physical entry
//! address and our logical CPU ID as its context. No Limine data is involved.

use core::arch::{asm, naked_asm};

use fdt::node::FdtNode;

use crate::environment::{MAX_NUM_CPUS, STACK_SIZE};
use crate::mem::KERNEL_STACK;
use crate::sched::scheduler::{online_cpu_mask, scheduler_ready};
use crate::sync::Once;
use crate::vm::addr::kernel_virt_to_phys;

const MPIDR_AFFINITY_MASK: u64 = 0x0000_00ff_00ff_ffff;
const PSCI_VERSION: u64 = 0x8400_0000;
const PSCI_CPU_ON_64: u64 = 0xc400_0003;

#[derive(Clone, Copy, Debug)]
enum Conduit {
    Smc,
    Hvc,
}

impl Conduit {
    fn from_fdt(fdt: &fdt::Fdt<'_>) -> Option<Self> {
        let node = fdt.all_nodes().find(|node| {
            enabled(node)
                && node.compatible().is_some_and(|compatible| {
                    compatible
                        .all()
                        .any(|value| matches!(value, "arm,psci-0.2" | "arm,psci-1.0"))
                })
        })?;
        match node.property("method")?.as_str()? {
            "smc" => Some(Self::Smc),
            "hvc" => Some(Self::Hvc),
            _ => None,
        }
    }

    fn invoke(self, function: u64, arg0: u64, arg1: u64, arg2: u64) -> i64 {
        let mut result = function;
        // SAFETY: The enabled PSCI firmware node selected the conduit. Use
        // the SMCCC register ABI, including caller-clobbered registers and
        // a memory barrier in the compiler model (firmware starts a CPU).
        unsafe {
            match self {
                Self::Smc => asm!(
                    "smc #0",
                    inout("x0") result,
                    inout("x1") arg0 => _,
                    inout("x2") arg1 => _,
                    inout("x3") arg2 => _,
                    in("x4") 0u64, in("x5") 0u64,
                    in("x6") 0u64, in("x7") 0u64,
                    clobber_abi("C"),
                    options(nostack),
                ),
                Self::Hvc => asm!(
                    "hvc #0",
                    inout("x0") result,
                    inout("x1") arg0 => _,
                    inout("x2") arg1 => _,
                    inout("x3") arg2 => _,
                    in("x4") 0u64, in("x5") 0u64,
                    in("x6") 0u64, in("x7") 0u64,
                    clobber_abi("C"),
                    options(nostack),
                ),
            }
        }
        // Error codes are signed 32-bit values, including for SMC64 calls.
        result as i32 as i64
    }
}

struct Configuration {
    conduit: Option<Conduit>,
    mpidrs: [u64; MAX_NUM_CPUS],
    cpu_count: usize,
}

static CONFIGURATION: Once<Configuration> = Once::new();

fn enabled(node: &FdtNode<'_, '_>) -> bool {
    node.property("status")
        .and_then(|property| property.as_str())
        .is_none_or(|status| matches!(status, "okay" | "ok"))
}

fn cpu_mpidr(node: &FdtNode<'_, '_>) -> Option<u64> {
    if !enabled(node) || node.property("device_type")?.as_str()? != "cpu" {
        return None;
    }
    let reg = node.property("reg")?.value;
    let mpidr = match reg.len() {
        4 => u32::from_be_bytes(reg.try_into().ok()?) as u64,
        8 => u64::from_be_bytes(reg.try_into().ok()?),
        _ => return None,
    };
    (mpidr & !MPIDR_AFFINITY_MASK == 0).then_some(mpidr)
}

/// Enumerate enabled PSCI CPUs, keeping the running CPU at logical ID zero.
pub(super) fn initialize(fdt: &fdt::Fdt<'_>, cmdline: &str) -> usize {
    let boot_mpidr: u64;
    // SAFETY: The Image bootstrap is executing at EL1 on the boot CPU.
    unsafe { asm!("mrs {}, mpidr_el1", out(reg) boot_mpidr, options(nomem, nostack)) };
    let mut configuration = Configuration {
        conduit: Conduit::from_fdt(fdt),
        mpidrs: [0; MAX_NUM_CPUS],
        cpu_count: 1,
    };
    configuration.mpidrs[0] = boot_mpidr & MPIDR_AFFINITY_MASK;
    let maximum = cmdline
        .split_whitespace()
        .filter_map(|word| word.strip_prefix("maxcpus="))
        .filter_map(|value| value.parse::<usize>().ok())
        .last()
        .unwrap_or(MAX_NUM_CPUS)
        .clamp(1, MAX_NUM_CPUS);

    if configuration.conduit.is_some() {
        if let Some(cpus) = fdt.find_node("/cpus") {
            for node in cpus.children() {
                let Some(mpidr) = cpu_mpidr(&node) else {
                    continue;
                };
                if configuration.mpidrs[..configuration.cpu_count].contains(&mpidr) {
                    continue;
                }
                if node.property("enable-method").and_then(|p| p.as_str()) != Some("psci") {
                    crate::println!(
                        "[linux-boot] CPU mpidr={:#x}: unsupported enable-method",
                        mpidr
                    );
                    continue;
                }
                if configuration.cpu_count == maximum {
                    break;
                }
                configuration.mpidrs[configuration.cpu_count] = mpidr;
                configuration.cpu_count += 1;
            }
        }
    }
    crate::println!(
        "[linux-boot] PSCI {:?}; {} CPU(s) selected; BSP mpidr={:#x}",
        configuration.conduit,
        configuration.cpu_count,
        configuration.mpidrs[0],
    );
    // Use the same firmware binding as other boot protocols, but map it to
    // the logical IDs actually selected by PSCI enumeration above.
    if let Some(cpus) = fdt.find_node("/cpus") {
        for node in cpus.children() {
            let Some(mpidr) = cpu_mpidr(&node) else {
                continue;
            };
            let Some(cpu_id) = configuration.mpidrs[..configuration.cpu_count]
                .iter()
                .position(|affinity| *affinity == mpidr)
            else {
                continue;
            };
            if let Some(domain) = crate::device::cpufreq::performance_domain_from_fdt(&node) {
                crate::device::cpufreq::register_cpu_performance_domain(cpu_id, domain);
            }
        }
    }
    let cpu_count = configuration.cpu_count;
    assert!(
        CONFIGURATION.set(configuration).is_ok(),
        "Linux CPU topology initialized twice"
    );
    cpu_count
}

/// Raw physical CPU_ON entry; x0 is the logical CPU ID supplied by the BSP.
///
/// Each CPU uses its own existing kernel boot-stack slot. BSS clearing and
/// all global initialization are exclusively owned by the Image boot CPU.
///
/// # Safety
///
/// Enter only through CPU_ON at EL1/EL2 with the MMU off and x0 holding an
/// exclusively assigned secondary logical CPU ID. The BSP must have completed
/// kernel initialization and published the immutable early translation table.
#[unsafe(link_section = ".head.text.entry.ap")]
#[unsafe(export_name = "_linux_image_entry_ap")]
#[unsafe(naked)]
pub unsafe extern "C" fn secondary_image_entry() -> ! {
    naked_asm!(
        "msr daifset, #0xf",
        "cbz x0, 2f",
        "cmp x0, {max_cpus}",
        "b.hs 2f",
        "adrp x1, {stack}",
        "add x1, x1, :lo12:{stack}",
        "mov x3, {stack_size}",
        "add x4, x0, #1",
        "madd x1, x4, x3, x1",
        "adrp x2, {rust_entry}",
        "add x2, x2, :lo12:{rust_entry}",
        "b {prepare}",
        "2:", "wfe", "b 2b",
        max_cpus = const MAX_NUM_CPUS,
        stack = sym KERNEL_STACK,
        stack_size = const STACK_SIZE,
        rust_entry = sym secondary_cpu_entry,
        prepare = sym super::prepare_el1_entry,
    );
}

extern "C" fn secondary_cpu_entry(cpu_id: usize) -> ! {
    super::page_table::install_secondary();
    crate::wait_for_ap_release();
    crate::start_ap(cpu_id)
}

fn counter() -> u64 {
    let value: u64;
    // SAFETY: The bootstrap established a consistent virtual counter at EL1.
    unsafe { asm!("mrs {}, cntvct_el0", out(reg) value, options(nomem, nostack)) };
    value
}

/// Start APs after global kernel initialization and the BSP's first task claim.
pub(super) fn start_secondary_cpus() {
    let configuration = CONFIGURATION.get().expect("Linux CPU topology unavailable");
    let Some(conduit) = configuration.conduit else {
        return;
    };
    // Keep the claimed BSP task in its bootstrap context until the hook returns.
    let saved_daif = crate::arch::interrupt::save_and_disable_interrupts();
    let version = conduit.invoke(PSCI_VERSION, 0, 0, 0);
    crate::println!(
        "[linux-boot] PSCI version={:#x}; starting secondary CPUs",
        version
    );
    if version < 2 {
        crate::println!(
            "[linux-boot] PSCI 0.2 or newer is required; secondary CPUs remain offline"
        );
        crate::arch::interrupt::restore_interrupts(saved_daif);
        return;
    }
    let entry = kernel_virt_to_phys(secondary_image_entry as *const () as usize);
    let frequency: u64;
    // SAFETY: CNTFRQ is programmed by firmware on the Image boot CPU.
    unsafe { asm!("mrs {}, cntfrq_el0", out(reg) frequency, options(nomem, nostack)) };
    assert_ne!(
        frequency, 0,
        "PSCI CPU startup requires the architected counter"
    );
    crate::release_aps();

    for cpu_id in 1..configuration.cpu_count {
        let result = conduit.invoke(
            PSCI_CPU_ON_64,
            configuration.mpidrs[cpu_id],
            entry,
            cpu_id as u64,
        );
        crate::println!(
            "[linux-boot] CPU_ON cpu={} mpidr={:#x} entry={:#x}: {}",
            cpu_id,
            configuration.mpidrs[cpu_id],
            entry,
            result,
        );
        if result != 0 {
            continue;
        }
        // Firmware success only means the request was accepted. Wait at most
        // one second for actual per-CPU initialization and scheduler publication.
        let start = counter();
        while !scheduler_ready(cpu_id) || online_cpu_mask() & (1 << cpu_id) == 0 {
            if counter().wrapping_sub(start) >= frequency {
                crate::println!("[linux-boot] CPU {}: scheduler-online timeout", cpu_id);
                break;
            }
            core::hint::spin_loop();
        }
    }
    let mask = online_cpu_mask();
    crate::println!(
        "[linux-boot] SMP scheduler online: {}/{} CPU(s), mask={:#x}",
        mask.count_ones(),
        configuration.cpu_count,
        mask,
    );
    crate::arch::interrupt::restore_interrupts(saved_daif);
}
