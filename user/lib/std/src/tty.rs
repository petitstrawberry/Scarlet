//! Terminal helpers for Scarlet native user space.
//!
//! This module wraps Scarlet's native TTY control operations in typed methods.
//! It intentionally exposes Scarlet-native types instead of POSIX function
//! names; Linux-compatible termios/ioctl handling stays in the ABI layer.

use crate::{
    fs::File,
    handle::{Handle, HandleError},
    io::{Error, ErrorKind, Result},
};

pub(crate) const SCTL_TTY_SET_ECHO: u32 = 0x5354_0001;
pub(crate) const SCTL_TTY_GET_ECHO: u32 = 0x5354_0002;
pub(crate) const SCTL_TTY_SET_CANONICAL: u32 = 0x5354_0003;
pub(crate) const SCTL_TTY_GET_CANONICAL: u32 = 0x5354_0004;
pub(crate) const SCTL_TTY_SET_WINSIZE: u32 = 0x5354_0005;
pub(crate) const SCTL_TTY_GET_WINSIZE: u32 = 0x5354_0006;
pub(crate) const SCTL_TTY_SET_READ_POLICY: u32 = 0x5354_0007;
pub(crate) const SCTL_TTY_GET_READ_POLICY: u32 = 0x5354_0008;
pub(crate) const SCTL_TTY_FLUSH_INPUT: u32 = 0x5354_0009;
pub(crate) const SCTL_TTY_SET_DEBUG: u32 = 0x5354_000A;
pub(crate) const SCTL_TTY_GET_DEBUG: u32 = 0x5354_000B;
pub(crate) const SCTL_TTY_SET_KBMODE: u32 = 0x5354_000C;
pub(crate) const SCTL_TTY_GET_KBMODE: u32 = 0x5354_000D;
pub(crate) const SCTL_TTY_SET_FOREGROUND_GROUP: u32 = 0x5354_000E;
pub(crate) const SCTL_TTY_GET_FOREGROUND_GROUP: u32 = 0x5354_000F;
pub(crate) const SCTL_TTY_SET_SIGNAL_CHARS: u32 = 0x5354_0010;
pub(crate) const SCTL_TTY_GET_SIGNAL_CHARS: u32 = 0x5354_0011;
pub(crate) const SCTL_TTY_SET_CRNL_INPUT: u32 = 0x5354_0012;
pub(crate) const SCTL_TTY_GET_CRNL_INPUT: u32 = 0x5354_0013;
pub(crate) const SCTL_TTY_SET_OUTPUT_POSTPROCESS: u32 = 0x5354_0014;
pub(crate) const SCTL_TTY_GET_OUTPUT_POSTPROCESS: u32 = 0x5354_0015;
pub(crate) const SCTL_TTY_SET_EXTENDED_INPUT: u32 = 0x5354_0016;
pub(crate) const SCTL_TTY_GET_EXTENDED_INPUT: u32 = 0x5354_0017;

/// Terminal window size in character cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowSize {
    /// Number of text columns.
    pub columns: u16,
    /// Number of text rows.
    pub rows: u16,
}

impl WindowSize {
    /// Create a terminal window size.
    ///
    /// # Arguments
    ///
    /// * `columns` - Number of text columns.
    /// * `rows` - Number of text rows.
    ///
    /// # Returns
    ///
    /// A new [`WindowSize`].
    pub const fn new(columns: u16, rows: u16) -> Self {
        Self { columns, rows }
    }

    fn pack(self) -> usize {
        ((self.columns as usize) << 16) | self.rows as usize
    }

    fn unpack(value: i32) -> Self {
        let packed = value as u32;
        Self {
            columns: (packed >> 16) as u16,
            rows: (packed & 0xFFFF) as u16,
        }
    }
}

/// TTY read wakeup policy for non-canonical input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadPolicy {
    /// Minimum queued bytes required before a read wakes.
    pub min_ready_bytes: u16,
    /// Read timeout in milliseconds.
    pub timeout_ms: u16,
}

impl ReadPolicy {
    /// Create a read policy.
    ///
    /// # Arguments
    ///
    /// * `min_ready_bytes` - Minimum queued bytes required before a read wakes.
    /// * `timeout_ms` - Read timeout in milliseconds.
    ///
    /// # Returns
    ///
    /// A new [`ReadPolicy`].
    pub const fn new(min_ready_bytes: u16, timeout_ms: u16) -> Self {
        Self {
            min_ready_bytes,
            timeout_ms,
        }
    }

    fn pack(self) -> usize {
        ((self.timeout_ms as usize) << 16) | self.min_ready_bytes as usize
    }

    fn unpack(value: i32) -> Self {
        let packed = value as u32;
        Self {
            min_ready_bytes: (packed & 0xFFFF) as u16,
            timeout_ms: ((packed >> 16) & 0xFFFF) as u16,
        }
    }
}

/// Scarlet keyboard translation mode for a terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardMode {
    /// Translate keyboard input to normal text/control sequences.
    Xlate,
    /// Medium raw keyboard mode.
    MediumRaw,
    /// Raw keyboard mode.
    Raw,
}

impl KeyboardMode {
    fn from_control(value: i32) -> Self {
        match value {
            0 => Self::Xlate,
            1 => Self::MediumRaw,
            _ => Self::Raw,
        }
    }

    fn as_control(self) -> usize {
        match self {
            Self::Xlate => 0,
            Self::MediumRaw => 1,
            Self::Raw => 2,
        }
    }
}

/// Snapshot of Scarlet-native terminal settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSettings {
    /// Whether written input is echoed back.
    pub echo: bool,
    /// Whether canonical line mode is enabled.
    pub canonical: bool,
    /// Whether terminal control characters generate process-control events.
    pub signal_chars: bool,
    /// Whether carriage return input is translated to newline.
    pub crnl_input: bool,
    /// Whether output post-processing is enabled.
    pub output_postprocess: bool,
    /// Whether extended input processing is enabled.
    pub extended_input: bool,
    /// Keyboard translation mode.
    pub keyboard_mode: KeyboardMode,
    /// Read wakeup policy.
    pub read_policy: ReadPolicy,
    /// Whether kernel-side TTY debug logging is enabled.
    pub debug: bool,
}

/// Borrowed terminal control view.
pub struct Terminal<'a> {
    handle: &'a Handle,
}

impl<'a> Terminal<'a> {
    /// Create a terminal control view from a [`File`].
    ///
    /// # Arguments
    ///
    /// * `file` - Open terminal-like file.
    ///
    /// # Returns
    ///
    /// A borrowed terminal control view.
    pub fn from_file(file: &'a File) -> Self {
        Self {
            handle: file.as_handle(),
        }
    }

    /// Create a terminal control view from a [`Handle`].
    ///
    /// # Arguments
    ///
    /// * `handle` - Open handle that supports TTY control operations.
    ///
    /// # Returns
    ///
    /// A borrowed terminal control view.
    pub fn from_handle(handle: &'a Handle) -> Self {
        Self { handle }
    }

    /// Return the terminal window size.
    ///
    /// # Returns
    ///
    /// Current terminal window size.
    pub fn winsize(&self) -> Result<WindowSize> {
        self.control(SCTL_TTY_GET_WINSIZE, 0)
            .map(WindowSize::unpack)
    }

    /// Set the terminal window size.
    ///
    /// # Arguments
    ///
    /// * `size` - New terminal window size.
    pub fn set_winsize(&self, size: WindowSize) -> Result<()> {
        self.control(SCTL_TTY_SET_WINSIZE, size.pack()).map(|_| ())
    }

    /// Return all native terminal settings.
    ///
    /// # Returns
    ///
    /// Current Scarlet-native terminal settings.
    pub fn settings(&self) -> Result<TerminalSettings> {
        Ok(TerminalSettings {
            echo: self.echo()?,
            canonical: self.canonical()?,
            signal_chars: self.signal_chars_enabled()?,
            crnl_input: self.crnl_input_enabled()?,
            output_postprocess: self.output_postprocess_enabled()?,
            extended_input: self.extended_input_enabled()?,
            keyboard_mode: self.keyboard_mode()?,
            read_policy: self.read_policy()?,
            debug: self.debug_enabled()?,
        })
    }

    /// Apply native terminal settings.
    ///
    /// # Arguments
    ///
    /// * `settings` - Settings to apply.
    pub fn apply_settings(&self, settings: TerminalSettings) -> Result<()> {
        self.set_echo(settings.echo)?;
        self.set_canonical(settings.canonical)?;
        self.set_signal_chars_enabled(settings.signal_chars)?;
        self.set_crnl_input_enabled(settings.crnl_input)?;
        self.set_output_postprocess_enabled(settings.output_postprocess)?;
        self.set_extended_input_enabled(settings.extended_input)?;
        self.set_keyboard_mode(settings.keyboard_mode)?;
        self.set_read_policy(settings.read_policy)?;
        self.set_debug_enabled(settings.debug)
    }

    /// Return whether echo is enabled.
    ///
    /// # Returns
    ///
    /// `true` when echo is enabled.
    pub fn echo(&self) -> Result<bool> {
        self.get_bool(SCTL_TTY_GET_ECHO)
    }

    /// Enable or disable echo.
    ///
    /// # Arguments
    ///
    /// * `enabled` - New echo state.
    pub fn set_echo(&self, enabled: bool) -> Result<()> {
        self.set_bool(SCTL_TTY_SET_ECHO, enabled)
    }

    /// Return whether canonical line mode is enabled.
    ///
    /// # Returns
    ///
    /// `true` when canonical line mode is enabled.
    pub fn canonical(&self) -> Result<bool> {
        self.get_bool(SCTL_TTY_GET_CANONICAL)
    }

    /// Enable or disable canonical line mode.
    ///
    /// # Arguments
    ///
    /// * `enabled` - New canonical state.
    pub fn set_canonical(&self, enabled: bool) -> Result<()> {
        self.set_bool(SCTL_TTY_SET_CANONICAL, enabled)
    }

    /// Return whether control characters generate process-control events.
    ///
    /// # Returns
    ///
    /// `true` when signal-character processing is enabled.
    pub fn signal_chars_enabled(&self) -> Result<bool> {
        self.get_bool(SCTL_TTY_GET_SIGNAL_CHARS)
    }

    /// Enable or disable terminal control-character processing.
    ///
    /// # Arguments
    ///
    /// * `enabled` - New signal-character processing state.
    pub fn set_signal_chars_enabled(&self, enabled: bool) -> Result<()> {
        self.set_bool(SCTL_TTY_SET_SIGNAL_CHARS, enabled)
    }

    /// Return whether carriage return input is translated to newline.
    ///
    /// # Returns
    ///
    /// `true` when CR-to-NL input translation is enabled.
    pub fn crnl_input_enabled(&self) -> Result<bool> {
        self.get_bool(SCTL_TTY_GET_CRNL_INPUT)
    }

    /// Enable or disable carriage return input translation.
    ///
    /// # Arguments
    ///
    /// * `enabled` - New CR-to-NL input translation state.
    pub fn set_crnl_input_enabled(&self, enabled: bool) -> Result<()> {
        self.set_bool(SCTL_TTY_SET_CRNL_INPUT, enabled)
    }

    /// Return whether output post-processing is enabled.
    ///
    /// # Returns
    ///
    /// `true` when output post-processing is enabled.
    pub fn output_postprocess_enabled(&self) -> Result<bool> {
        self.get_bool(SCTL_TTY_GET_OUTPUT_POSTPROCESS)
    }

    /// Enable or disable output post-processing.
    ///
    /// # Arguments
    ///
    /// * `enabled` - New output post-processing state.
    pub fn set_output_postprocess_enabled(&self, enabled: bool) -> Result<()> {
        self.set_bool(SCTL_TTY_SET_OUTPUT_POSTPROCESS, enabled)
    }

    /// Return whether extended input processing is enabled.
    ///
    /// # Returns
    ///
    /// `true` when extended input processing is enabled.
    pub fn extended_input_enabled(&self) -> Result<bool> {
        self.get_bool(SCTL_TTY_GET_EXTENDED_INPUT)
    }

    /// Enable or disable extended input processing.
    ///
    /// # Arguments
    ///
    /// * `enabled` - New extended input processing state.
    pub fn set_extended_input_enabled(&self, enabled: bool) -> Result<()> {
        self.set_bool(SCTL_TTY_SET_EXTENDED_INPUT, enabled)
    }

    /// Return the terminal read policy.
    ///
    /// # Returns
    ///
    /// Current read wakeup policy.
    pub fn read_policy(&self) -> Result<ReadPolicy> {
        self.control(SCTL_TTY_GET_READ_POLICY, 0)
            .map(ReadPolicy::unpack)
    }

    /// Set the terminal read policy.
    ///
    /// # Arguments
    ///
    /// * `policy` - New read wakeup policy.
    pub fn set_read_policy(&self, policy: ReadPolicy) -> Result<()> {
        self.control(SCTL_TTY_SET_READ_POLICY, policy.pack())
            .map(|_| ())
    }

    /// Return the keyboard translation mode.
    ///
    /// # Returns
    ///
    /// Current keyboard mode.
    pub fn keyboard_mode(&self) -> Result<KeyboardMode> {
        self.control(SCTL_TTY_GET_KBMODE, 0)
            .map(KeyboardMode::from_control)
    }

    /// Set the keyboard translation mode.
    ///
    /// # Arguments
    ///
    /// * `mode` - New keyboard translation mode.
    pub fn set_keyboard_mode(&self, mode: KeyboardMode) -> Result<()> {
        self.control(SCTL_TTY_SET_KBMODE, mode.as_control())
            .map(|_| ())
    }

    /// Return whether TTY debug logging is enabled.
    ///
    /// # Returns
    ///
    /// `true` when debug logging is enabled.
    pub fn debug_enabled(&self) -> Result<bool> {
        self.get_bool(SCTL_TTY_GET_DEBUG)
    }

    /// Enable or disable TTY debug logging.
    ///
    /// # Arguments
    ///
    /// * `enabled` - New debug logging state.
    pub fn set_debug_enabled(&self, enabled: bool) -> Result<()> {
        self.set_bool(SCTL_TTY_SET_DEBUG, enabled)
    }

    /// Flush queued terminal input.
    pub fn flush_input(&self) -> Result<()> {
        self.control(SCTL_TTY_FLUSH_INPUT, 0).map(|_| ())
    }

    /// Return the foreground process group visible to this task.
    ///
    /// # Returns
    ///
    /// Foreground process group, or `None` when the terminal has no foreground
    /// group.
    pub fn foreground_group(&self) -> Result<Option<usize>> {
        let value = self.control(SCTL_TTY_GET_FOREGROUND_GROUP, 0)?;
        if value < 0 {
            Ok(None)
        } else {
            Ok(Some(value as usize))
        }
    }

    /// Set the foreground process group.
    ///
    /// # Arguments
    ///
    /// * `process_group_id` - User-visible process group ID.
    pub fn set_foreground_group(&self, process_group_id: usize) -> Result<()> {
        self.control(SCTL_TTY_SET_FOREGROUND_GROUP, process_group_id)
            .map(|_| ())
    }

    fn get_bool(&self, command: u32) -> Result<bool> {
        self.control(command, 0).map(|value| value != 0)
    }

    fn set_bool(&self, command: u32, enabled: bool) -> Result<()> {
        self.control(command, enabled as usize).map(|_| ())
    }

    fn control(&self, command: u32, arg: usize) -> Result<i32> {
        self.handle
            .control(command, arg)
            .map_err(control_error_to_io_error)
    }
}

fn control_error_to_io_error(error: HandleError) -> Error {
    match error {
        HandleError::Unsupported => Error::new(ErrorKind::Unsupported, "TTY control unsupported"),
        HandleError::PermissionDenied => {
            Error::new(ErrorKind::PermissionDenied, "TTY control denied")
        }
        HandleError::InvalidHandle => Error::new(ErrorKind::InvalidInput, "invalid TTY handle"),
        HandleError::InvalidParameter => {
            Error::new(ErrorKind::InvalidInput, "invalid TTY control argument")
        }
        HandleError::NotFound => Error::new(ErrorKind::NotFound, "TTY object not found"),
        HandleError::OutOfResources => {
            Error::new(ErrorKind::OutOfMemory, "TTY control out of resources")
        }
        HandleError::SystemError(_) => Error::new(ErrorKind::Other, "TTY control failed"),
    }
}
