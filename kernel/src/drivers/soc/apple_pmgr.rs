//! Apple Power Management (PMGR) driver
//!
//! Controls power domains and resets for Apple Silicon peripherals.
//! Reference: asahi-linux `drivers/soc/apple/pmgr.c`
//!
//! # Device Tree Binding
//!
//! ```text
//! power-management@23b700000 {
//!     compatible = "apple,t8103-pmgr", "apple,pmgr", "syscon", "simple-mfd";
//!     reg = <0x02 0x3b700000 0x00 0x14000>;
//!     #address-cells = <0x01>;
//!     #size-cells = <0x01>;
//!
//!     power-controller@100 {
//!         compatible = "apple,t8103-pmgr-pwrstate", "apple,pmgr-pwrstate";
//!         reg = <0x100 0x04>;
//!         #power-domain-cells = <0x00>;
//!         #reset-cells = <0x00>;
//!         label = "sbr";
//!         apple,always-on;
//!     };
//! };
//! ```
//!
//! # Register Layout
//!
//! Each power domain has a single 32-bit register at its offset:
//! - Bit 0: Power state (0 = on, 1 = off)
//! - Bit 28: Reset assert (1 = reset asserted)
//!
//! # Usage
//!
//! Other drivers reference power domains via phandle in `power-domains` property.
//! This driver provides a global registry so that any driver can enable/disable
//! power domains and assert/deassert resets by phandle index.

#![allow(dead_code)]

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use spin::Mutex;

use crate::{
    arch::mmio,
    device::{
        DeviceInfo,
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    driver_initcall, early_println,
};

// =============================================================================
// Register Bit Definitions
// =============================================================================

/// Power state bit: 0 = powered on, 1 = powered off
const PMGR_PS_TARGET: u32 = 1 << 0;
/// Power state actual status bit
const PMGR_PS_ACTUAL: u32 = 1 << 1;
/// Reset assert bit
const PMGR_RESET: u32 = 1 << 28;
/// Index of this power domain (used as key for lookups)
const PMGR_IDX_SHIFT: u32 = 16;

// =============================================================================
// Power Domain Descriptor
// =============================================================================

/// Descriptor for a single power domain, parsed from device tree child node.
struct PowerDomain {
    /// Offset of the power domain register within the PMGR MMIO region
    offset: usize,
    /// Human-readable label from device tree
    label: alloc::string::String,
    /// Whether this domain is marked as always-on
    always_on: bool,
    /// Index of this power domain (derived from reg property, first cell)
    index: u32,
}

impl PowerDomain {
    /// Create a new power domain descriptor.
    fn new(offset: usize, index: u32, label: alloc::string::String, always_on: bool) -> Self {
        Self {
            offset,
            label,
            always_on,
            index,
        }
    }
}

// =============================================================================
// PMGR Controller Instance
// =============================================================================

/// A single PMGR controller instance (there can be multiple PMGR blocks in SoC).
struct PmgrInstance {
    /// Base virtual address of this PMGR MMIO region
    base_addr: usize,
    /// Size of the MMIO region
    size: usize,
    /// Power domains managed by this PMGR block
    domains: BTreeMap<u32, PowerDomain>,
}

impl PmgrInstance {
    /// Create a new PMGR instance.
    fn new(base_addr: usize, size: usize) -> Self {
        Self {
            base_addr,
            size,
            domains: BTreeMap::new(),
        }
    }

    /// Read the power domain register.
    #[inline]
    fn read_reg(&self, domain: &PowerDomain) -> u32 {
        // SAFETY: domain.offset is within the MMIO-mapped PMGR region
        unsafe { mmio::read32(self.base_addr + domain.offset) }
    }

    /// Write the power domain register.
    #[inline]
    fn write_reg(&self, domain: &PowerDomain, val: u32) {
        // SAFETY: domain.offset is within the MMIO-mapped PMGR region
        unsafe { mmio::write32(self.base_addr + domain.offset, val) }
    }

    /// Enable (power on) a power domain.
    ///
    /// Clears bit 0 of the power domain register to set target state to ON,
    /// then polls until actual state matches.
    fn enable(&self, domain: &PowerDomain) -> Result<(), &'static str> {
        if domain.always_on {
            return Ok(()); // Skip always-on domains
        }

        let reg = self.read_reg(domain);
        if reg & PMGR_PS_TARGET == 0 {
            // Already powered on
            return Ok(());
        }

        // Clear power state target bit to request ON
        self.write_reg(domain, reg & !PMGR_PS_TARGET);

        // Poll until actual state is ON (bit 1 = 0 means on)
        let mut timeout = 1000;
        loop {
            let val = self.read_reg(domain);
            if val & PMGR_PS_ACTUAL == 0 {
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
    /// Sets bit 0 to request OFF state, then polls until actual state matches.
    fn disable(&self, domain: &PowerDomain) -> Result<(), &'static str> {
        if domain.always_on {
            return Ok(()); // Skip always-on domains
        }

        let reg = self.read_reg(domain);
        if reg & PMGR_PS_TARGET != 0 {
            // Already powered off
            return Ok(());
        }

        // Set power state target bit to request OFF
        self.write_reg(domain, reg | PMGR_PS_TARGET);

        // Poll until actual state is OFF (bit 1 = 1 means off)
        let mut timeout = 1000;
        loop {
            let val = self.read_reg(domain);
            if val & PMGR_PS_ACTUAL != 0 {
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
    fn reset_assert(&self, domain: &PowerDomain) {
        let reg = self.read_reg(domain);
        self.write_reg(domain, reg | PMGR_RESET);
    }

    /// Deassert reset on a power domain.
    fn reset_deassert(&self, domain: &PowerDomain) {
        let reg = self.read_reg(domain);
        self.write_reg(domain, reg & !PMGR_RESET);
    }

    /// Check if a power domain is currently powered on.
    fn is_on(&self, domain: &PowerDomain) -> bool {
        let reg = self.read_reg(domain);
        // Both target and actual should be 0 for powered-on
        reg & PMGR_PS_TARGET == 0 && reg & PMGR_PS_ACTUAL == 0
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
    domain_map: BTreeMap<(u32, u32), Arc<PowerDomain>>,
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
fn get_registry() -> Option<spin::MutexGuard<'static, Option<PmgrRegistry>>> {
    let guard = PMGR_REGISTRY.lock();
    if guard.is_some() { Some(guard) } else { None }
}

// =============================================================================
// Public API
// =============================================================================

/// Result of a PMGR domain lookup.
pub struct PmgrDomain {
    inner: Arc<PmgrInstance>,
    domain: Arc<PowerDomain>,
}

impl PmgrDomain {
    /// Enable (power on) this domain.
    pub fn enable(&self) -> Result<(), &'static str> {
        self.inner.enable(&self.domain)
    }

    /// Disable (power off) this domain.
    pub fn disable(&self) -> Result<(), &'static str> {
        self.inner.disable(&self.domain)
    }

    /// Assert reset.
    pub fn reset_assert(&self) {
        self.inner.reset_assert(&self.domain)
    }

    /// Deassert reset.
    pub fn reset_deassert(&self) {
        self.inner.reset_deassert(&self.domain)
    }

    /// Check if powered on.
    pub fn is_on(&self) -> bool {
        self.inner.is_on(&self.domain)
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
    let guard = get_registry().ok_or("pmgr: registry not initialized")?;
    let registry = guard.as_ref().unwrap();

    let (pmgr_phandle, domain_index) = registry
        .pwrstate_phandles
        .get(&pwrstate_phandle)
        .ok_or("pmgr: pwrstate phandle not found")?;

    pmgr_get_domain(*pmgr_phandle, *domain_index)
}

// =============================================================================
// Platform Driver Implementation
// =============================================================================

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
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

    let base_addr = crate::vm::ioremap(paddr, size).map_err(|_| "apple-pmgr: ioremap failed")?;

    let mut registry_guard = PMGR_REGISTRY.lock();

    if registry_guard.is_none() {
        *registry_guard = Some(PmgrRegistry::new());
    }

    let registry = registry_guard.as_mut().unwrap();

    let instance = Arc::new(PmgrInstance::new(base_addr, size));

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

driver_initcall!(register_pmgr_driver);

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

    early_println!(
        "[apple-pmgr] registering domain '{}' at offset={:#x}, index={}, always_on={}",
        label,
        offset,
        index,
        always_on
    );

    let mut registry_guard = PMGR_REGISTRY.lock();
    let registry = registry_guard
        .as_mut()
        .ok_or("apple-pmgr-pwrstate: PMGR registry not initialized")?;

    let parent_phandle = device
        .parent_phandle()
        .ok_or("apple-pmgr-pwrstate: no parent phandle")?;

    let domain = Arc::new(PowerDomain::new(offset, index, label, always_on));
    registry
        .domain_map
        .insert((parent_phandle, index), Arc::clone(&domain));

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

    if let Some(ph) = pwrstate_phandle {
        registry
            .pwrstate_phandles
            .insert(ph, (parent_phandle, index));
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
