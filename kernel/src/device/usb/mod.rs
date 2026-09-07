extern crate alloc;

use alloc::sync::Arc;

/// USB host controller provider registered by bus-specific host drivers.
pub trait UsbHostController: Send + Sync {}

/// Stable USB device identity exposed to external interface drivers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UsbDeviceIdentity {
    /// USB vendor ID from the device descriptor.
    pub vendor_id: u16,
    /// USB product ID from the device descriptor.
    pub product_id: u16,
    /// Device-level USB class code.
    pub device_class: u8,
    /// Device-level USB subclass code.
    pub device_subclass: u8,
    /// Device-level USB protocol code.
    pub device_protocol: u8,
}

/// Stable physical topology of an enumerated USB device.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct UsbDeviceLocation {
    /// Scarlet-assigned host-controller identifier.
    pub host_id: u32,
    /// One-based root-hub port number.
    pub root_port_id: u8,
    /// xHCI-compatible downstream hub route string.
    pub route_string: u32,
}

/// Descriptor information for one interrupt-IN endpoint and its interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UsbInterruptInEndpointInfo {
    /// Configuration value containing the interface.
    pub configuration_value: u8,
    /// Interface number containing the endpoint.
    pub interface_number: u8,
    /// Alternate setting containing the endpoint.
    pub alternate_setting: u8,
    /// Interface class code.
    pub interface_class: u8,
    /// Interface subclass code.
    pub interface_subclass: u8,
    /// Interface protocol code.
    pub interface_protocol: u8,
    /// Interrupt-IN endpoint address.
    pub endpoint_address: u8,
    /// Maximum interrupt packet size advertised by the endpoint.
    pub max_packet_size: u16,
    /// USB polling interval advertised by the endpoint.
    pub interval: u8,
}

/// Runtime callback for an externally claimed USB interrupt-IN interface.
pub trait UsbInterruptInHandler: Send + Sync {
    /// Consume one successfully completed interrupt-IN payload.
    ///
    /// # Arguments
    ///
    /// * `report` - Exact payload bytes returned by the USB transfer.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the payload was accepted, or a static diagnostic string
    /// when the device-specific decoder rejects it.
    fn handle_report(&self, report: &[u8]) -> Result<(), &'static str>;

    /// Notify the interface that its USB device was disconnected.
    ///
    /// Implementations release transient input state here while retaining any
    /// stable Scarlet device identity needed for a later reconnection.
    fn disconnected(&self);
}

/// External driver for a USB interface backed by one interrupt-IN endpoint.
pub trait UsbInterruptInDriver: Send + Sync {
    /// Return a stable driver name for diagnostics.
    ///
    /// # Returns
    ///
    /// Static driver name.
    fn name(&self) -> &'static str;

    /// Decide whether this driver owns an enumerated interrupt-IN endpoint.
    ///
    /// # Arguments
    ///
    /// * `device` - Device descriptor identity.
    /// * `endpoint` - Interface and endpoint descriptor information.
    ///
    /// # Returns
    ///
    /// `true` when the driver wants Scarlet's host controller to configure and
    /// bind this endpoint.
    fn matches(&self, device: &UsbDeviceIdentity, endpoint: &UsbInterruptInEndpointInfo) -> bool;

    /// Bind a matched interface after its endpoint is configured.
    ///
    /// # Arguments
    ///
    /// * `device` - Device descriptor identity.
    /// * `endpoint` - Claimed interface and endpoint descriptor information.
    /// * `location` - Stable physical topology used to recognize reconnects.
    ///
    /// # Returns
    ///
    /// A report/disconnect handler, or a static error string when binding
    /// fails.
    fn bind(
        &self,
        device: &UsbDeviceIdentity,
        endpoint: &UsbInterruptInEndpointInfo,
        location: UsbDeviceLocation,
    ) -> Result<Arc<dyn UsbInterruptInHandler>, &'static str>;
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
