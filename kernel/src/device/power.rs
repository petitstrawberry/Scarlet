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

/// Firmware power-domain provider that resolves phandle argument cells.
pub trait PowerDomainProvider: Send + Sync {
    /// Return the number of argument cells following this provider's phandle.
    ///
    /// # Returns
    ///
    /// Value of the provider node's `#power-domain-cells` property.
    fn power_domain_cells(&self) -> usize;

    /// Resolve one firmware power-domain specifier.
    ///
    /// # Arguments
    ///
    /// * `specifier` - Cells following the provider phandle.
    ///
    /// # Returns
    ///
    /// The selected domain, or an error for an unsupported specifier.
    fn get_domain(&self, specifier: &[u32]) -> Result<Arc<dyn PowerDomain>, &'static str>;
}

struct FixedPowerDomainProvider {
    domain: Arc<dyn PowerDomain>,
}

impl PowerDomainProvider for FixedPowerDomainProvider {
    fn power_domain_cells(&self) -> usize {
        0
    }

    fn get_domain(&self, specifier: &[u32]) -> Result<Arc<dyn PowerDomain>, &'static str> {
        if !specifier.is_empty() {
            return Err("power: fixed domain does not accept argument cells");
        }
        Ok(Arc::clone(&self.domain))
    }
}

static POWER_MANAGER: IrqSpinLock<Option<PowerManagerInner>> = IrqSpinLock::new(None);

struct PowerManagerInner {
    providers: BTreeMap<u32, Arc<dyn PowerDomainProvider>>,
}

impl PowerManagerInner {
    fn new() -> Self {
        Self {
            providers: BTreeMap::new(),
        }
    }

    fn register(&mut self, phandle: u32, domain: Arc<dyn PowerDomain>) {
        self.providers.insert(
            phandle,
            Arc::new(FixedPowerDomainProvider { domain }) as Arc<dyn PowerDomainProvider>,
        );
    }

    fn register_provider(&mut self, phandle: u32, provider: Arc<dyn PowerDomainProvider>) {
        self.providers.insert(phandle, provider);
    }

    fn get_provider(&self, phandle: u32) -> Option<Arc<dyn PowerDomainProvider>> {
        self.providers.get(&phandle).cloned()
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

    /// Register a multi-domain firmware provider by phandle.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the provider node.
    /// * `provider` - Provider that resolves its declared argument cells.
    pub fn register_provider(phandle: u32, provider: Arc<dyn PowerDomainProvider>) {
        let mut guard = POWER_MANAGER.lock();
        if let Some(ref mut manager) = *guard {
            manager.register_provider(phandle, provider);
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
        guard
            .as_ref()
            .and_then(|manager| manager.get_provider(phandle))
            .and_then(|provider| {
                (provider.power_domain_cells() == 0)
                    .then(|| provider.get_domain(&[]).ok())
                    .flatten()
            })
    }

    /// Report whether a firmware power-domain provider is registered.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the provider node.
    ///
    /// # Returns
    ///
    /// `true` when a provider is available for the phandle.
    pub fn has_provider(phandle: u32) -> bool {
        POWER_MANAGER
            .lock()
            .as_ref()
            .is_some_and(|manager| manager.get_provider(phandle).is_some())
    }

    /// Resolve a firmware power-domain specifier through a registered provider.
    ///
    /// # Arguments
    ///
    /// * `phandle` - Firmware phandle identifying the provider node.
    /// * `specifier` - Argument cells following the provider phandle.
    ///
    /// # Returns
    ///
    /// The selected power domain, or an error when the provider/specifier is
    /// unavailable or invalid.
    pub fn resolve_domain(
        phandle: u32,
        specifier: &[u32],
    ) -> Result<Arc<dyn PowerDomain>, &'static str> {
        let provider = POWER_MANAGER
            .lock()
            .as_ref()
            .and_then(|manager| manager.get_provider(phandle))
            .ok_or("power: domain provider not found")?;
        provider.get_domain(specifier)
    }

    /// Enable all power domains referenced by a platform device.
    ///
    /// Each entry is decoded using the provider's declared
    /// `#power-domain-cells` width. Zero-cell fixed-domain providers and
    /// multi-domain providers can therefore coexist in one property.
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

        let cells: alloc::vec::Vec<u32> = bytes
            .chunks_exact(4)
            .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap_or([0; 4])))
            .collect();
        let mut index = 0;
        while index < cells.len() {
            let phandle = cells[index];
            index += 1;
            if phandle == 0 {
                continue;
            }

            let provider = match POWER_MANAGER
                .lock()
                .as_ref()
                .and_then(|manager| manager.get_provider(phandle))
            {
                Some(provider) => provider,
                None => {
                    crate::early_println!(
                        "[power] domain phandle={:#x} not found for {}",
                        phandle,
                        device.name()
                    );
                    // Keep the pre-provider behavior for platforms that rely on
                    // firmware/boot-loader power handoff.  DeviceManager treats
                    // ordinary power errors as non-fatal and lets the concrete
                    // driver validate the inherited hardware state.  Returning
                    // PROBE_DEFER here would make every consumer wait forever
                    // when the selected BSP omits a firmware power controller,
                    // regressing devices that worked before multi-cell provider
                    // support was added. Drivers that cannot safely use handoff
                    // must explicitly require their provider before MMIO access.
                    return Err("power: domain not found");
                }
            };
            let argument_count = provider.power_domain_cells();
            let end = index
                .checked_add(argument_count)
                .ok_or("power: domain specifier overflows")?;
            let specifier = cells
                .get(index..end)
                .ok_or("power: truncated power-domain specifier")?;
            index = end;
            let domain = provider.get_domain(specifier)?;

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

    struct IndexedProvider {
        domains: [Arc<TestPowerDomain>; 2],
    }

    impl PowerDomainProvider for IndexedProvider {
        fn power_domain_cells(&self) -> usize {
            1
        }

        fn get_domain(&self, specifier: &[u32]) -> Result<Arc<dyn PowerDomain>, &'static str> {
            let [index] = specifier else {
                return Err("test: malformed indexed domain");
            };
            self.domains
                .get(*index as usize)
                .cloned()
                .map(|domain| domain as Arc<dyn PowerDomain>)
                .ok_or("test: indexed domain out of range")
        }
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
    fn test_enable_device_domains_decodes_provider_arguments() {
        PowerManager::clear_for_test();
        PowerManager::init();

        let first = Arc::new(TestPowerDomain::new("provider-first"));
        let second = Arc::new(TestPowerDomain::new("provider-second"));
        PowerManager::register_provider(
            0x30,
            Arc::new(IndexedProvider {
                domains: [Arc::clone(&first), Arc::clone(&second)],
            }),
        );

        assert!(PowerManager::has_provider(0x30));
        assert!(!PowerManager::has_provider(0x31));
        assert_eq!(
            PowerManager::resolve_domain(0x30, &[1]).unwrap().label(),
            "provider-second"
        );
        PowerManager::enable_device_domains(&test_device(&[0x30, 1])).unwrap();

        assert!(!first.is_enabled());
        assert!(second.is_enabled());
        assert_eq!(second.enable_count.load(Ordering::SeqCst), 1);
    }

    #[test_case]
    fn test_enable_device_domains_preserves_firmware_handoff_for_unregistered_provider() {
        PowerManager::clear_for_test();
        PowerManager::init();

        assert_eq!(
            PowerManager::enable_device_domains(&test_device(&[0x40, 0])).unwrap_err(),
            "power: domain not found"
        );
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
