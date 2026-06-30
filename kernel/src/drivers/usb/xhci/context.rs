//! xHCI Device Context structures for slot management.

use core::mem::size_of;

pub const DEVICE_CONTEXT_ENTRIES: usize = 32;
pub const INPUT_CONTEXT_ENTRIES: usize = 33;
pub const ADDRESS_DEVICE_INPUT_CONTEXT_ENTRIES: usize = 3;

/// xHCI slot context (32 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SlotContext {
    pub route_string: u32,
    pub speed_flags: u32,
    pub tt_info: u32,
    pub interrupter_state: u32,
    pub device_address: u32,
    pub reserved_20: u32,
    pub reserved_24: u32,
    pub reserved_28: u32,
}

impl SlotContext {
    pub const fn size() -> usize {
        size_of::<Self>()
    }

    pub fn set_speed(&mut self, speed: u8) {
        self.route_string = (self.route_string & !(0xF << 20)) | ((speed as u32) << 20);
    }

    pub fn set_context_entries(&mut self, entries: u8) {
        self.route_string = (self.route_string & !(0x1F << 27)) | ((entries as u32) << 27);
    }

    pub fn set_max_exit_latency(&mut self, latency: u16) {
        self.speed_flags = (self.speed_flags & !0xFFFF) | (latency as u32);
    }

    pub fn set_root_hub_port(&mut self, port: u8) {
        self.speed_flags = (self.speed_flags & !(0xFF << 16)) | ((port as u32) << 16);
    }

    pub fn set_interrupter_target(&mut self, interrupter: u16) {
        self.tt_info = (self.tt_info & !(0x3ff << 22)) | ((interrupter as u32) << 22);
    }
}

/// xHCI endpoint context (32 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct EndpointContext {
    pub dword0: u32,
    pub dword1: u32,
    pub tr_dequeue_low: u32,
    pub tr_dequeue_high: u32,
    pub dword4: u32,
    pub reserved_14: u32,
    pub reserved_18: u32,
    pub reserved_1c: u32,
}

impl EndpointContext {
    pub const fn size() -> usize {
        size_of::<Self>()
    }

    pub fn set_endpoint_type(&mut self, ep_type: u8) {
        self.dword1 = (self.dword1 & !(0x7 << 3)) | ((ep_type as u32) << 3);
    }

    pub fn set_endpoint_state(&mut self, state: u8) {
        self.dword0 = (self.dword0 & !0x7) | ((state as u32) & 0x7);
    }

    pub fn set_error_count(&mut self, count: u8) {
        self.dword1 = (self.dword1 & !(0x3 << 1)) | (((count as u32) & 0x3) << 1);
    }

    pub fn set_max_packet_size(&mut self, max_packet: u16) {
        self.dword1 = (self.dword1 & !(0xFFFF << 16)) | ((max_packet as u32) << 16);
    }

    pub fn set_dequeue_pointer(&mut self, paddr: u64) {
        let pointer = paddr & !0xFu64;
        self.tr_dequeue_low = pointer as u32;
        self.tr_dequeue_high = (pointer >> 32) as u32;
    }

    pub fn set_dequeue_cycle(&mut self, cycle: bool) {
        if cycle {
            self.tr_dequeue_low |= 1;
        } else {
            self.tr_dequeue_low &= !1;
        }
    }

    pub fn set_max_burst_size(&mut self, burst: u8) {
        self.dword1 = (self.dword1 & !(0xFF << 8)) | ((burst as u32) << 8);
    }

    pub fn set_interval(&mut self, interval: u8) {
        self.dword0 = (self.dword0 & !(0xFF << 16)) | ((interval as u32) << 16);
    }

    pub fn set_average_trb_length(&mut self, length: u16) {
        self.dword4 = (self.dword4 & !0xFFFF) | (length as u32);
    }
}

/// Endpoint type values.
pub mod ep_type {
    pub const ISOCH_OUT: u8 = 1;
    pub const BULK_OUT: u8 = 2;
    pub const INTERRUPT_OUT: u8 = 3;
    pub const CONTROL: u8 = 4;
    pub const ISOCH_IN: u8 = 5;
    pub const BULK_IN: u8 = 6;
    pub const INTERRUPT_IN: u8 = 7;
}

/// USB device speed values.
pub mod speed {
    pub const FULL: u8 = 1;
    pub const LOW: u8 = 2;
    pub const HIGH: u8 = 3;
    pub const SUPER: u8 = 4;
}

/// xHCI input control context (32 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct InputControlContext {
    pub drop_context_flags: u32,
    pub add_context_flags: u32,
    pub reserved_08: u32,
    pub configuration_value: u32,
    pub alternate_settings: u32,
    pub reserved_14: u32,
    pub reserved_18: u32,
    pub reserved_1c: u32,
}

impl InputControlContext {
    pub const fn size() -> usize {
        size_of::<Self>()
    }

    pub fn add_slot_context(&mut self) {
        self.add_context_flags |= 1;
    }

    pub fn add_endpoint(&mut self, ep_index: u8) {
        self.add_context_flags |= 1 << ep_index;
    }

    pub fn drop_endpoint(&mut self, ep_index: u8) {
        self.drop_context_flags |= 1 << ep_index;
    }
}

/// xHCI input context for Address Device and Configure Endpoint commands.
#[repr(C, align(64))]
#[derive(Debug, Clone, Copy, Default)]
pub struct InputContext {
    pub control: InputControlContext,
    pub slot: SlotContext,
    pub endpoint0: EndpointContext,
}

impl InputContext {
    pub const fn size() -> usize {
        size_of::<Self>()
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn configure_for_address(&mut self, slot: u8, port: u8, speed: u8) {
        let _ = slot;
        self.control.add_slot_context();
        self.control.add_endpoint(1);

        self.slot.set_speed(speed);
        self.slot.set_context_entries(1);
        self.slot.set_max_exit_latency(0);
        self.slot.set_root_hub_port(port);
        self.slot.set_interrupter_target(0);
        self.slot.device_address = 0;

        self.endpoint0.set_endpoint_state(0);
        self.endpoint0.set_endpoint_type(ep_type::CONTROL);
        self.endpoint0.set_error_count(3);
        self.endpoint0.set_max_burst_size(0);
        self.endpoint0
            .set_max_packet_size(Self::default_max_packet(speed));
        self.endpoint0.set_average_trb_length(8);
    }

    pub fn interrupt_endpoint_context(
        dci: u8,
        max_packet_size: u16,
        interval: u8,
        dequeue_pointer: u64,
        is_in: bool,
    ) -> (u8, EndpointContext) {
        let mut endpoint = EndpointContext::default();
        endpoint.set_endpoint_state(0);
        endpoint.set_endpoint_type(if is_in {
            ep_type::INTERRUPT_IN
        } else {
            ep_type::INTERRUPT_OUT
        });
        endpoint.set_error_count(3);
        endpoint.set_max_burst_size(0);
        endpoint.set_max_packet_size(max_packet_size);
        endpoint.set_interval(interval);
        endpoint.set_dequeue_pointer(dequeue_pointer);
        endpoint.set_dequeue_cycle(true);
        endpoint.set_average_trb_length(max_packet_size);
        (dci, endpoint)
    }

    /// Build an endpoint context for a bulk endpoint.
    ///
    /// # Arguments
    ///
    /// * `dci` - xHCI device context index for the endpoint.
    /// * `max_packet_size` - USB endpoint maximum packet size.
    /// * `dequeue_pointer` - Physical address of the endpoint transfer ring.
    /// * `is_in` - Whether this endpoint transfers device-to-host data.
    ///
    /// # Returns
    ///
    /// The DCI and initialized endpoint context.
    pub fn bulk_endpoint_context(
        dci: u8,
        max_packet_size: u16,
        dequeue_pointer: u64,
        is_in: bool,
    ) -> (u8, EndpointContext) {
        let mut endpoint = EndpointContext::default();
        endpoint.set_endpoint_state(0);
        endpoint.set_endpoint_type(if is_in {
            ep_type::BULK_IN
        } else {
            ep_type::BULK_OUT
        });
        endpoint.set_error_count(3);
        endpoint.set_max_burst_size(0);
        endpoint.set_max_packet_size(max_packet_size);
        endpoint.set_interval(0);
        endpoint.set_dequeue_pointer(dequeue_pointer);
        endpoint.set_dequeue_cycle(true);
        endpoint.set_average_trb_length(max_packet_size);
        (dci, endpoint)
    }

    fn default_max_packet(speed: u8) -> u16 {
        match speed {
            speed::LOW => 8,
            speed::FULL => 8,
            speed::HIGH => 64,
            speed::SUPER => 512,
            _ => 64,
        }
    }
}

pub const fn device_context_bytes(context_size: usize) -> usize {
    context_size * DEVICE_CONTEXT_ENTRIES
}

pub const fn full_input_context_bytes(context_size: usize) -> usize {
    context_size * INPUT_CONTEXT_ENTRIES
}

pub const fn address_input_context_bytes(context_size: usize) -> usize {
    context_size * ADDRESS_DEVICE_INPUT_CONTEXT_ENTRIES
}

pub struct InputContextBuffer {
    base: usize,
    context_size: usize,
}

impl InputContextBuffer {
    pub const fn new(base: usize, context_size: usize) -> Self {
        Self { base, context_size }
    }

    pub fn control_mut(&self) -> *mut InputControlContext {
        self.base as *mut InputControlContext
    }

    pub fn slot_mut(&self) -> *mut SlotContext {
        (self.base + self.context_size) as *mut SlotContext
    }

    pub fn endpoint_mut(&self, dci: u8) -> Result<*mut EndpointContext, &'static str> {
        if !(1..=31).contains(&dci) {
            return Err("Invalid endpoint DCI");
        }

        Ok((self.base + self.context_size * (dci as usize + 1)) as *mut EndpointContext)
    }
}

pub struct DeviceContextBuffer {
    base: usize,
    context_size: usize,
}

impl DeviceContextBuffer {
    pub const fn new(base: usize, context_size: usize) -> Self {
        Self { base, context_size }
    }

    pub fn slot(&self) -> *const SlotContext {
        self.base as *const SlotContext
    }

    pub fn endpoint(&self, dci: u8) -> Result<*const EndpointContext, &'static str> {
        if !(1..=31).contains(&dci) {
            return Err("Invalid endpoint DCI");
        }

        Ok((self.base + self.context_size * dci as usize) as *const EndpointContext)
    }
}

/// Device context (slot + up to 31 endpoints, 2KB max).
#[repr(C, align(64))]
pub struct DeviceContext {
    pub slot: SlotContext,
    pub endpoints: [EndpointContext; 31],
}

impl DeviceContext {
    pub const fn size() -> usize {
        size_of::<Self>()
    }

    pub fn new() -> Self {
        Self {
            slot: SlotContext::default(),
            endpoints: [EndpointContext::default(); 31],
        }
    }
}

impl Default for DeviceContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_slot_context_size() {
        assert_eq!(SlotContext::size(), 32);
    }

    #[test_case]
    fn test_endpoint_context_size() {
        assert_eq!(EndpointContext::size(), 32);
    }

    #[test_case]
    fn test_input_control_context_size() {
        assert_eq!(InputControlContext::size(), 32);
    }

    #[test_case]
    fn test_input_context_size_multiple_of_64() {
        assert_eq!(InputContext::size() % 64, 0);
    }

    #[test_case]
    fn test_device_context_alignment() {
        assert_eq!(core::mem::align_of::<DeviceContext>(), 64);
    }

    #[test_case]
    fn test_input_context_address_configuration() {
        let mut ctx = InputContext::new();
        ctx.configure_for_address(1, 1, speed::FULL);

        assert_eq!(ctx.control.add_context_flags & 1, 1);
        assert_eq!(ctx.control.add_context_flags & 2, 2);
    }

    #[test_case]
    fn test_runtime_context_buffer_sizes() {
        assert_eq!(device_context_bytes(32), 1024);
        assert_eq!(device_context_bytes(64), 2048);
        assert_eq!(address_input_context_bytes(32), 96);
        assert_eq!(address_input_context_bytes(64), 192);
        assert_eq!(full_input_context_bytes(32), 1056);
        assert_eq!(full_input_context_bytes(64), 2112);
    }
}
