//! Pin-controller provider interface.
//!
//! The device core resolves pinctrl state phandles and decodes standard pin
//! configuration properties. SoC-specific providers remain responsible for
//! interpreting pin/group and function names and programming their hardware.

extern crate alloc;

use alloc::vec::Vec;

/// Bias configuration requested by a firmware pinctrl state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PinctrlBias {
    /// Disable the controller's internal pull resistor.
    Disable,
    /// Enable an internal pull-down resistor.
    PullDown,
    /// Enable an internal pull-up resistor.
    PullUp,
}

/// Provider-neutral pinctrl state decoded from firmware.
pub struct PinctrlState<'a> {
    /// Provider-specific pin or group names.
    pub pins: Vec<&'a str>,
    /// Optional provider-specific mux function name.
    pub function: Option<&'a str>,
    /// Optional bias configuration.
    pub bias: Option<PinctrlBias>,
    /// Optional drive strength in milliamps.
    pub drive_strength_ma: Option<u32>,
    /// Optional output value; `None` preserves the current direction/value.
    pub output: Option<bool>,
    /// Whether the state explicitly requests input-enable.
    pub input_enable: bool,
}

/// Error returned while applying a pinctrl state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PinctrlError {
    /// The provider does not implement one of the requested names/settings.
    Unsupported,
    /// The state contains an invalid pin, function, or electrical setting.
    Invalid,
}

/// SoC-specific pin-controller provider.
pub trait PinctrlController: Send + Sync {
    /// Validate one decoded firmware pinctrl state without changing hardware.
    ///
    /// The device core uses this method to preflight every child of a nested
    /// pinctrl container before applying any of them. Providers must return
    /// `Ok(())` only when a subsequent [`Self::apply_state`] for the same
    /// state is supported and will return `Ok`. This lets the device core
    /// safely apply a validated nested container without an error from a later
    /// child leaving an earlier child configured.
    ///
    /// # Arguments
    ///
    /// * `state` - Provider-neutral state whose names and settings are to be
    ///   checked.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the state is safe to apply, or a provider error. The
    /// default conservatively reports unsupported so existing providers cannot
    /// cause a partially applied nested state until they implement validation.
    fn validate_state(&self, _state: &PinctrlState<'_>) -> Result<(), PinctrlError> {
        Err(PinctrlError::Unsupported)
    }

    /// Apply one decoded firmware pinctrl state.
    ///
    /// # Arguments
    ///
    /// * `state` - Provider-neutral state whose names are interpreted by this
    ///   controller.
    ///
    /// # Returns
    ///
    /// Number of pins/groups configured, or a provider error.
    ///
    /// When [`Self::validate_state`] returned `Ok(())` for this state, this
    /// method must also return `Ok` and may then program hardware.
    fn apply_state(&self, state: &PinctrlState<'_>) -> Result<usize, PinctrlError>;
}
