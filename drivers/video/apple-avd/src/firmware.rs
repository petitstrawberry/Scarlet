use scarlet::device::video::avd_fw;

/// Decoded Apple AVD firmware message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvdFirmwareMessage {
    /// Video processor completed work.
    VideoProcessorDone,
    /// Video processor reported an error for the enclosed pipe.
    VideoProcessorError(u32),
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
        if raw & avd_fw::MSG_UNKNOWN_IRQ != 0 {
            Self::UnknownIrq(raw & !avd_fw::MSG_UNKNOWN_IRQ)
        } else if raw & avd_fw::MSG_PP_DONE != 0 {
            Self::PostProcessorDone
        } else if raw & avd_fw::MSG_VP_ERROR != 0 {
            Self::VideoProcessorError(raw & !avd_fw::MSG_VP_ERROR)
        } else if raw & avd_fw::MSG_VP_DONE != 0 {
            Self::VideoProcessorDone
        } else {
            Self::VideoProcessorError(raw)
        }
    }

    /// Return the raw message word.
    ///
    /// # Returns
    ///
    /// Raw firmware ABI value.
    pub fn raw(self) -> u32 {
        match self {
            Self::VideoProcessorDone => avd_fw::MSG_VP_DONE,
            Self::VideoProcessorError(pipe) => avd_fw::MSG_VP_ERROR | pipe,
            Self::PostProcessorDone => avd_fw::MSG_PP_DONE,
            Self::UnknownIrq(irq) => avd_fw::MSG_UNKNOWN_IRQ | irq,
            Self::Raw(value) => value,
        }
    }

    /// Return whether the message represents a fatal firmware state.
    ///
    /// # Returns
    ///
    /// `true` for panic or processor error notifications.
    pub fn is_fault(self) -> bool {
        matches!(self, Self::VideoProcessorError(_) | Self::UnknownIrq(_))
    }
}
