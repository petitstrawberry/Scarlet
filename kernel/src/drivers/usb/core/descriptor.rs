use core::mem::size_of;

/// Standard USB descriptor header shared by all descriptor payloads.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DescriptorHeader {
    pub length: u8,
    pub descriptor_type: u8,
}

/// USB device descriptor as defined by the USB 2.0 specification.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub usb_version: u16,
    pub device_class: u8,
    pub device_subclass: u8,
    pub device_protocol: u8,
    pub max_packet_size0: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub device_version: u16,
    pub manufacturer_index: u8,
    pub product_index: u8,
    pub serial_number_index: u8,
    pub num_configurations: u8,
}

/// USB configuration descriptor.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigurationDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub total_length: u16,
    pub num_interfaces: u8,
    pub configuration_value: u8,
    pub configuration_index: u8,
    pub attributes: u8,
    pub max_power: u8,
}

/// USB interface descriptor.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub interface_number: u8,
    pub alternate_setting: u8,
    pub num_endpoints: u8,
    pub interface_class: u8,
    pub interface_subclass: u8,
    pub interface_protocol: u8,
    pub interface_index: u8,
}

/// USB endpoint descriptor.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub endpoint_address: u8,
    pub attributes: u8,
    pub max_packet_size: u16,
    pub interval: u8,
}

impl DeviceDescriptor {
    /// Returns the encoded descriptor size in bytes.
    pub const fn encoded_size() -> usize {
        size_of::<Self>()
    }
}

impl ConfigurationDescriptor {
    /// Returns the encoded descriptor size in bytes.
    pub const fn encoded_size() -> usize {
        size_of::<Self>()
    }
}

impl InterfaceDescriptor {
    /// Returns the encoded descriptor size in bytes.
    pub const fn encoded_size() -> usize {
        size_of::<Self>()
    }
}

impl EndpointDescriptor {
    /// Returns the encoded descriptor size in bytes.
    pub const fn encoded_size() -> usize {
        size_of::<Self>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_standard_descriptor_sizes() {
        assert_eq!(DeviceDescriptor::encoded_size(), 18);
        assert_eq!(ConfigurationDescriptor::encoded_size(), 9);
        assert_eq!(InterfaceDescriptor::encoded_size(), 9);
        assert_eq!(EndpointDescriptor::encoded_size(), 7);
    }
}
