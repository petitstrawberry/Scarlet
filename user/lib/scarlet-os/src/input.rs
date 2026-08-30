//! Scarlet native input event device access.
//!
//! The wrapper is built directly on [`Handle`] and its stream capability so it
//! works in both `no_std` Scarlet programs and `std`-linked native programs.

use crate::handle::capability::{StreamError, StreamResult};
use crate::handle::{Handle, HandleError, HandleResult};

/// Query the input device's [`InputDeviceKind`].
pub const SCTL_INPUT_GET_KIND: u32 = 0x5353_0100;
/// Query the input device's capability bit mask.
pub const SCTL_INPUT_GET_CAPABILITIES: u32 = 0x5353_0101;
/// Query the minimum value of the absolute axis passed as the control argument.
pub const SCTL_INPUT_GET_ABS_MIN: u32 = 0x5353_0102;
/// Query the maximum value of the absolute axis passed as the control argument.
pub const SCTL_INPUT_GET_ABS_MAX: u32 = 0x5353_0103;
/// Query the number of concurrently reportable multitouch contacts.
pub const SCTL_INPUT_GET_MT_SLOT_COUNT: u32 = 0x5353_0104;
/// Query the bit mask of supported `SW_*` switch codes.
pub const SCTL_INPUT_GET_SWITCH_CAPABILITIES: u32 = 0x5353_0105;
/// Query the current state of the `SW_*` switch code passed as the control argument.
pub const SCTL_INPUT_GET_SWITCH_STATE: u32 = 0x5353_0106;

/// Device produces key or button events.
pub const INPUT_CAP_KEY: u32 = 1 << 0;
/// Device produces relative-axis events.
pub const INPUT_CAP_REL: u32 = 1 << 1;
/// Device produces absolute-axis events.
pub const INPUT_CAP_ABS: u32 = 1 << 2;
/// Device represents direct touch rather than an indirect pointer surface.
pub const INPUT_CAP_DIRECT_TOUCH: u32 = 1 << 3;
/// Device reports multitouch slots and contact axes.
pub const INPUT_CAP_MT: u32 = 1 << 4;
/// Device reports `EV_SW` switch state changes.
pub const INPUT_CAP_SWITCH: u32 = 1 << 5;
/// Device is physically integrated into the system rather than externally attached.
pub const INPUT_CAP_INTERNAL: u32 = 1 << 6;

/// Largest Linux-compatible absolute-axis code accepted by the metadata ABI.
pub const ABS_MAX: u16 = 0x3f;
/// Largest multitouch slot table accepted by the kernel metadata ABI.
pub const MAX_MT_SLOTS: u16 = 64;

/// Linux-compatible input event types used by raw event records.
pub mod event_types {
    /// Switch state changes (for example, lid and tablet-mode switches).
    pub const EV_SW: u16 = 0x05;
}

/// Linux-compatible synchronization codes used by raw event records.
pub mod syn_codes {
    /// Separates legacy multitouch contact reports.
    pub const SYN_MT_REPORT: u16 = 0x02;
    /// Events were dropped; consumers must discard cached input state.
    pub const SYN_DROPPED: u16 = 0x03;
}

/// Linux-compatible multitouch absolute-axis codes used by raw event records.
pub mod abs_codes {
    /// Multitouch contact slot selector.
    pub const ABS_MT_SLOT: u16 = 0x2f;
    /// Multitouch contact major-axis size.
    pub const ABS_MT_TOUCH_MAJOR: u16 = 0x30;
    /// Multitouch contact X position.
    pub const ABS_MT_POSITION_X: u16 = 0x35;
    /// Multitouch contact Y position.
    pub const ABS_MT_POSITION_Y: u16 = 0x36;
    /// Multitouch contact tracking identifier.
    pub const ABS_MT_TRACKING_ID: u16 = 0x39;
    /// Multitouch contact pressure.
    pub const ABS_MT_PRESSURE: u16 = 0x3a;
}

/// Linux-compatible posture and lid switch codes used by raw event records.
pub mod switch_codes {
    /// Lid is closed.
    pub const SW_LID: u16 = 0x00;
    /// Device is in tablet mode.
    pub const SW_TABLET_MODE: u16 = 0x01;
    /// Largest Linux-compatible switch code accepted by the control ABI.
    pub const SW_MAX: u16 = 0x10;
}

/// Logical input device class.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputDeviceKind {
    /// Device class is not declared.
    Unknown = 0,
    /// Keyboard device.
    Keyboard = 1,
    /// Relative mouse device.
    Mouse = 2,
    /// Indirect touchpad device.
    Touchpad = 3,
    /// Direct touchscreen device.
    Touchscreen = 4,
    /// Graphics tablet or stylus device.
    Tablet = 5,
    /// Posture, lid, or other switch device.
    Switch = 6,
}

impl TryFrom<i32> for InputDeviceKind {
    type Error = ();

    fn try_from(value: i32) -> core::result::Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Unknown),
            1 => Ok(Self::Keyboard),
            2 => Ok(Self::Mouse),
            3 => Ok(Self::Touchpad),
            4 => Ok(Self::Touchscreen),
            5 => Ok(Self::Tablet),
            6 => Ok(Self::Switch),
            _ => Err(()),
        }
    }
}

/// Inclusive raw range for one `EV_ABS` axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AbsoluteAxisInfo {
    /// Linux-compatible `ABS_*` axis code.
    pub code: u16,
    /// Inclusive logical minimum emitted by the device.
    pub minimum: i32,
    /// Inclusive logical maximum emitted by the device.
    pub maximum: i32,
}

/// Owning wrapper for a Scarlet native input event device.
#[derive(Debug)]
pub struct InputDevice {
    handle: Handle,
}

impl InputDevice {
    /// Open an input event device for reading.
    ///
    /// # Arguments
    ///
    /// * `path` - Device path such as `/dev/touchscreen0`.
    ///
    /// # Returns
    ///
    /// An input device wrapper, or a handle error if the path cannot be opened.
    pub fn open(path: &str) -> HandleResult<Self> {
        Self::from_handle(Handle::open(path, 0)?)
    }

    /// Wrap an owned stream-capable handle.
    ///
    /// # Arguments
    ///
    /// * `handle` - Handle whose ownership is transferred to this wrapper.
    ///
    /// # Returns
    ///
    /// An input device wrapper, or [`HandleError::Unsupported`] if the object
    /// is not stream-capable.
    pub fn from_handle(handle: Handle) -> HandleResult<Self> {
        handle.as_stream()?;
        Ok(Self { handle })
    }

    /// Read raw input event bytes.
    ///
    /// # Arguments
    ///
    /// * `buffer` - Destination buffer.
    ///
    /// # Returns
    ///
    /// Number of bytes read, or a stream error.
    pub fn read(&self, buffer: &mut [u8]) -> StreamResult<usize> {
        self.handle
            .as_stream()
            .map_err(|_| StreamError::Unsupported)?
            .read(buffer)
    }

    /// Change whether reads block when no event is available.
    ///
    /// # Arguments
    ///
    /// * `enabled` - Whether reads should return without blocking.
    ///
    /// # Returns
    ///
    /// Success or a handle error from the kernel.
    pub fn set_nonblocking(&self, enabled: bool) -> HandleResult<()> {
        self.handle.set_nonblocking(enabled)
    }

    /// Return the device's declared logical class.
    ///
    /// # Returns
    ///
    /// The class reported by the kernel, or a handle error if metadata is
    /// unavailable or malformed.
    pub fn kind(&self) -> HandleResult<InputDeviceKind> {
        let raw = self.handle.control(SCTL_INPUT_GET_KIND, 0)?;
        InputDeviceKind::try_from(raw).map_err(|_| HandleError::InvalidParameter)
    }

    /// Return the device's `INPUT_CAP_*` bit mask.
    ///
    /// # Returns
    ///
    /// The capability mask, or a handle error if metadata is unavailable or
    /// malformed.
    pub fn capabilities(&self) -> HandleResult<u32> {
        let raw = self.handle.control(SCTL_INPUT_GET_CAPABILITIES, 0)?;
        u32::try_from(raw).map_err(|_| HandleError::InvalidParameter)
    }

    /// Query the inclusive raw range of an absolute axis.
    ///
    /// # Arguments
    ///
    /// * `code` - Linux-compatible `ABS_*` code.
    ///
    /// # Returns
    ///
    /// The declared range. Unsupported, invalid, and malformed axes return a
    /// handle error.
    pub fn absolute_axis(&self, code: u16) -> HandleResult<AbsoluteAxisInfo> {
        if code > ABS_MAX {
            return Err(HandleError::InvalidParameter);
        }
        let minimum = self
            .handle
            .control(SCTL_INPUT_GET_ABS_MIN, usize::from(code))?;
        let maximum = self
            .handle
            .control(SCTL_INPUT_GET_ABS_MAX, usize::from(code))?;
        if minimum >= maximum {
            return Err(HandleError::InvalidParameter);
        }
        Ok(AbsoluteAxisInfo {
            code,
            minimum,
            maximum,
        })
    }

    /// Return the declared number of concurrently reportable multitouch contacts.
    ///
    /// # Returns
    ///
    /// The slot count, or a handle error if multitouch metadata is unavailable
    /// or malformed.
    pub fn multitouch_slot_count(&self) -> HandleResult<u16> {
        let raw = self.handle.control(SCTL_INPUT_GET_MT_SLOT_COUNT, 0)?;
        let slot_count = u16::try_from(raw).map_err(|_| HandleError::InvalidParameter)?;
        if slot_count == 0 {
            return Err(HandleError::InvalidParameter);
        }
        Ok(slot_count)
    }

    /// Return the bit mask of declared `SW_*` switch codes.
    ///
    /// # Returns
    ///
    /// The switch capability mask, or a handle error if metadata is unavailable
    /// or malformed.
    pub fn switch_capabilities(&self) -> HandleResult<u32> {
        let raw = self.handle.control(SCTL_INPUT_GET_SWITCH_CAPABILITIES, 0)?;
        u32::try_from(raw).map_err(|_| HandleError::InvalidParameter)
    }

    /// Return the current state of a declared `SW_*` switch code.
    ///
    /// # Arguments
    ///
    /// * `code` - Linux-compatible `SW_*` switch code.
    ///
    /// # Returns
    ///
    /// `true` when the switch is active, `false` when it is inactive, or a
    /// handle error when the switch is unavailable or the returned state is
    /// malformed.
    pub fn switch_state(&self, code: u16) -> HandleResult<bool> {
        if code > switch_codes::SW_MAX {
            return Err(HandleError::InvalidParameter);
        }
        match self
            .handle
            .control(SCTL_INPUT_GET_SWITCH_STATE, usize::from(code))?
        {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(HandleError::InvalidParameter),
        }
    }

    /// Borrow the underlying handle.
    ///
    /// # Returns
    ///
    /// The owned input event handle.
    pub const fn as_handle(&self) -> &Handle {
        &self.handle
    }

    /// Consume this wrapper and return its handle.
    ///
    /// # Returns
    ///
    /// The owned input event handle.
    pub fn into_handle(self) -> Handle {
        self.handle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_kind_values_are_stable_and_validated() {
        assert_eq!(
            InputDeviceKind::try_from(4),
            Ok(InputDeviceKind::Touchscreen)
        );
        assert!(InputDeviceKind::try_from(-1).is_err());
        assert!(InputDeviceKind::try_from(7).is_err());
    }

    #[test]
    fn multitouch_and_switch_abi_values_are_stable() {
        assert_eq!(SCTL_INPUT_GET_MT_SLOT_COUNT, 0x5353_0104);
        assert_eq!(SCTL_INPUT_GET_SWITCH_CAPABILITIES, 0x5353_0105);
        assert_eq!(SCTL_INPUT_GET_SWITCH_STATE, 0x5353_0106);
        assert_eq!(INPUT_CAP_MT, 1 << 4);
        assert_eq!(INPUT_CAP_SWITCH, 1 << 5);
        assert_eq!(INPUT_CAP_INTERNAL, 1 << 6);
        assert_eq!(event_types::EV_SW, 0x05);
        assert_eq!(syn_codes::SYN_MT_REPORT, 0x02);
        assert_eq!(syn_codes::SYN_DROPPED, 0x03);
        assert_eq!(abs_codes::ABS_MT_SLOT, 0x2f);
        assert_eq!(abs_codes::ABS_MT_TOUCH_MAJOR, 0x30);
        assert_eq!(abs_codes::ABS_MT_POSITION_X, 0x35);
        assert_eq!(abs_codes::ABS_MT_POSITION_Y, 0x36);
        assert_eq!(abs_codes::ABS_MT_TRACKING_ID, 0x39);
        assert_eq!(abs_codes::ABS_MT_PRESSURE, 0x3a);
        assert_eq!(switch_codes::SW_LID, 0x00);
        assert_eq!(switch_codes::SW_TABLET_MODE, 0x01);
        assert_eq!(InputDeviceKind::try_from(6), Ok(InputDeviceKind::Switch));
    }
}
