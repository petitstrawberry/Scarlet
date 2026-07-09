use scarlet::device::video::avd_fw;

/// Decoded Apple AVD firmware message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvdFirmwareMessage {
    /// CM3 firmware has reached its ready loop.
    Ready,
    /// CM3 firmware reported a panic.
    Panic,
    /// Video processor completed work.
    VideoProcessorDone,
    /// Video processor reported an error.
    VideoProcessorError,
    /// Post-processor completed work.
    PostProcessorDone,
    /// Firmware reported an interrupt that the kernel did not classify.
    UnknownIrq(u32),
    /// Message not yet classified by the Scarlet AVD ABI.
    Raw(u32),
}

/// Raw firmware mailbox word and its decoded class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvdFirmwareMessageWord {
    /// Raw CM3-to-AP mailbox value.
    pub raw: u32,
    /// Decoded message class.
    pub message: AvdFirmwareMessage,
}

impl AvdFirmwareMessage {
    /// Decode a raw firmware mailbox word.
    ///
    /// # Arguments
    ///
    /// * `raw` - Raw CM3 to AP mailbox value.
    ///
    /// # Returns
    ///
    /// Classified firmware message.
    pub fn decode(raw: u32) -> Self {
        match raw {
            avd_fw::MSG_READY => Self::Ready,
            avd_fw::MSG_PANIC => Self::Panic,
            value if value & 0xff00 == avd_fw::MSG_PP_DONE => Self::PostProcessorDone,
            value if value & 0xffff_ff00 == avd_fw::MSG_UNKNOWN_IRQ => {
                Self::UnknownIrq(value & 0xff)
            }
            value if value & 0xff00 == avd_fw::MSG_VP_DONE => Self::VideoProcessorDone,
            value if value & 0xff00 == avd_fw::MSG_VP_ERROR => Self::VideoProcessorError,
            value => Self::Raw(value),
        }
    }

    /// Return the raw message word.
    ///
    /// # Returns
    ///
    /// Raw firmware ABI value.
    pub fn raw(self) -> u32 {
        match self {
            Self::Ready => avd_fw::MSG_READY,
            Self::Panic => avd_fw::MSG_PANIC,
            Self::VideoProcessorDone => avd_fw::MSG_VP_DONE,
            Self::VideoProcessorError => avd_fw::MSG_VP_ERROR,
            Self::PostProcessorDone => avd_fw::MSG_PP_DONE,
            Self::UnknownIrq(irq) => avd_fw::MSG_UNKNOWN_IRQ | (irq & 0xff),
            Self::Raw(value) => value,
        }
    }

    /// Return whether the message represents a fatal firmware state.
    ///
    /// # Returns
    ///
    /// `true` for panic or processor error notifications.
    pub fn is_fault(self) -> bool {
        matches!(self, Self::Panic | Self::VideoProcessorError)
    }
}
