use scarlet::device::video::avd_fw;

use crate::h264::H264DecodeRequest;

const CMD_H264_DECODE: u32 = 0x10;
const CMD_TAG_MASK: u32 = 0x0000_ffff;
const CMD_KIND_SHIFT: u32 = 24;

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
    UnknownIrq,
    /// Message not yet classified by the Scarlet AVD ABI.
    Raw(u32),
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
            avd_fw::MSG_UNKNOWN_IRQ => Self::UnknownIrq,
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
            Self::UnknownIrq => avd_fw::MSG_UNKNOWN_IRQ,
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

/// AP to CM3 command word with driver-side tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvdFirmwareCommand {
    /// Raw command word written to the AP-to-CM3 mailbox.
    pub raw: u32,
    /// Driver-local tag used to match follow-up traces.
    pub tag: u32,
}

/// State for producing Apple AVD firmware commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvdFirmwareMailbox {
    next_tag: u32,
}

impl AvdFirmwareMailbox {
    /// Create a command encoder with tag zero as the next tag.
    ///
    /// # Returns
    ///
    /// Empty firmware mailbox command state.
    pub const fn new() -> Self {
        Self { next_tag: 0 }
    }

    /// Encode a H.264 decode command.
    ///
    /// # Arguments
    ///
    /// * `request` - H.264 decode request whose session and frame numbers are
    ///   folded into the command word.
    ///
    /// # Returns
    ///
    /// Firmware command word and tag.
    pub fn encode_h264_decode(&mut self, request: &H264DecodeRequest) -> AvdFirmwareCommand {
        let tag = self.next_tag & CMD_TAG_MASK;
        self.next_tag = self.next_tag.wrapping_add(1) & CMD_TAG_MASK;

        let stream_hint = ((request.session_id as u32) ^ request.frame_number) & 0xff;
        AvdFirmwareCommand {
            raw: (CMD_H264_DECODE << CMD_KIND_SHIFT) | (stream_hint << 16) | tag,
            tag,
        }
    }
}
