#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use scarlet::sync::Mutex;

use scarlet::device::power::PowerManager;
use scarlet::device::reset::ResetController;
use scarlet::{
    arch::mmio,
    device::{
        DeviceInfo,
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    early_println,
};

// =============================================================================
// Register Bit Definitions
// =============================================================================

/// Power state target field (bits [3:0])
const PMGR_PS_TARGET: u32 = 0xf;
/// Current power state field (bits [7:4])
const PMGR_PS_ACTUAL: u32 = 0xf << 4;
/// Shift amount to extract PS_ACTUAL value
const PMGR_PS_ACTUAL_SHIFT: u32 = 4;
/// Reset assert bit
const PMGR_RESET: u32 = 1 << 31;
/// Auto power management enable
const PMGR_AUTO_ENABLE: u32 = 1 << 28;
/// Device was power-gated flag
const PMGR_WAS_PWRGATED: u32 = 1 << 8;
/// Device was clock-gated flag
const PMGR_WAS_CLKGATED: u32 = 1 << 9;
/// Flags to clear after power state transition
const PMGR_FLAGS: u32 = PMGR_WAS_CLKGATED | PMGR_WAS_PWRGATED;

/// Power state: fully active
const PMGR_PS_ACTIVE: u32 = 0xf;
/// Power state: clock gated
const PMGR_PS_CLKGATE: u32 = 0x4;
/// Power state: power gated (off)
const PMGR_PS_PWRGATE: u32 = 0x0;

// =============================================================================
// Power Domain Descriptor
// =============================================================================

/// Descriptor for a single power domain, parsed from device tree child node.
struct ApplePmDomain {
    offset: usize,
    pmgr_phandle: u32,
    label: alloc::string::String,
    always_on: bool,
    externally_clocked: bool,
    reset_cells: Option<usize>,
    index: u32,
    parent_phandles: Vec<u32>,
}

impl ApplePmDomain {
    /// Create a new power domain descriptor.
    fn new(
        offset: usize,
        pmgr_phandle: u32,
        index: u32,
        label: alloc::string::String,
        always_on: bool,
        externally_clocked: bool,
        reset_cells: Option<usize>,
    ) -> Self {
        Self {
            offset,
            pmgr_phandle,
            label,
            always_on,
            externally_clocked,
            reset_cells,
            index,
            parent_phandles: Vec::new(),
        }
    }
}

// =============================================================================
// PMGR Controller Instance
// =============================================================================

/// A single PMGR controller instance (there can be multiple PMGR blocks in SoC).
struct PmgrInstance {
    /// Physical base address of this PMGR MMIO region
    paddr: usize,
    /// Base virtual address of this PMGR MMIO region
    base_addr: usize,
    /// Size of the MMIO region
    size: usize,
    /// Power domains managed by this PMGR block
    domains: BTreeMap<u32, ApplePmDomain>,
}

impl PmgrInstance {
    /// Create a new PMGR instance.
    fn new(paddr: usize, base_addr: usize, size: usize) -> Self {
        Self {
            paddr,
            base_addr,
            size,
            domains: BTreeMap::new(),
        }
    }

    /// Read the power domain register.
    #[inline]
    fn read_reg(&self, domain: &ApplePmDomain) -> u32 {
        // SAFETY: domain.offset is within the MMIO-mapped PMGR region
        unsafe { mmio::read32(self.base_addr + domain.offset) }
    }

    fn write_reg(&self, domain: &ApplePmDomain, val: u32) {
        // SAFETY: domain.offset is within the MMIO-mapped PMGR region
        unsafe { mmio::write32(self.base_addr + domain.offset, val) }
    }

    fn enable_domain_local(&self, domain: &ApplePmDomain) -> Result<(), &'static str> {
        if domain.always_on {
            return Ok(());
        }

        let reg = self.read_reg(domain);
        let actual = (reg & PMGR_PS_ACTUAL) >> PMGR_PS_ACTUAL_SHIFT;
        if actual == PMGR_PS_ACTIVE {
            return Ok(());
        }

        let val = (reg & !PMGR_PS_TARGET) | PMGR_PS_ACTIVE;
        let val = val & !PMGR_FLAGS;
        self.write_reg(domain, val);

        let mut timeout = 1000;
        loop {
            let val = self.read_reg(domain);
            let actual = (val & PMGR_PS_ACTUAL) >> PMGR_PS_ACTUAL_SHIFT;
            if actual == PMGR_PS_ACTIVE {
                return Ok(());
            }
            timeout -= 1;
            if timeout == 0 {
                early_println!(
                    "[pmgr] timeout enabling power domain '{}' (reg={:#x})",
                    domain.label,
                    reg
                );
                return Err("pmgr: power domain enable timeout");
            }
            core::hint::spin_loop();
        }
    }

    /// Disable (power off) a power domain.
    ///
    /// Sets PS_TARGET to PWRGATE (0x0), then polls PS_ACTUAL until it reaches PWRGATE.
    fn disable_domain_local(&self, domain: &ApplePmDomain) -> Result<(), &'static str> {
        if domain.always_on {
            return Ok(());
        }

        let reg = self.read_reg(domain);
        let actual = (reg & PMGR_PS_ACTUAL) >> PMGR_PS_ACTUAL_SHIFT;
        if actual == PMGR_PS_PWRGATE {
            return Ok(());
        }

        let val = reg & !PMGR_PS_TARGET;
        self.write_reg(domain, val);

        let mut timeout = 1000;
        loop {
            let val = self.read_reg(domain);
            let actual = (val & PMGR_PS_ACTUAL) >> PMGR_PS_ACTUAL_SHIFT;
            if actual == PMGR_PS_PWRGATE {
                return Ok(());
            }
            timeout -= 1;
            if timeout == 0 {
                early_println!(
                    "[pmgr] timeout disabling power domain '{}' (reg={:#x})",
                    domain.label,
                    reg
                );
                return Err("pmgr: power domain disable timeout");
            }
            core::hint::spin_loop();
        }
    }

    /// Assert reset on a power domain.
    fn reset_assert_domain_local(&self, domain: &ApplePmDomain) {
        let reg = self.read_reg(domain);
        self.write_reg(domain, reg | PMGR_RESET);
    }

    /// Deassert reset on a power domain.
    fn reset_deassert_domain_local(&self, domain: &ApplePmDomain) {
        let reg = self.read_reg(domain);
        self.write_reg(domain, reg & !PMGR_RESET);
    }

    /// Check if a power domain is currently powered on.
    fn is_domain_on_local(&self, domain: &ApplePmDomain) -> bool {
        let reg = self.read_reg(domain);
        let actual = (reg & PMGR_PS_ACTUAL) >> PMGR_PS_ACTUAL_SHIFT;
        actual == PMGR_PS_ACTIVE
    }

    fn with_instance<R>(domain: &ApplePmDomain, f: impl FnOnce(&PmgrInstance) -> R) -> Option<R> {
        let guard = get_registry()?;
        let registry = guard.as_ref()?;
        let instance = registry.instances.get(&domain.pmgr_phandle)?;
        Some(f(instance))
    }

    fn enable_domain(domain: &ApplePmDomain) -> Result<(), &'static str> {
        for &parent_ph in &domain.parent_phandles {
            if let Ok(parent) = pmgr_get_domain_by_phandle(parent_ph) {
                parent.enable()?;
            }
        }

        Self::with_instance(domain, |instance| instance.enable_domain_local(domain))
            .unwrap_or(Err("pmgr: instance not found"))
    }

    fn disable_domain(domain: &ApplePmDomain) -> Result<(), &'static str> {
        Self::with_instance(domain, |instance| instance.disable_domain_local(domain))
            .unwrap_or(Err("pmgr: instance not found"))
    }

    fn reset_assert_domain(domain: &ApplePmDomain) {
        let _ = Self::with_instance(domain, |instance| {
            instance.reset_assert_domain_local(domain)
        });
    }

    fn reset_deassert_domain(domain: &ApplePmDomain) {
        let _ = Self::with_instance(domain, |instance| {
            instance.reset_deassert_domain_local(domain)
        });
    }

    fn is_domain_on(domain: &ApplePmDomain) -> bool {
        Self::with_instance(domain, |instance| instance.is_domain_on_local(domain)).unwrap_or(false)
    }
}

impl scarlet::device::power::PowerDomain for ApplePmDomain {
    fn enable(&self) -> Result<(), &'static str> {
        PmgrInstance::enable_domain(self)
    }

    fn disable(&self) -> Result<(), &'static str> {
        PmgrInstance::disable_domain(self)
    }

    fn is_enabled(&self) -> bool {
        PmgrInstance::is_domain_on(self)
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn requires_external_clock(&self) -> bool {
        self.externally_clocked
    }
}

impl ResetController for ApplePmDomain {
    fn name(&self) -> &'static str {
        "apple-pmgr-pwrstate-reset"
    }

    fn reset_cells(&self) -> usize {
        self.reset_cells.unwrap_or(0)
    }

    fn assert_reset(&self, spec: &[u32]) -> Result<(), &'static str> {
        if spec.len() != self.reset_cells() {
            return Err("apple-pmgr: invalid reset specifier");
        }
        PmgrInstance::reset_assert_domain(self);
        Ok(())
    }

    fn deassert_reset(&self, spec: &[u32]) -> Result<(), &'static str> {
        if spec.len() != self.reset_cells() {
            return Err("apple-pmgr: invalid reset specifier");
        }
        PmgrInstance::reset_deassert_domain(self);
        Ok(())
    }
}

// =============================================================================
// Global PMGR Manager
// =============================================================================

/// Global PMGR registry that holds all PMGR instances.
///
/// Power domains are looked up by a composite key: (instance_index, domain_index).
/// The `power-domains = <&pmgr N>` DT property provides N as the domain index
/// within the referenced PMGR instance.
static PMGR_REGISTRY: Mutex<Option<PmgrRegistry>> = Mutex::new(None);

/// Holds all registered PMGR instances, keyed by phandle.
struct PmgrRegistry {
    instances: BTreeMap<u32, Arc<PmgrInstance>>,
    domain_map: BTreeMap<(u32, u32), Arc<ApplePmDomain>>,
    pwrstate_phandles: BTreeMap<u32, (u32, u32)>,
}

impl PmgrRegistry {
    fn new() -> Self {
        Self {
            instances: BTreeMap::new(),
            domain_map: BTreeMap::new(),
            pwrstate_phandles: BTreeMap::new(),
        }
    }
}

/// Get a reference to the global PMGR registry.
fn get_registry() -> Option<scarlet::sync::MutexGuard<'static, Option<PmgrRegistry>>> {
    let guard = PMGR_REGISTRY.lock();
    if guard.is_some() { Some(guard) } else { None }
}

// =============================================================================
// Public API
// =============================================================================

/// Result of a PMGR domain lookup.
pub struct PmgrDomain {
    inner: Arc<PmgrInstance>,
    domain: Arc<ApplePmDomain>,
}

impl PmgrDomain {
    /// Enable (power on) this domain.
    pub fn enable(&self) -> Result<(), &'static str> {
        PmgrInstance::enable_domain(&self.domain)
    }

    /// Disable (power off) this domain.
    pub fn disable(&self) -> Result<(), &'static str> {
        self.inner.disable_domain_local(&self.domain)
    }

    /// Assert reset.
    pub fn reset_assert(&self) {
        self.inner.reset_assert_domain_local(&self.domain)
    }

    /// Deassert reset.
    pub fn reset_deassert(&self) {
        self.inner.reset_deassert_domain_local(&self.domain)
    }

    /// Check if powered on.
    pub fn is_on(&self) -> bool {
        self.inner.is_domain_on_local(&self.domain)
    }

    /// Get the label of this domain.
    pub fn label(&self) -> &str {
        &self.domain.label
    }
}

/// Look up a power domain by (PMGR phandle, domain index).
///
/// This is the primary API for other drivers to acquire power domain control.
/// Drivers use this after parsing their `power-domains` DT property.
///
/// # Arguments
///
/// * `pmgr_phandle` - The phandle of the PMGR controller node
/// * `domain_index` - The index of the power domain within that PMGR
///
/// # Returns
///
/// A `PmgrDomain` handle, or an error if the domain is not found.
pub fn pmgr_get_domain(pmgr_phandle: u32, domain_index: u32) -> Result<PmgrDomain, &'static str> {
    let guard = get_registry().ok_or("pmgr: registry not initialized")?;
    let registry = guard.as_ref().unwrap();

    let domain = registry
        .domain_map
        .get(&(pmgr_phandle, domain_index))
        .ok_or("pmgr: domain not found")?;

    // Find the instance for this domain
    let instance = registry
        .instances
        .get(&pmgr_phandle)
        .ok_or("pmgr: instance not found")?;

    Ok(PmgrDomain {
        inner: Arc::clone(instance),
        domain: Arc::clone(domain),
    })
}

/// Look up a power domain by its firmware label.
///
/// This is useful for Apple devices whose pwrstate node exists in the PMGR
/// tree but is not referenced directly by the consumer node's `power-domains`
/// property.
///
/// # Arguments
///
/// * `label` - Firmware `label` property from a PMGR pwrstate node.
///
/// # Returns
///
/// A `PmgrDomain` handle, or an error if no matching domain is registered.
pub fn pmgr_get_domain_by_label(label: &str) -> Result<PmgrDomain, &'static str> {
    let guard = get_registry().ok_or("pmgr: registry not initialized")?;
    let registry = guard.as_ref().unwrap();

    let domain = registry
        .domain_map
        .values()
        .find(|domain| domain.label.as_str() == label)
        .ok_or("pmgr: domain label not found")?;

    let instance = registry
        .instances
        .get(&domain.pmgr_phandle)
        .ok_or("pmgr: instance not found")?;

    Ok(PmgrDomain {
        inner: Arc::clone(instance),
        domain: Arc::clone(domain),
    })
}

/// Check if the PMGR registry has been initialized.
pub fn pmgr_is_initialized() -> bool {
    let guard = PMGR_REGISTRY.lock();
    guard.is_some()
}

/// Look up a power domain by the power-controller node's own phandle.
///
/// This is the convenience API for device drivers that read `power-domains = <&phandle>`
/// from their device tree node and want to enable that domain.
pub fn pmgr_get_domain_by_phandle(pwrstate_phandle: u32) -> Result<PmgrDomain, &'static str> {
    let (pmgr_phandle, domain_index) = {
        let guard = get_registry().ok_or("pmgr: registry not initialized")?;
        let registry = guard.as_ref().unwrap();
        let result = registry
            .pwrstate_phandles
            .get(&pwrstate_phandle)
            .ok_or("pmgr: pwrstate phandle not found")?;
        (result.0, result.1)
    };

    pmgr_get_domain(pmgr_phandle, domain_index)
}

/// Look up a power domain by its PMGR register physical address.
///
/// m1n1's ADT `clock-gates` entries resolve to concrete PMGR power-state
/// registers through `/arm-io/pmgr/devices` and `/arm-io/pmgr/ps-regs`. Guest
/// DT overlays can carry those physical register addresses even when the
/// consumer node has no direct FDT `power-domains` reference. This helper maps
/// that address back to a registered FDT pwrstate domain.
///
/// # Arguments
///
/// * `register_paddr` - Physical address of the PMGR power-state register.
///
/// # Returns
///
/// A `PmgrDomain` handle, or an error if no registered domain owns that
/// register.
pub fn pmgr_get_domain_by_register_paddr(
    register_paddr: usize,
) -> Result<PmgrDomain, &'static str> {
    let guard = get_registry().ok_or("pmgr: registry not initialized")?;
    let registry = guard.as_ref().unwrap();

    for domain in registry.domain_map.values() {
        let Some(instance) = registry.instances.get(&domain.pmgr_phandle) else {
            continue;
        };
        if instance.paddr + domain.offset == register_paddr {
            return Ok(PmgrDomain {
                inner: Arc::clone(instance),
                domain: Arc::clone(domain),
            });
        }
    }

    Err("pmgr: register paddr not found")
}

// =============================================================================
// Platform Driver Implementation
// =============================================================================

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    PowerManager::init();

    let mem_resource = device
        .get_resources()
        .iter()
        .find(|r| matches!(r.res_type, PlatformDeviceResourceType::MEM))
        .ok_or("apple-pmgr: no memory resource found")?;

    let paddr = mem_resource.start;
    let size = mem_resource.end - mem_resource.start + 1;

    early_println!(
        "[apple-pmgr] probing {} at paddr={:#x}, size={:#x}",
        device.name(),
        paddr,
        size
    );

    let base_addr = scarlet::vm::ioremap(paddr, size).map_err(|_| "apple-pmgr: ioremap failed")?;

    let mut registry_guard = PMGR_REGISTRY.lock();

    if registry_guard.is_none() {
        *registry_guard = Some(PmgrRegistry::new());
    }

    let registry = registry_guard.as_mut().unwrap();

    let instance = Arc::new(PmgrInstance::new(paddr, base_addr, size));

    let phandle = device
        .property("phandle")
        .and_then(|p| p.as_usize())
        .map(|v| v as u32)
        .or_else(|| {
            device
                .property("linux,phandle")
                .and_then(|p| p.as_usize())
                .map(|v| v as u32)
        })
        .unwrap_or(device.id() as u32);

    registry.instances.insert(phandle, instance);

    early_println!(
        "[apple-pmgr] registered PMGR instance at {:#x} (phandle={})",
        base_addr,
        phandle
    );

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_pmgr_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-pmgr",
        probe_fn,
        remove_fn,
        alloc::vec![
            "apple,t8103-pmgr",
            "apple,pmgr",
            "apple,t6000-pmgr",
            "apple,t6020-pmgr",
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Critical);
    register_pwrstate_driver();
}

scarlet::driver_initcall!(register_pmgr_driver);

// =============================================================================
// Power Domain (pwrstate) Driver
// =============================================================================

fn pwrstate_probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let mem_resource = device
        .get_resources()
        .iter()
        .find(|r| matches!(r.res_type, PlatformDeviceResourceType::MEM))
        .ok_or("apple-pmgr-pwrstate: no memory resource found")?;

    let offset = mem_resource.start;
    let index = device.id() as u32;

    let label = device
        .property("label")
        .and_then(|p| p.as_str())
        .unwrap_or("unknown");
    let label = String::from(label);

    let always_on = device.property("apple,always-on").is_some();
    let externally_clocked = device.property("apple,externally-clocked").is_some();
    let reset_cells = device
        .property("#reset-cells")
        .and_then(|p| p.as_usize())
        .map(|v| v as usize);

    let pwrstate_phandle = device
        .property("phandle")
        .and_then(|p| p.as_usize())
        .map(|v| v as u32)
        .or_else(|| {
            device
                .property("linux,phandle")
                .and_then(|p| p.as_usize())
                .map(|v| v as u32)
        });

    let parent_phandles = if let Some(pd_prop) = device.property("power-domains") {
        let bytes = pd_prop.value();
        let mut parents = Vec::new();
        let mut offset = 0usize;
        while offset + 4 <= bytes.len() {
            let ph = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap_or([0; 4]));
            if ph != 0 {
                parents.push(ph);
            }
            offset += 4;
        }
        parents
    } else {
        Vec::new()
    };

    early_println!(
        "[apple-pmgr] registering domain '{}' at offset={:#x}, index={}, always_on={}, externally_clocked={}",
        label,
        offset,
        index,
        always_on,
        externally_clocked
    );

    let mut registry_guard = PMGR_REGISTRY.lock();
    let registry = registry_guard
        .as_mut()
        .ok_or("apple-pmgr-pwrstate: PMGR registry not initialized")?;

    let parent_phandle = device
        .parent_phandle()
        .ok_or("apple-pmgr-pwrstate: no parent phandle")?;

    let mut domain = ApplePmDomain::new(
        offset,
        parent_phandle,
        index,
        label,
        always_on,
        externally_clocked,
        reset_cells,
    );
    domain.parent_phandles = parent_phandles;
    let domain = Arc::new(domain);
    registry
        .domain_map
        .insert((parent_phandle, index), Arc::clone(&domain));

    if let Some(ph) = pwrstate_phandle {
        registry
            .pwrstate_phandles
            .insert(ph, (parent_phandle, index));
        PowerManager::register_domain(
            ph,
            Arc::clone(&domain) as Arc<dyn scarlet::device::power::PowerDomain>,
        );
        if reset_cells.is_some() {
            DeviceManager::get_manager()
                .register_reset_controller(ph, Arc::clone(&domain) as Arc<dyn ResetController>);
        }
    }

    Ok(())
}

fn pwrstate_remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_pwrstate_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-pmgr-pwrstate",
        pwrstate_probe_fn,
        pwrstate_remove_fn,
        alloc::vec!["apple,t8103-pmgr-pwrstate", "apple,pmgr-pwrstate"],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Critical);
}

#[used]
static SCARLET_DRIVER_APPLE_PMGR_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
