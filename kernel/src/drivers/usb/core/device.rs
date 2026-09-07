/// USB link speed reported by the host controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbSpeed {
    Low,
    Full,
    High,
    Super,
    SuperPlus,
}

/// USB device lifecycle state during enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbDeviceState {
    Attached,
    Powered,
    Default,
    Addressed,
    Configured,
}

/// Minimal USB device record used by Scarlet's in-tree USB stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsbDevice {
    slot_id: u8,
    port_id: u8,
    address: u8,
    speed: UsbSpeed,
    state: UsbDeviceState,
}

impl UsbDevice {
    /// Creates a new USB device record for a connected port.
    pub const fn new(slot_id: u8, port_id: u8, speed: UsbSpeed) -> Self {
        Self {
            slot_id,
            port_id,
            address: 0,
            speed,
            state: UsbDeviceState::Attached,
        }
    }

    /// Returns the xHCI slot ID assigned to the device.
    pub const fn slot_id(&self) -> u8 {
        self.slot_id
    }

    /// Returns the root-hub port number where the device is attached.
    pub const fn port_id(&self) -> u8 {
        self.port_id
    }

    /// Returns the USB device address, or zero before assignment.
    pub const fn address(&self) -> u8 {
        self.address
    }

    /// Returns the negotiated link speed.
    pub const fn speed(&self) -> UsbSpeed {
        self.speed
    }

    /// Returns the current device state.
    pub const fn state(&self) -> UsbDeviceState {
        self.state
    }

    /// Updates the enumeration state.
    pub fn set_state(&mut self, state: UsbDeviceState) {
        self.state = state;
    }

    /// Assigns a USB address and transitions to the addressed state.
    pub fn assign_address(&mut self, address: u8) {
        self.address = address;
        self.state = UsbDeviceState::Addressed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_usb_device_state_transitions() {
        let mut device = UsbDevice::new(1, 2, UsbSpeed::High);
        assert_eq!(device.state(), UsbDeviceState::Attached);
        assert_eq!(device.address(), 0);

        device.assign_address(5);
        assert_eq!(device.address(), 5);
        assert_eq!(device.state(), UsbDeviceState::Addressed);
    }
}
