/// USB host controller provider registered by bus-specific host drivers.
pub trait UsbHostController: Send + Sync {
    /// Poll and handle pending host-controller events.
    ///
    /// # Returns
    ///
    /// This method reports errors internally because it is used from generic
    /// host polling paths that cannot act on controller-specific failures.
    fn poll_events(&self);
}

/// USB data role reported by a Type-C port controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsbDataRole {
    /// No active USB data connection is present.
    None,
    /// The local port acts as a USB host.
    Host,
    /// The local port acts as a USB device.
    Device,
}

/// Type-C plug orientation reported by a Type-C port controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypecOrientation {
    /// No active plug orientation is available.
    None,
    /// Plug is in the normal orientation.
    Normal,
    /// Plug is in the reverse orientation.
    Reverse,
}

/// Snapshot of a Type-C port controller status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TypecPortStatus {
    /// Whether a plug is currently present.
    pub connected: bool,
    /// Whether the controller reports an active USB2 data connection.
    pub usb2: bool,
    /// Whether the controller reports an active USB3 data connection.
    pub usb3: bool,
    /// Current USB data role.
    pub data_role: UsbDataRole,
    /// Current plug orientation.
    pub orientation: TypecOrientation,
    /// Raw vendor status register value.
    pub raw_status: u32,
    /// Raw vendor power-status register value.
    pub raw_power_status: u32,
    /// Raw vendor data-status register value.
    pub raw_data_status: u32,
}

/// Type-C port provider exposed by connector controller drivers.
pub trait TypecPort: Send + Sync {
    /// Return a stable provider name for logging and diagnostics.
    ///
    /// # Returns
    ///
    /// Static provider name.
    fn name(&self) -> &'static str;

    /// Read the current Type-C port status.
    ///
    /// # Returns
    ///
    /// Current port status, or an error string when the controller cannot be queried.
    fn status(&self) -> Result<TypecPortStatus, &'static str>;
}
