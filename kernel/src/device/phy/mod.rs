//! PHY subsystem abstractions.
//!
//! The PHY subsystem exposes provider-neutral PHY handles with reference-counted
//! power state and optional mode selection for bus controllers and device drivers.

extern crate alloc;

use alloc::sync::Arc;
use spin::Mutex;

/// PHY operation errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhyError {
    /// Requested PHY or specifier was not found.
    NotFound,
    /// Operation is not supported by this PHY.
    NotSupported,
    /// Requested PHY mode is invalid for this PHY.
    InvalidMode,
    /// Power-on sequencing failed.
    PowerOnFailed,
    /// Power-off sequencing failed.
    PowerOffFailed,
    /// Reset sequencing failed.
    ResetFailed,
    /// PHY is busy and cannot satisfy the operation.
    Busy,
    /// Operation timed out before completion.
    Timeout,
    /// Hardware access failed.
    HardwareError,
}

/// Generic PHY operating mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhyMode {
    /// USB host controller mode.
    UsbHost,
    /// USB device controller mode.
    UsbDevice,
    /// USB OTG dual-role mode.
    UsbOtg,
    /// PCI Express mode.
    Pcie,
    /// SATA mode.
    Sata,
    /// DisplayPort mode.
    DisplayPort,
    /// MIPI mode.
    Mipi,
    /// Provider-specific mode value.
    Other(u32),
}

/// Consumer-side PHY handle with refcounted power state.
#[derive(Clone)]
pub struct PhyHandle {
    phy: Arc<dyn Phy>,
    state: Arc<Mutex<PhyState>>,
}

struct PhyState {
    power_count: u32,
    mode: Option<PhyMode>,
}

impl PhyHandle {
    /// Create a new PHY handle.
    ///
    /// # Arguments
    ///
    /// * `phy` - PHY implementation to wrap.
    ///
    /// # Returns
    ///
    /// A handle with zero power users and no selected mode.
    pub fn new(phy: Arc<dyn Phy>) -> Self {
        Self {
            phy,
            state: Arc::new(Mutex::new(PhyState {
                power_count: 0,
                mode: None,
            })),
        }
    }

    /// Power on the PHY with reference counting.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the PHY is powered or was already powered by another user.
    pub fn power_on(&self) -> Result<(), PhyError> {
        let mut state = self.state.lock();
        if state.power_count == 0 {
            self.phy.power_on()?;
        }
        state.power_count = state.power_count.saturating_add(1);
        Ok(())
    }

    /// Drop one power reference and power off when the final user releases it.
    ///
    /// The reference count saturates at zero. Power-off errors are intentionally
    /// ignored because this method is used for best-effort cleanup paths.
    pub fn power_off(&self) {
        let call_power_off = {
            let mut state = self.state.lock();
            if state.power_count == 0 {
                false
            } else {
                state.power_count -= 1;
                state.power_count == 0
            }
        };

        if call_power_off {
            let _ = self.phy.power_off();
        }
    }

    /// Set the PHY operating mode.
    ///
    /// # Arguments
    ///
    /// * `mode` - Requested PHY mode.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the mode was accepted by the PHY.
    pub fn set_mode(&self, mode: PhyMode) -> Result<(), PhyError> {
        self.phy.set_mode(mode)?;
        self.state.lock().mode = Some(mode);
        Ok(())
    }

    /// Reset the PHY.
    ///
    /// # Returns
    ///
    /// `Ok(())` when reset completed successfully.
    pub fn reset(&self) -> Result<(), PhyError> {
        self.phy.reset()
    }

    /// Check whether this handle currently holds any power references.
    ///
    /// # Returns
    ///
    /// `true` when the handle's shared power count is non-zero.
    pub fn is_powered(&self) -> bool {
        self.state.lock().power_count > 0
    }

    /// Return the last mode set through this handle.
    ///
    /// # Returns
    ///
    /// The cached mode, or `None` when no mode was set.
    pub fn mode(&self) -> Option<PhyMode> {
        self.state.lock().mode
    }
}

/// A single PHY instance.
pub trait Phy: Send + Sync {
    /// Return the PHY name.
    ///
    /// # Returns
    ///
    /// Static name used for logging and diagnostics.
    fn name(&self) -> &'static str;

    /// Power on the PHY hardware.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the PHY is powered on.
    fn power_on(&self) -> Result<(), PhyError>;

    /// Power off the PHY hardware.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the PHY is powered off.
    fn power_off(&self) -> Result<(), PhyError>;

    /// Reset the PHY hardware.
    ///
    /// # Returns
    ///
    /// `Ok(())` when reset completed successfully.
    fn reset(&self) -> Result<(), PhyError>;

    /// Set the PHY operating mode.
    ///
    /// # Arguments
    ///
    /// * `mode` - Requested PHY mode.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the PHY accepted the mode.
    fn set_mode(&self, mode: PhyMode) -> Result<(), PhyError>;

    /// Return the PHY operating mode.
    ///
    /// # Returns
    ///
    /// Current PHY mode, or `None` when unknown.
    fn get_mode(&self) -> Option<PhyMode>;
}

/// PHY controller that exposes one or more PHY instances.
pub trait PhyProvider: Send + Sync {
    /// Return the provider name.
    ///
    /// # Returns
    ///
    /// Static provider name used for diagnostics.
    fn name(&self) -> &'static str;

    /// Return the number of cells after the phandle in the `phys` property.
    ///
    /// # Returns
    ///
    /// Number of provider-specific `u32` cells required to identify a PHY.
    fn phy_cells(&self) -> usize;

    /// Resolve a PHY by provider-specific specifier cells.
    ///
    /// # Arguments
    ///
    /// * `spec` - Provider-specific cells from the `phys` property.
    ///
    /// # Returns
    ///
    /// PHY handle for the requested specifier.
    fn get_phy(&self, spec: &[u32]) -> Result<PhyHandle, PhyError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    struct TestPhy {
        power_on_count: AtomicUsize,
        power_off_count: AtomicUsize,
        reset_count: AtomicUsize,
        mode: Mutex<Option<PhyMode>>,
    }

    impl TestPhy {
        fn new() -> Self {
            Self {
                power_on_count: AtomicUsize::new(0),
                power_off_count: AtomicUsize::new(0),
                reset_count: AtomicUsize::new(0),
                mode: Mutex::new(None),
            }
        }
    }

    impl Phy for TestPhy {
        fn name(&self) -> &'static str {
            "test-phy"
        }

        fn power_on(&self) -> Result<(), PhyError> {
            self.power_on_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn power_off(&self) -> Result<(), PhyError> {
            self.power_off_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn reset(&self) -> Result<(), PhyError> {
            self.reset_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn set_mode(&self, mode: PhyMode) -> Result<(), PhyError> {
            *self.mode.lock() = Some(mode);
            Ok(())
        }

        fn get_mode(&self) -> Option<PhyMode> {
            *self.mode.lock()
        }
    }

    #[test_case]
    fn test_phy_handle_power_refcount() {
        let phy_impl = Arc::new(TestPhy::new());
        let phy = PhyHandle::new(phy_impl.clone());

        assert!(phy.power_on().is_ok());
        assert!(phy.power_on().is_ok());
        assert!(phy.is_powered());
        assert_eq!(phy_impl.power_on_count.load(Ordering::SeqCst), 1);

        phy.power_off();
        assert!(phy.is_powered());
        assert_eq!(phy_impl.power_off_count.load(Ordering::SeqCst), 0);

        phy.power_off();
        assert!(!phy.is_powered());
        assert_eq!(phy_impl.power_off_count.load(Ordering::SeqCst), 1);

        phy.power_off();
        assert_eq!(phy_impl.power_off_count.load(Ordering::SeqCst), 1);
    }

    #[test_case]
    fn test_phy_handle_mode_set_get_and_reset() {
        let phy_impl = Arc::new(TestPhy::new());
        let phy = PhyHandle::new(phy_impl.clone());

        assert_eq!(phy.mode(), None);
        assert_eq!(phy_impl.get_mode(), None);
        assert!(phy.set_mode(PhyMode::UsbHost).is_ok());
        assert_eq!(phy.mode(), Some(PhyMode::UsbHost));
        assert_eq!(phy_impl.get_mode(), Some(PhyMode::UsbHost));

        assert!(phy.reset().is_ok());
        assert_eq!(phy_impl.reset_count.load(Ordering::SeqCst), 1);
    }
}
