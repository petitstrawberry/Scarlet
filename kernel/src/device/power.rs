//! Power-domain registration and device power sequencing.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::Mutex;

use crate::device::DeviceInfo;
use crate::device::platform::PlatformDeviceInfo;

/// Power-domain operations exposed by platform PM controllers.
pub trait PowerDomain: Send + Sync {
    /// Enable the power domain.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` once the domain is usable, or an error string if the
    /// controller could not enable it.
    fn enable(&self) -> Result<(), &'static str>;

    /// Disable the power domain.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` once the domain is disabled, or an error string if the
    /// controller could not disable it.
    fn disable(&self) -> Result<(), &'static str>;

    /// Report whether the power domain is currently enabled.
    ///
    /// # Returns
    ///
    /// Returns `true` if the domain is active.
    fn is_enabled(&self) -> bool;

    /// Return a human-readable domain label.
    ///
    /// # Returns
    ///
    /// The label supplied by the platform firmware or driver.
    fn label(&self) -> &str;
}

static POWER_MANAGER: Mutex<Option<PowerManagerInner>> = Mutex::new(None);

struct PowerManagerInner {
    domains: BTreeMap<u32, Arc<dyn PowerDomain>>,
}

impl PowerManagerInner {
    fn new() -> Self {
        Self {
            domains: BTreeMap::new(),
        }
    }

    fn register(&mut self, phandle: u32, domain: Arc<dyn PowerDomain>) {
        self.domains.insert(phandle, domain);
    }

    fn get(&self, phandle: u32) -> Option<Arc<dyn PowerDomain>> {
        self.domains.get(&phandle).cloned()
    }
}

pub struct PowerManager;

impl PowerManager {
    pub fn init() {
        let mut guard = POWER_MANAGER.lock();
        if guard.is_none() {
            *guard = Some(PowerManagerInner::new());
        }
    }

    pub fn register_domain(phandle: u32, domain: Arc<dyn PowerDomain>) {
        let mut guard = POWER_MANAGER.lock();
        if let Some(ref mut mgr) = *guard {
            mgr.register(phandle, domain);
        }
    }

    pub fn get_domain(phandle: u32) -> Option<Arc<dyn PowerDomain>> {
        let guard = POWER_MANAGER.lock();
        guard.as_ref().and_then(|mgr| mgr.get(phandle))
    }

    pub fn enable_device_domains(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
        let pd_prop = match device.property("power-domains") {
            Some(p) => p,
            None => return Ok(()),
        };

        let bytes = pd_prop.value();
        if bytes.len() < 4 {
            return Ok(());
        }

        let phandle = u32::from_be_bytes(bytes[0..4].try_into().unwrap_or([0; 4]));

        let domain = match Self::get_domain(phandle) {
            Some(d) => d,
            None => {
                crate::early_println!(
                    "[power] domain phandle={:#x} not found for {}",
                    phandle,
                    device.name()
                );
                return Err("power: domain not found");
            }
        };

        if !domain.is_enabled() {
            crate::early_println!(
                "[power] enabling domain '{}' for {}",
                domain.label(),
                device.name()
            );
            if let Err(e) = domain.enable() {
                crate::early_println!(
                    "[power] failed to enable domain '{}': {} (continuing)",
                    domain.label(),
                    e
                );
            }
        }
        Ok(())
    }
}
