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
    EnableSlotCommand = 9,
    AddressDeviceCommand = 11,
    ConfigureEndpointCommand = 12,
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

    /// Builds a Normal transfer TRB.
    pub fn normal_transfer(data_buffer: u64, len: u32, interrupt_on_completion: bool) -> Self {
        let mut trb = Self::new(TrbType::Normal);
        trb.parameter = data_buffer;
        trb.status = len & 0x1ffff;
        if interrupt_on_completion {
            trb.control |= 1 << 5;
        }
        trb
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
}
