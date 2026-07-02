//! Apple SoC CPU frequency diagnostics.
//!
//! This driver does not implement cpufreq policy or frequency switching. It
//! exposes Apple cluster DVFS status registers through the common cpufreq
//! provider registry.

use core::sync::atomic::{AtomicBool, Ordering};

use spin::Mutex;

use crate::{
    arch::mmio,
    device::{
        cpufreq::{
            CpuFrequencyBackend, CpuFrequencyInfo, cpu_performance_domain, register_backend,
        },
        fdt::FdtManager,
    },
    driver_initcall, vm,
};

const APPLE_DVFS_STATUS: usize = 0x50;
const MIN_MMIO_SIZE: usize = 0x1000;
const MAX_CPUFREQ_DOMAINS: usize = 8;
const MAX_OPPS_PER_DOMAIN: usize = 32;
const INVALID_PHANDLE: u32 = 0;

static SCANNED_FDT: AtomicBool = AtomicBool::new(false);
static CPUFREQ_DOMAINS: Mutex<[AppleCpuFreqDomain; MAX_CPUFREQ_DOMAINS]> =
    Mutex::new([AppleCpuFreqDomain::empty(); MAX_CPUFREQ_DOMAINS]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PstateEncoding {
    S5l8960x,
    T8103,
    T8112,
    Unknown,
}

impl PstateEncoding {
    fn from_node(node: &fdt::node::FdtNode<'_, '_>) -> Self {
        if compatible_contains(node, b"apple,s5l8960x-cluster-cpufreq") {
            Self::S5l8960x
        } else if compatible_contains(node, b"apple,t8103-cluster-cpufreq") {
            Self::T8103
        } else if compatible_contains(node, b"apple,t8112-cluster-cpufreq") {
            Self::T8112
        } else {
            Self::Unknown
        }
    }

    fn current_pstate(self, status: u32) -> Option<u32> {
        match self {
            Self::S5l8960x => Some((status >> 3) & 0x7),
            Self::T8103 => Some((status >> 4) & 0xf),
            Self::T8112 => Some((status >> 5) & 0x1f),
            Self::Unknown => None,
        }
    }

    fn target_pstate(self, status: u32) -> Option<u32> {
        match self {
            Self::S5l8960x => Some(status & 0x7),
            Self::T8103 => Some(status & 0xf),
            Self::T8112 => Some(status & 0x1f),
            Self::Unknown => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct OppEntry {
    pstate: u32,
    freq_khz: u64,
}

impl OppEntry {
    const fn empty() -> Self {
        Self {
            pstate: 0,
            freq_khz: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct AppleCpuFreqDomain {
    valid: bool,
    phandle: u32,
    paddr: usize,
    size: usize,
    vaddr: usize,
    map_failed: bool,
    encoding: PstateEncoding,
    opp_count: usize,
    opps: [OppEntry; MAX_OPPS_PER_DOMAIN],
}

impl AppleCpuFreqDomain {
    const fn empty() -> Self {
        Self {
            valid: false,
            phandle: INVALID_PHANDLE,
            paddr: 0,
            size: 0,
            vaddr: 0,
            map_failed: false,
            encoding: PstateEncoding::Unknown,
            opp_count: 0,
            opps: [OppEntry::empty(); MAX_OPPS_PER_DOMAIN],
        }
    }

    fn freq_for_pstate(&self, pstate: Option<u32>) -> Option<u64> {
        let pstate = pstate?;
        self.opps[..self.opp_count]
            .iter()
            .find(|opp| opp.pstate == pstate)
            .map(|opp| opp.freq_khz)
    }

    fn max_freq_khz(&self) -> Option<u64> {
        self.opps[..self.opp_count]
            .iter()
            .map(|opp| opp.freq_khz)
            .max()
    }
}

fn cpu_frequency_info(cpu_id: usize) -> Option<CpuFrequencyInfo> {
    ensure_fdt_scanned();

    let phandle = cpu_performance_domain(cpu_id)?;
    let mut domains = CPUFREQ_DOMAINS.lock();
    let domain = domains
        .iter_mut()
        .find(|domain| domain.valid && domain.phandle == phandle)?;
    let vaddr = ensure_mapped(domain)?;

    // SAFETY: `vaddr` is an ioremap'd Apple cluster-cpufreq MMIO base.
    let raw_status = unsafe { mmio::read32(vaddr + APPLE_DVFS_STATUS) };
    let current_pstate = domain.encoding.current_pstate(raw_status);
    let target_pstate = domain.encoding.target_pstate(raw_status);

    Some(CpuFrequencyInfo {
        performance_domain: phandle,
        raw_status,
        current_pstate,
        target_pstate,
        current_freq_khz: domain.freq_for_pstate(current_pstate),
        target_freq_khz: domain.freq_for_pstate(target_pstate),
        max_freq_khz: domain.max_freq_khz(),
    })
}

fn ensure_mapped(domain: &mut AppleCpuFreqDomain) -> Option<usize> {
    if domain.vaddr != 0 {
        return Some(domain.vaddr);
    }
    if domain.map_failed {
        return None;
    }

    let size = domain.size.max(MIN_MMIO_SIZE);
    match vm::ioremap(domain.paddr, size) {
        Ok(vaddr) => {
            domain.vaddr = vaddr;
            Some(vaddr)
        }
        Err(err) => {
            domain.map_failed = true;
            crate::early_println!(
                "[apple-cpufreq] failed to map domain phandle={:#x} paddr={:#x}: {}",
                domain.phandle,
                domain.paddr,
                err
            );
            None
        }
    }
}

fn ensure_fdt_scanned() {
    if SCANNED_FDT.load(Ordering::Acquire) {
        return;
    }

    let mut domains = CPUFREQ_DOMAINS.lock();
    if SCANNED_FDT.load(Ordering::Relaxed) {
        return;
    }

    scan_fdt(&mut domains);
    SCANNED_FDT.store(true, Ordering::Release);
}

fn scan_fdt(domains: &mut [AppleCpuFreqDomain; MAX_CPUFREQ_DOMAINS]) {
    let Some(fdt) = FdtManager::get_manager().get_fdt() else {
        return;
    };

    let mut next = 0usize;
    for node in fdt.all_nodes() {
        if !compatible_contains(&node, b"apple,cluster-cpufreq")
            && !compatible_contains(&node, b"apple,s5l8960x-cluster-cpufreq")
            && !compatible_contains(&node, b"apple,t8103-cluster-cpufreq")
            && !compatible_contains(&node, b"apple,t8112-cluster-cpufreq")
        {
            continue;
        }

        let Some(phandle) =
            read_u32_prop(&node, "phandle").or_else(|| read_u32_prop(&node, "linux,phandle"))
        else {
            continue;
        };
        let Some((paddr, size)) = first_reg_region(&node) else {
            continue;
        };

        if next >= domains.len() {
            crate::early_println!("[apple-cpufreq] too many cluster-cpufreq domains");
            break;
        }

        let mut domain = AppleCpuFreqDomain {
            valid: true,
            phandle,
            paddr,
            size,
            vaddr: 0,
            map_failed: false,
            encoding: PstateEncoding::from_node(&node),
            opp_count: 0,
            opps: [OppEntry::empty(); MAX_OPPS_PER_DOMAIN],
        };
        load_domain_opps(fdt, phandle, &mut domain);

        crate::early_println!(
            "[apple-cpufreq] domain phandle={:#x} paddr={:#x} size={:#x} opps={}",
            domain.phandle,
            domain.paddr,
            domain.size,
            domain.opp_count
        );

        domains[next] = domain;
        next += 1;
    }
}

fn load_domain_opps(fdt: &fdt::Fdt<'_>, domain_phandle: u32, domain: &mut AppleCpuFreqDomain) {
    let Some(opp_table_phandle) = opp_table_phandle_for_domain(fdt, domain_phandle) else {
        return;
    };
    let Some(opp_table) = find_node_by_phandle(fdt, opp_table_phandle) else {
        return;
    };

    for opp_node in opp_table.children() {
        if domain.opp_count >= MAX_OPPS_PER_DOMAIN {
            break;
        }

        let Some(pstate) = read_u32_prop(&opp_node, "opp-level") else {
            continue;
        };
        let Some(freq_hz) = read_u64_prop(&opp_node, "opp-hz") else {
            continue;
        };

        domain.opps[domain.opp_count] = OppEntry {
            pstate,
            freq_khz: freq_hz.div_ceil(1000),
        };
        domain.opp_count += 1;
    }
}

fn opp_table_phandle_for_domain(fdt: &fdt::Fdt<'_>, domain_phandle: u32) -> Option<u32> {
    let cpus = fdt.find_node("/cpus")?;
    for cpu in cpus.children() {
        if read_u32_prop(&cpu, "performance-domains") != Some(domain_phandle) {
            continue;
        }
        if let Some(phandle) = read_u32_prop(&cpu, "operating-points-v2") {
            return Some(phandle);
        }
    }

    None
}

fn first_reg_region(node: &fdt::node::FdtNode<'_, '_>) -> Option<(usize, usize)> {
    let region = (*node).reg()?.next()?;
    Some((
        region.starting_address as usize,
        region.size.unwrap_or(MIN_MMIO_SIZE),
    ))
}

fn compatible_contains(node: &fdt::node::FdtNode<'_, '_>, needle: &[u8]) -> bool {
    let Some(prop) = node.property("compatible") else {
        return false;
    };

    prop.value
        .split(|byte| *byte == 0)
        .any(|entry| entry == needle)
}

fn find_node_by_phandle<'a>(
    fdt: &'a fdt::Fdt<'a>,
    phandle: u32,
) -> Option<fdt::node::FdtNode<'a, 'a>> {
    fdt.find_phandle(phandle).or_else(|| {
        fdt.all_nodes().find(|node| {
            read_u32_prop(node, "linux,phandle")
                .map(|node_phandle| node_phandle == phandle)
                .unwrap_or(false)
        })
    })
}

fn read_u32_prop(node: &fdt::node::FdtNode<'_, '_>, name: &str) -> Option<u32> {
    let prop = node.property(name)?;
    read_be_u32(prop.value)
}

fn read_u64_prop(node: &fdt::node::FdtNode<'_, '_>, name: &str) -> Option<u64> {
    let prop = node.property(name)?;
    match prop.value.len() {
        0..=3 => None,
        4..=7 => read_be_u32(prop.value).map(u64::from),
        _ => Some(u64::from_be_bytes(prop.value[0..8].try_into().ok()?)),
    }
}

fn read_be_u32(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < 4 {
        return None;
    }
    Some(u32::from_be_bytes(bytes[0..4].try_into().ok()?))
}

fn register_apple_soc_cpufreq_backend() {
    if let Err(err) = register_backend(CpuFrequencyBackend {
        name: "apple-soc-cpufreq",
        snapshot: cpu_frequency_info,
    }) {
        crate::early_println!("[apple-cpufreq] failed to register backend: {}", err);
    }
}

driver_initcall!(register_apple_soc_cpufreq_backend);
