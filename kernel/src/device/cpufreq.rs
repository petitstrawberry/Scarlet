//! CPU frequency diagnostics and provider registry.
//!
//! This module provides a small common layer for platform-specific CPU
//! frequency providers. It is intentionally read-only for now; policy,
//! governors, and frequency switching should build on top of this registry
//! instead of exposing platform-specific details to callers.

use core::sync::atomic::{AtomicU32, Ordering};

use spin::Mutex;

use crate::environment::MAX_NUM_CPUS;

const MAX_CPUFREQ_BACKENDS: usize = 8;
const INVALID_PERFORMANCE_DOMAIN: u32 = 0;

/// Architecture- or platform-provided CPU frequency diagnostic information.
///
/// This is a best-effort snapshot for debug/introspection surfaces. Not every
/// platform can report every field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuFrequencyInfo {
    /// Firmware phandle of the performance domain shared by this CPU.
    pub performance_domain: u32,
    /// Raw platform-specific frequency/status register value.
    pub raw_status: u32,
    /// Current performance state, if the platform layout is known.
    pub current_pstate: Option<u32>,
    /// Target performance state, if the platform layout is known.
    pub target_pstate: Option<u32>,
    /// Current frequency in kHz, if the current pstate is known in the OPP table.
    pub current_freq_khz: Option<u64>,
    /// Target frequency in kHz, if the target pstate is known in the OPP table.
    pub target_freq_khz: Option<u64>,
    /// Maximum frequency in kHz from the OPP table.
    pub max_freq_khz: Option<u64>,
}

/// CPU frequency information provider.
#[derive(Clone, Copy)]
pub struct CpuFrequencyBackend {
    /// Stable backend name for diagnostics.
    pub name: &'static str,
    /// Snapshot callback for a logical CPU ID.
    pub snapshot: fn(usize) -> Option<CpuFrequencyInfo>,
}

static CPU_PERF_DOMAINS: [AtomicU32; MAX_NUM_CPUS] =
    [const { AtomicU32::new(INVALID_PERFORMANCE_DOMAIN) }; MAX_NUM_CPUS];
static CPUFREQ_BACKENDS: Mutex<[Option<CpuFrequencyBackend>; MAX_CPUFREQ_BACKENDS]> =
    Mutex::new([None; MAX_CPUFREQ_BACKENDS]);

/// Register the firmware performance domain associated with a CPU.
///
/// # Arguments
///
/// * `cpu_id` - Logical CPU ID used by the scheduler.
/// * `phandle` - Firmware phandle from a CPU node's `performance-domains`.
pub fn register_cpu_performance_domain(cpu_id: usize, phandle: u32) {
    if cpu_id >= MAX_NUM_CPUS || phandle == INVALID_PERFORMANCE_DOMAIN {
        return;
    }

    CPU_PERF_DOMAINS[cpu_id].store(phandle, Ordering::SeqCst);
}

/// Return the firmware performance domain associated with a CPU.
///
/// # Arguments
///
/// * `cpu_id` - Logical CPU ID used by the scheduler.
///
/// # Returns
///
/// The registered performance-domain phandle, or `None` if unavailable.
pub fn cpu_performance_domain(cpu_id: usize) -> Option<u32> {
    if cpu_id >= MAX_NUM_CPUS {
        return None;
    }

    match CPU_PERF_DOMAINS[cpu_id].load(Ordering::SeqCst) {
        INVALID_PERFORMANCE_DOMAIN => None,
        phandle => Some(phandle),
    }
}

/// Register a CPU frequency backend.
///
/// # Arguments
///
/// * `backend` - Provider callbacks for a platform-specific implementation.
///
/// # Returns
///
/// `Ok(())` on success, or an error if the registry is full.
pub fn register_backend(backend: CpuFrequencyBackend) -> Result<(), &'static str> {
    let mut backends = CPUFREQ_BACKENDS.lock();

    if backends
        .iter()
        .flatten()
        .any(|registered| registered.name == backend.name)
    {
        return Ok(());
    }

    let Some(slot) = backends.iter_mut().find(|slot| slot.is_none()) else {
        return Err("cpufreq: backend registry full");
    };
    *slot = Some(backend);
    Ok(())
}

/// Return CPU frequency diagnostics for a CPU.
///
/// # Arguments
///
/// * `cpu_id` - Logical CPU ID used by the scheduler.
///
/// # Returns
///
/// A best-effort frequency snapshot, or `None` if no backend can report one.
pub fn cpu_frequency_info(cpu_id: usize) -> Option<CpuFrequencyInfo> {
    let backends = CPUFREQ_BACKENDS.lock();

    backends
        .iter()
        .flatten()
        .find_map(|backend| (backend.snapshot)(cpu_id))
}
