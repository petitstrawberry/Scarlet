//! Watchdog subsystem abstractions.
//!
//! Provides a registry of hardware watchdog timers. The framework is intentionally
//! minimal: it exposes start/stop/ping/set_timeout for each registered watchdog.
//! A future watchdog daemon can ping all registered watchdogs on a timer.

/// Watchdog operation errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogError {
    /// Operation is not supported by this watchdog.
    NotSupported,
    /// Requested timeout is invalid or outside the supported range.
    InvalidTimeout,
    /// Watchdog is already running.
    AlreadyRunning,
    /// Watchdog is not currently running.
    NotRunning,
    /// Hardware access failed.
    HardwareError,
    /// Watchdog is busy and cannot satisfy the operation.
    Busy,
}

/// Watchdog timer trait.
pub trait Watchdog: Send + Sync {
    /// Return the watchdog name.
    ///
    /// # Returns
    ///
    /// Static name used for logging and diagnostics.
    fn name(&self) -> &'static str;

    /// Start the watchdog with the given timeout in milliseconds.
    ///
    /// If `timeout_ms` is 0, use the hardware default.
    ///
    /// # Arguments
    ///
    /// * `timeout_ms` - Requested timeout in milliseconds, or 0 for the hardware default.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the watchdog was started.
    fn start(&self, timeout_ms: u32) -> Result<(), WatchdogError>;

    /// Stop the watchdog.
    ///
    /// Hardware that cannot be stopped may return [`WatchdogError::NotSupported`].
    ///
    /// # Returns
    ///
    /// `Ok(())` when the watchdog was stopped.
    fn stop(&self) -> Result<(), WatchdogError>;

    /// Ping (kick) the watchdog to reset the timer.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the watchdog timer was reset.
    fn ping(&self) -> Result<(), WatchdogError>;

    /// Check if the watchdog is currently running.
    ///
    /// # Returns
    ///
    /// `true` if the watchdog is enabled in hardware.
    fn is_running(&self) -> bool;

    /// Set the timeout in milliseconds.
    ///
    /// # Arguments
    ///
    /// * `timeout_ms` - Requested timeout in milliseconds.
    ///
    /// # Returns
    ///
    /// Actual timeout programmed in milliseconds.
    fn set_timeout(&self, timeout_ms: u32) -> Result<u32, WatchdogError>;

    /// Get the current timeout in milliseconds, or `None` if not set.
    ///
    /// # Returns
    ///
    /// Current timeout in milliseconds, or `None` when unavailable.
    fn get_timeout(&self) -> Option<u32>;

    /// Get the minimum supported timeout in milliseconds.
    ///
    /// # Returns
    ///
    /// Minimum timeout in milliseconds.
    fn min_timeout(&self) -> u32 {
        1
    }

    /// Get the maximum supported timeout in milliseconds.
    ///
    /// # Returns
    ///
    /// Maximum timeout in milliseconds.
    fn max_timeout(&self) -> u32 {
        u32::MAX
    }

    /// Get the last reset reason, if known.
    ///
    /// # Returns
    ///
    /// Hardware-specific reset reason, or `None` when unavailable.
    fn last_reset_reason(&self) -> Option<u32> {
        None
    }
}
