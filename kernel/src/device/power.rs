//! Power-domain registration and device power sequencing.

use crate::sync::IrqSpinLock;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;

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

    /// Report whether this domain needs an externally running clock before enable.
    ///
    /// # Returns
    ///
    /// `true` when the owning device driver must enable the required clock
    /// before powering this domain.
    fn requires_external_clock(&self) -> bool {
        false
    }
}

static POWER_MANAGER: IrqSpinLock<Option<PowerManagerInner>> = IrqSpinLock::new(None);

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

/// Global power-domain registry.
pub struct PowerManager;

impl PowerManager {
    /// Initialize the global power manager.
    ///
    /// Calling this more than once is harmless.
    pub fn init() {
        let mut guard = POWER_MANAGER.lock();
        if guard.is_none() {
            *guard = Some(PowerManagerInner::new());
        }
    }

    /// Register a power domain by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the power-domain node.
    /// * `domain` - Power-domain implementation.
    pub fn register_domain(phandle: u32, domain: Arc<dyn PowerDomain>) {
        let mut guard = POWER_MANAGER.lock();
        if let Some(ref mut mgr) = *guard {
            mgr.register(phandle, domain);
        }
    }

    /// Look up a registered power domain by firmware phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the power-domain node.
    ///
    /// # Returns
    ///
    /// Registered power domain, or `None` when missing.
    pub fn get_domain(phandle: u32) -> Option<Arc<dyn PowerDomain>> {
        let guard = POWER_MANAGER.lock();
        guard.as_ref().and_then(|mgr| mgr.get(phandle))
    }

    /// Enable all power domains referenced by a platform device.
    ///
    /// The current power-domain registry resolves domains by phandle, so this
    /// helper expects `power-domains` to contain one or more phandle-only
    /// entries. This matches Apple pwrstate nodes, including devices that list
    /// several independent domains.
    ///
    /// # Arguments
    ///
    /// * `device` - Platform device containing an optional `power-domains` property.
    ///
    /// # Returns
    ///
    /// `Ok(())` when all referenced domains were enabled or no domains exist.
    pub fn enable_device_domains(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
        let pd_prop = match device.property("power-domains") {
            Some(p) => p,
            None => return Ok(()),
        };

        let bytes = pd_prop.value();
        if bytes.is_empty() {
            return Ok(());
        }
        if bytes.len() % 4 != 0 {
            return Err("power: malformed power-domains");
        }

        for chunk in bytes.chunks_exact(4) {
            let phandle = u32::from_be_bytes(chunk.try_into().unwrap_or([0; 4]));
            if phandle == 0 {
                continue;
            }

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

            if domain.requires_external_clock() {
                crate::early_println!(
                    "[power] deferring externally-clocked domain '{}' for {}",
                    domain.label(),
                    device.name()
                );
                continue;
            }

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
        }
        Ok(())
    }

    #[cfg(test)]
    fn clear_for_test() {
        *POWER_MANAGER.lock() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use crate::device::platform::PlatformDeviceProperty;

    struct TestPowerDomain {
        label: &'static str,
        enabled: AtomicBool,
        enable_count: AtomicUsize,
    }

    impl TestPowerDomain {
        fn new(label: &'static str) -> Self {
            Self {
                label,
                enabled: AtomicBool::new(false),
                enable_count: AtomicUsize::new(0),
            }
        }
    }

    impl PowerDomain for TestPowerDomain {
        fn enable(&self) -> Result<(), &'static str> {
            self.enabled.store(true, Ordering::SeqCst);
            self.enable_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn disable(&self) -> Result<(), &'static str> {
            self.enabled.store(false, Ordering::SeqCst);
            Ok(())
        }

        fn is_enabled(&self) -> bool {
            self.enabled.load(Ordering::SeqCst)
        }

        fn label(&self) -> &str {
            self.label
        }
    }

    fn be_cells(cells: &[u32]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for cell in cells {
            bytes.extend_from_slice(&cell.to_be_bytes());
        }
        bytes
    }

    fn test_device(power_domains: &[u32]) -> PlatformDeviceInfo {
        PlatformDeviceInfo::new(
            "power-test-device",
            0,
            alloc::vec!["test,power-device"],
            alloc::vec![],
            alloc::vec![PlatformDeviceProperty::new(
                "power-domains",
                &be_cells(power_domains),
            )],
            None,
        )
    }

    #[test_case]
    fn test_enable_device_domains_enables_all_domains() {
        PowerManager::clear_for_test();
        PowerManager::init();

        let first = Arc::new(TestPowerDomain::new("first"));
        let second = Arc::new(TestPowerDomain::new("second"));
        PowerManager::register_domain(0x10, first.clone());
        PowerManager::register_domain(0x20, second.clone());

        PowerManager::enable_device_domains(&test_device(&[0x10, 0x20])).unwrap();

        assert!(first.is_enabled());
        assert!(second.is_enabled());
        assert_eq!(first.enable_count.load(Ordering::SeqCst), 1);
        assert_eq!(second.enable_count.load(Ordering::SeqCst), 1);
    }

    #[test_case]
    fn test_enable_device_domains_rejects_malformed_property() {
        PowerManager::clear_for_test();
        PowerManager::init();

        let device = PlatformDeviceInfo::new(
            "power-test-device",
            0,
            alloc::vec!["test,power-device"],
            alloc::vec![],
            alloc::vec![PlatformDeviceProperty::new("power-domains", &[0, 1])],
            None,
        );

        assert_eq!(
            PowerManager::enable_device_domains(&device).unwrap_err(),
            "power: malformed power-domains"
        );
    }
}
