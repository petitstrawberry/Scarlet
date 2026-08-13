use core::mem::size_of;

/// xHCI TRB type values used by the initial implementation.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrbType {
    Normal = 1,
    SetupStage = 2,
    DataStage = 3,
    StatusStage = 4,
    Link = 6,
    NoOp = 8,
    EnableSlotCommand = 9,
    DisableSlotCommand = 10,
    AddressDeviceCommand = 11,
    ConfigureEndpointCommand = 12,
    ResetEndpointCommand = 14,
    StopEndpointCommand = 15,
    SetTrDequeuePointerCommand = 16,
    CommandCompletionEvent = 33,
    PortStatusChangeEvent = 34,
    TransferEvent = 32,
}

/// Raw 16-byte xHCI transfer request block.
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Trb {
    pub parameter: u64,
    pub status: u32,
    pub control: u32,
}

impl Trb {
    /// Creates a zeroed TRB with the requested type encoded.
    pub const fn new(trb_type: TrbType) -> Self {
        Self {
            parameter: 0,
            status: 0,
            control: (trb_type as u32) << 10,
        }
    }

    /// Returns the encoded TRB type field.
    pub const fn trb_type(&self) -> u8 {
        ((self.control >> 10) & 0x3f) as u8
    }

    /// Updates the TRB cycle bit.
    pub fn set_cycle(&mut self, cycle: bool) {
        if cycle {
            self.control |= 1;
        } else {
            self.control &= !1;
        }
    }

    pub fn set_chain(&mut self, chain: bool) {
        if chain {
            self.control |= 1 << 4;
        } else {
            self.control &= !(1 << 4);
        }
    }

    /// Sets the slot ID field used by command TRBs and completion events.
    pub fn set_slot_id(&mut self, slot_id: u8) {
        self.control = (self.control & !(0xff << 24)) | ((slot_id as u32) << 24);
    }

    /// Returns the slot ID encoded in the TRB.
    pub const fn slot_id(&self) -> u8 {
        ((self.control >> 24) & 0xff) as u8
    }

    /// Returns the completion code for event TRBs.
    pub const fn completion_code(&self) -> u8 {
        ((self.status >> 24) & 0xff) as u8
    }

    /// Returns the endpoint ID encoded in transfer and event TRBs.
    pub const fn endpoint_id(&self) -> u8 {
        ((self.control >> 16) & 0x1f) as u8
    }

    /// Returns the transfer length portion of the status field.
    pub const fn transfer_length(&self) -> u32 {
        self.status & 0x00ff_ffff
    }

    /// Returns the raw TRB pointer payload used by event TRBs.
    pub const fn trb_pointer(&self) -> u64 {
        self.parameter
    }

    /// Builds an Enable Slot command TRB.
    pub const fn enable_slot_command() -> Self {
        Self::new(TrbType::EnableSlotCommand)
    }

    /// Builds a Disable Slot command TRB.
    ///
    /// # Arguments
    ///
    /// * `slot_id` - xHCI slot ID to release.
    ///
    /// # Returns
    ///
    /// Encoded Disable Slot command TRB.
    pub fn disable_slot_command(slot_id: u8) -> Self {
        let mut trb = Self::new(TrbType::DisableSlotCommand);
        trb.set_slot_id(slot_id);
        trb
    }

    /// Builds an Address Device command TRB.
    pub fn address_device_command(input_context_ptr: u64, slot_id: u8, bsr: bool) -> Self {
        let mut trb = Self::new(TrbType::AddressDeviceCommand);
        trb.parameter = input_context_ptr;
        if bsr {
            trb.control |= 1 << 9;
        }
        trb.set_slot_id(slot_id);
        trb
    }

    /// Builds a Configure Endpoint command TRB.
    pub fn configure_endpoint_command(
        input_context_ptr: u64,
        slot_id: u8,
        deconfigure: bool,
    ) -> Self {
        let mut trb = Self::new(TrbType::ConfigureEndpointCommand);
        trb.parameter = input_context_ptr;
        if deconfigure {
            trb.control |= 1 << 9;
        }
        trb.set_slot_id(slot_id);
        trb
    }

    /// Builds a Reset Endpoint command TRB.
    ///
    /// # Arguments
    ///
    /// * `slot_id` - xHCI slot ID containing the endpoint.
    /// * `endpoint_id` - xHCI endpoint context index.
    /// * `preserve_streams` - True to preserve stream state.
    ///
    /// # Returns
    ///
    /// Encoded Reset Endpoint command TRB.
    pub fn reset_endpoint_command(slot_id: u8, endpoint_id: u8, preserve_streams: bool) -> Self {
        let mut trb = Self::new(TrbType::ResetEndpointCommand);
        trb.control |= (endpoint_id as u32) << 16;
        if preserve_streams {
            trb.control |= 1 << 9;
        }
        trb.set_slot_id(slot_id);
        trb
    }

    /// Builds a Stop Endpoint command TRB.
    ///
    /// # Arguments
    ///
    /// * `slot_id` - xHCI slot ID containing the endpoint.
    /// * `endpoint_id` - xHCI endpoint context index.
    /// * `suspend` - True to suspend instead of stopping the endpoint.
    ///
    /// # Returns
    ///
    /// Encoded Stop Endpoint command TRB.
    pub fn stop_endpoint_command(slot_id: u8, endpoint_id: u8, suspend: bool) -> Self {
        let mut trb = Self::new(TrbType::StopEndpointCommand);
        trb.control |= (endpoint_id as u32) << 16;
        if suspend {
            trb.control |= 1 << 23;
        }
        trb.set_slot_id(slot_id);
        trb
    }

    /// Builds a Set TR Dequeue Pointer command TRB.
    ///
    /// # Arguments
    ///
    /// * `dequeue_pointer` - Transfer ring dequeue pointer including DCS in bit 0.
    /// * `slot_id` - xHCI slot ID containing the endpoint.
    /// * `endpoint_id` - xHCI endpoint context index.
    ///
    /// # Returns
    ///
    /// Encoded Set TR Dequeue Pointer command TRB.
    pub fn set_tr_dequeue_pointer_command(
        dequeue_pointer: u64,
        slot_id: u8,
        endpoint_id: u8,
    ) -> Self {
        let mut trb = Self::new(TrbType::SetTrDequeuePointerCommand);
        trb.parameter = dequeue_pointer;
        trb.control |= (endpoint_id as u32) << 16;
        trb.set_slot_id(slot_id);
        trb
    }

    /// Builds a Normal transfer TRB with IOC set.
    pub fn normal_transfer(data_buffer: u64, len: u32) -> Self {
        let mut trb = Self::new(TrbType::Normal);
        trb.parameter = data_buffer;
        trb.status = len & 0x1ffff;
        trb.control |= 1 << 5;
        trb
    }

    /// Builds an IN Normal transfer TRB with IOC and ISP set.
    pub fn normal_transfer_in(data_buffer: u64, len: u32) -> Self {
        let mut trb = Self::normal_transfer(data_buffer, len);
        trb.control |= 1 << 2;
        trb
    }

    /// Builds a transfer-ring No-Op TRB without interrupt-on-completion.
    pub const fn no_op_transfer() -> Self {
        Self::new(TrbType::NoOp)
    }

    /// Builds a Setup Stage transfer TRB.
    pub fn setup_stage(
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        length: u16,
        transfer_type: u8,
    ) -> Self {
        let mut trb = Self::new(TrbType::SetupStage);
        trb.parameter = (request_type as u64)
            | ((request as u64) << 8)
            | ((value as u64) << 16)
            | ((index as u64) << 32)
            | ((length as u64) << 48);
        trb.status = 8;
        trb.control |= 1 << 6;
        trb.control |= ((transfer_type as u32) & 0x3) << 16;
        trb
    }

    /// Builds a Data Stage transfer TRB.
    pub fn data_stage(data_buffer: u64, len: u32, direction_in: bool) -> Self {
        let mut trb = Self::new(TrbType::DataStage);
        trb.parameter = data_buffer;
        trb.status = len & 0x1ffff;
        if direction_in {
            trb.control |= 1 << 2;
            trb.control |= 1 << 16;
        }
        trb
    }

    /// Builds a Status Stage transfer TRB.
    pub fn status_stage(direction_in: bool, interrupt_on_completion: bool) -> Self {
        let mut trb = Self::new(TrbType::StatusStage);
        if direction_in {
            trb.control |= 1 << 16;
        }
        if interrupt_on_completion {
            trb.control |= 1 << 5;
        }
        trb
    }

    /// Returns the encoded TRB size in bytes.
    pub const fn encoded_size() -> usize {
        size_of::<Self>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_trb_encoding() {
        let mut trb = Trb::new(TrbType::EnableSlotCommand);
        assert_eq!(Trb::encoded_size(), 16);
        assert_eq!(trb.trb_type(), TrbType::EnableSlotCommand as u8);

        trb.set_cycle(true);
        assert_eq!(trb.control & 1, 1);
    }

    #[test_case]
    fn test_slot_id_encoding() {
        let mut trb = Trb::enable_slot_command();
        trb.set_slot_id(7);
        assert_eq!(trb.slot_id(), 7);
    }

    #[test_case]
    fn test_data_stage_in_sets_isp_and_direction_without_chain() {
        let trb = Trb::data_stage(0x1000, 9, true);

        assert_eq!(trb.trb_type(), TrbType::DataStage as u8);
        assert_ne!(trb.control & (1 << 2), 0);
        assert_eq!(trb.control & (1 << 4), 0);
        assert_ne!(trb.control & (1 << 16), 0);
    }

    #[test_case]
    fn test_no_op_transfer_has_no_ioc() {
        let trb = Trb::no_op_transfer();

        assert_eq!(trb.trb_type(), TrbType::NoOp as u8);
        assert_eq!(trb.control & (1 << 5), 0);
    }

    #[test_case]
    fn test_normal_transfer_in_sets_isp() {
        let trb = Trb::normal_transfer_in(0x1000, 36);

        assert_eq!(trb.trb_type(), TrbType::Normal as u8);
        assert_ne!(trb.control & (1 << 2), 0);
        assert_ne!(trb.control & (1 << 5), 0);
    }

    #[test_case]
    fn test_endpoint_recovery_command_encoding() {
        let reset = Trb::reset_endpoint_command(2, 5, false);
        assert_eq!(reset.trb_type(), TrbType::ResetEndpointCommand as u8);
        assert_eq!(reset.slot_id(), 2);
        assert_eq!(reset.endpoint_id(), 5);

        let stop = Trb::stop_endpoint_command(3, 4, false);
        assert_eq!(stop.trb_type(), TrbType::StopEndpointCommand as u8);
        assert_eq!(stop.slot_id(), 3);
        assert_eq!(stop.endpoint_id(), 4);

        let set_deq = Trb::set_tr_dequeue_pointer_command(0x1001, 4, 6);
        assert_eq!(
            set_deq.trb_type(),
            TrbType::SetTrDequeuePointerCommand as u8
        );
        assert_eq!(set_deq.parameter, 0x1001);
        assert_eq!(set_deq.slot_id(), 4);
        assert_eq!(set_deq.endpoint_id(), 6);
    }
}
