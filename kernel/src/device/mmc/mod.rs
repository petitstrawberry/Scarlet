//! Host-independent MMC protocol types.
//!
//! This module is the boundary between the MMC/eMMC card state machine and a
//! concrete host controller. Generic card logic only issues [`MmcCommand`]s
//! through [`MmcHost`]; PCI SDHCI and future Qualcomm SDHCI implementations
//! provide the controller-specific transport.

/// Result type used by MMC host-controller operations.
pub type MmcResult<T> = Result<T, MmcError>;

/// Errors returned by an MMC host or card operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmcError {
    /// No card is currently present in the slot.
    NoMedia,
    /// A controller or card operation timed out.
    Timeout,
    /// The controller rejected or failed a command.
    Command,
    /// A data transfer failed.
    Data,
    /// The controller reported an invalid response.
    Response,
    /// The requested operation is not supported.
    Unsupported,
    /// A caller supplied an invalid argument or buffer.
    InvalidArgument,
    /// The requested block range is outside the media.
    OutOfRange,
    /// The card changed while an operation was in progress.
    MediaChanged,
}

impl MmcError {
    /// Return a stable diagnostic string for this error.
    ///
    /// # Returns
    ///
    /// A static human-readable error description.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoMedia => "No MMC media present",
            Self::Timeout => "MMC operation timed out",
            Self::Command => "MMC command failed",
            Self::Data => "MMC data transfer failed",
            Self::Response => "Invalid MMC response",
            Self::Unsupported => "Unsupported MMC operation",
            Self::InvalidArgument => "Invalid MMC argument",
            Self::OutOfRange => "MMC request out of range",
            Self::MediaChanged => "MMC media changed",
        }
    }
}

/// Response encoding expected for an MMC command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmcResponseType {
    /// Command has no response.
    None,
    /// 48-bit response with CRC and command-index validation.
    R1,
    /// R1 response followed by a card-busy interval.
    R1b,
    /// 136-bit CID or CSD response.
    R2,
    /// 48-bit OCR response without CRC or command-index validation.
    R3,
}

/// One command submitted to an MMC host controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MmcCommand {
    index: u8,
    argument: u32,
    response: MmcResponseType,
}

impl MmcCommand {
    /// Construct an MMC command.
    ///
    /// # Arguments
    ///
    /// * `index` - Six-bit MMC command index.
    /// * `argument` - Command argument in wire byte order.
    /// * `response` - Expected response encoding.
    ///
    /// # Returns
    ///
    /// A command descriptor suitable for [`MmcHost::send_command`].
    pub const fn new(index: u8, argument: u32, response: MmcResponseType) -> Self {
        Self {
            index,
            argument,
            response,
        }
    }

    /// Get the command index.
    ///
    /// # Returns
    ///
    /// Six-bit MMC command index.
    pub const fn index(self) -> u8 {
        self.index
    }

    /// Get the command argument.
    ///
    /// # Returns
    ///
    /// Raw 32-bit command argument.
    pub const fn argument(self) -> u32 {
        self.argument
    }

    /// Get the expected response type.
    ///
    /// # Returns
    ///
    /// Response encoding selected for this command.
    pub const fn response(self) -> MmcResponseType {
        self.response
    }
}

/// Response words returned by a host controller.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MmcResponse {
    words: [u32; 4],
}

impl MmcResponse {
    /// Construct a response from host-controller response registers.
    ///
    /// # Arguments
    ///
    /// * `words` - Response words ordered from response register 0 through 3.
    ///
    /// # Returns
    ///
    /// A response value.
    pub const fn new(words: [u32; 4]) -> Self {
        Self { words }
    }

    /// Read one response word.
    ///
    /// # Arguments
    ///
    /// * `index` - Response register index from 0 through 3.
    ///
    /// # Returns
    ///
    /// The selected word, or zero for an out-of-range index.
    pub const fn word(self, index: usize) -> u32 {
        if index < self.words.len() {
            self.words[index]
        } else {
            0
        }
    }

    /// Return every response word.
    ///
    /// # Returns
    ///
    /// Response registers ordered from 0 through 3.
    pub const fn words(self) -> [u32; 4] {
        self.words
    }
}

/// Optional data phase associated with an MMC command.
pub enum MmcData<'a> {
    /// Read card data into the supplied buffer.
    Read(&'a mut [u8]),
    /// Write the supplied buffer to the card.
    Write(&'a [u8]),
}

impl MmcData<'_> {
    /// Return the transfer length in bytes.
    ///
    /// # Returns
    ///
    /// Length of the read or write buffer.
    pub fn len(&self) -> usize {
        match self {
            Self::Read(buffer) => buffer.len(),
            Self::Write(buffer) => buffer.len(),
        }
    }

    /// Return whether the transfer has no payload.
    ///
    /// # Returns
    ///
    /// `true` when the transfer buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Return whether data flows from the card to memory.
    ///
    /// # Returns
    ///
    /// `true` for a read data phase.
    pub const fn is_read(&self) -> bool {
        matches!(self, Self::Read(_))
    }
}

/// Width of the MMC data bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmcBusWidth {
    /// One data line.
    One,
    /// Four data lines.
    Four,
    /// Eight data lines.
    Eight,
}

/// Controller interface consumed by host-independent MMC card logic.
pub trait MmcHost: Send {
    /// Reset and power up the host controller for card identification.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the controller is ready for commands.
    fn reset(&mut self) -> MmcResult<()>;

    /// Program the card bus clock.
    ///
    /// # Arguments
    ///
    /// * `frequency_hz` - Requested clock frequency in hertz.
    ///
    /// # Returns
    ///
    /// `Ok(())` after the clock is stable and enabled.
    fn set_clock(&mut self, frequency_hz: u32) -> MmcResult<()>;

    /// Program the card data-bus width.
    ///
    /// # Arguments
    ///
    /// * `width` - Desired host-side bus width.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the controller accepted the width.
    fn set_bus_width(&mut self, width: MmcBusWidth) -> MmcResult<()>;

    /// Report whether media is currently present.
    ///
    /// # Returns
    ///
    /// `true` when commands may be sent to a card.
    fn card_present(&self) -> bool;

    /// Report whether media can be physically removed from this slot.
    ///
    /// # Returns
    ///
    /// `true` for a removable card slot and `false` for soldered eMMC.
    fn is_removable(&self) -> bool;

    /// Submit one MMC command and its optional data phase.
    ///
    /// # Arguments
    ///
    /// * `command` - Command index, argument, and response type.
    /// * `data` - Optional read or write data phase.
    ///
    /// # Returns
    ///
    /// The decoded host response, or an MMC transport error.
    fn send_command(
        &mut self,
        command: MmcCommand,
        data: Option<MmcData<'_>>,
    ) -> MmcResult<MmcResponse>;
}
