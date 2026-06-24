//! Common clock framework abstractions for kernel device drivers.
//!
//! The clk subsystem exposes provider-neutral clock handles with reference-counted
//! prepare/enable state and optional rate/parent control.

extern crate alloc;

use alloc::sync::Arc;
use core::ops::{BitOr, BitOrAssign};
use spin::Mutex;

/// Clock operation errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClkError {
    /// Operation is not supported by this clock.
    Unsupported,
    /// Requested rate is invalid for this clock.
    InvalidRate,
    /// Requested parent is invalid for this clock.
    InvalidParent,
    /// Referenced clock provider was not found.
    ProviderNotFound,
    /// Referenced clock was not found in a provider.
    ClockNotFound,
    /// Clock specifier cells are malformed or invalid.
    InvalidSpecifier,
    /// Hardware access failed.
    HardwareError,
    /// Clock is busy and cannot satisfy the operation.
    Busy,
    /// Requested item was not found.
    NotFound,
}

/// Clock capability and behavior flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClkFlags(u32);

impl ClkFlags {
    /// No clock flags.
    pub const NONE: Self = Self(0);
    /// Propagate rate changes to the parent when local setting is unsupported.
    pub const SET_RATE_PARENT: Self = Self(1 << 0);
    /// Clock is critical and should not be disabled by generic code.
    pub const IS_CRITICAL: Self = Self(1 << 1);

    /// Returns true when all bits in `other` are present.
    ///
    /// # Arguments
    ///
    /// * `other` - Flags that must be contained in `self`.
    ///
    /// # Returns
    ///
    /// `true` if every bit from `other` is set in `self`.
    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Return the raw flag bits.
    ///
    /// # Returns
    ///
    /// Raw `u32` representation of the flags.
    pub fn bits(self) -> u32 {
        self.0
    }
}

impl BitOr for ClkFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for ClkFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// A single clock exposed by a clock provider.
pub trait Clk: Send + Sync {
    /// Return the clock name.
    ///
    /// # Returns
    ///
    /// Static name used for logging and diagnostics.
    fn name(&self) -> &'static str;

    /// Return clock flags.
    ///
    /// # Returns
    ///
    /// Behavior flags for this clock.
    fn flags(&self) -> ClkFlags {
        ClkFlags::NONE
    }

    /// Prepare the clock before enabling it.
    ///
    /// # Returns
    ///
    /// `Ok(())` when preparation succeeds.
    fn prepare(&self) -> Result<(), ClkError> {
        Ok(())
    }

    /// Undo a previous prepare operation.
    fn unprepare(&self) {}

    /// Enable the clock output.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the clock output is enabled.
    fn enable(&self) -> Result<(), ClkError>;

    /// Disable the clock output.
    fn disable(&self);

    /// Check whether the clock is enabled in hardware.
    ///
    /// # Returns
    ///
    /// `true` if the clock is currently enabled.
    fn is_enabled(&self) -> bool;

    /// Recalculate this clock's rate from its parent rate.
    ///
    /// # Arguments
    ///
    /// * `parent_rate` - Current parent clock rate in Hz, or 0 for root clocks.
    ///
    /// # Returns
    ///
    /// Current clock rate in Hz.
    fn recalc_rate(&self, parent_rate: u64) -> u64;

    /// Round a requested rate to the nearest supported rate.
    ///
    /// # Arguments
    ///
    /// * `rate` - Requested rate in Hz.
    /// * `parent_rate` - Current parent rate in Hz.
    ///
    /// # Returns
    ///
    /// Supported rate closest to `rate`.
    fn round_rate(&self, rate: u64, parent_rate: u64) -> Result<u64, ClkError> {
        let _ = parent_rate;
        Ok(rate)
    }

    /// Set this clock's rate.
    ///
    /// # Arguments
    ///
    /// * `rate` - Requested rate in Hz.
    /// * `parent_rate` - Current parent rate in Hz.
    ///
    /// # Returns
    ///
    /// Actual programmed rate in Hz.
    fn set_rate(&self, rate: u64, parent_rate: u64) -> Result<u64, ClkError> {
        let _ = (rate, parent_rate);
        Err(ClkError::Unsupported)
    }

    /// Return this clock's parent.
    ///
    /// # Returns
    ///
    /// Parent clock handle, or `None` for root clocks.
    fn parent(&self) -> Option<ClkHandle> {
        None
    }

    /// Change this clock's parent.
    ///
    /// # Arguments
    ///
    /// * `parent` - New parent clock handle.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the parent was changed.
    fn set_parent(&self, parent: ClkHandle) -> Result<(), ClkError> {
        let _ = parent;
        Err(ClkError::Unsupported)
    }
}

/// Mutable state shared by cloned clock handles.
pub struct ClkState {
    /// Clock implementation.
    pub clk: Arc<dyn Clk>,
    /// Number of active prepare users.
    pub prepare_count: usize,
    /// Number of active enable users.
    pub enable_count: usize,
    /// Last observed rate, kept only as a diagnostic cache.
    pub cached_rate: Option<u64>,
}

/// Reference-counted handle to a clock.
#[derive(Clone)]
pub struct ClkHandle {
    state: Arc<Mutex<ClkState>>,
}

impl ClkHandle {
    /// Create a new clock handle.
    ///
    /// # Arguments
    ///
    /// * `clk` - Clock implementation to wrap.
    ///
    /// # Returns
    ///
    /// A handle with zero prepare and enable users.
    pub fn new(clk: Arc<dyn Clk>) -> Self {
        Self {
            state: Arc::new(Mutex::new(ClkState {
                clk,
                prepare_count: 0,
                enable_count: 0,
                cached_rate: None,
            })),
        }
    }

    /// Return the clock name.
    ///
    /// # Returns
    ///
    /// Static clock name.
    pub fn name(&self) -> &'static str {
        self.clk().name()
    }

    /// Prepare and enable the clock with reference counting.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the clock is prepared and enabled.
    pub fn prepare_enable(&self) -> Result<(), ClkError> {
        let clk = self.clk();
        let needs_prepare = self.state.lock().prepare_count == 0;
        if needs_prepare {
            clk.prepare()?;
        }

        {
            let mut state = self.state.lock();
            state.prepare_count += 1;
        }

        let needs_enable = self.state.lock().enable_count == 0;
        if needs_enable && let Err(err) = clk.enable() {
            self.unwind_prepare_after_enable_error(&clk);
            return Err(err);
        }

        self.state.lock().enable_count += 1;
        Ok(())
    }

    /// Disable and unprepare the clock, saturating at zero users.
    pub fn disable_unprepare(&self) {
        let clk = self.clk();
        let (call_disable, call_unprepare) = {
            let mut state = self.state.lock();
            let call_disable = if state.enable_count > 0 {
                state.enable_count -= 1;
                state.enable_count == 0
            } else {
                false
            };
            let call_unprepare = if state.prepare_count > 0 {
                state.prepare_count -= 1;
                state.prepare_count == 0
            } else {
                false
            };
            (call_disable, call_unprepare)
        };

        if call_disable {
            clk.disable();
        }
        if call_unprepare {
            clk.unprepare();
        }
    }

    /// Recalculate and return the current clock rate.
    ///
    /// # Returns
    ///
    /// Current clock rate in Hz.
    pub fn rate(&self) -> u64 {
        let clk = self.clk();
        let parent_rate = clk.parent().map_or(0, |parent| parent.rate());
        let rate = clk.recalc_rate(parent_rate);
        self.state.lock().cached_rate = Some(rate);
        rate
    }

    /// Round a requested rate using the clock implementation.
    ///
    /// # Arguments
    ///
    /// * `rate` - Requested rate in Hz.
    ///
    /// # Returns
    ///
    /// Supported rounded rate in Hz.
    pub fn round_rate(&self, rate: u64) -> Result<u64, ClkError> {
        let clk = self.clk();
        let parent_rate = clk.parent().map_or(0, |parent| parent.rate());
        clk.round_rate(rate, parent_rate)
    }

    /// Set this clock's rate, optionally propagating to the parent.
    ///
    /// # Arguments
    ///
    /// * `rate` - Requested rate in Hz.
    ///
    /// # Returns
    ///
    /// Actual programmed rate in Hz.
    pub fn set_rate(&self, rate: u64) -> Result<u64, ClkError> {
        let clk = self.clk();
        let parent = clk.parent();
        let parent_rate = parent.as_ref().map_or(0, ClkHandle::rate);

        match clk.set_rate(rate, parent_rate) {
            Ok(actual) => {
                self.state.lock().cached_rate = Some(actual);
                Ok(actual)
            }
            Err(ClkError::Unsupported) if clk.flags().contains(ClkFlags::SET_RATE_PARENT) => {
                let parent = parent.ok_or(ClkError::Unsupported)?;
                parent.set_rate(rate)?;
                let new_parent_rate = parent.rate();
                let actual = clk.set_rate(rate, new_parent_rate)?;
                self.state.lock().cached_rate = Some(actual);
                Ok(actual)
            }
            Err(err) => Err(err),
        }
    }

    /// Set this clock's parent.
    ///
    /// # Arguments
    ///
    /// * `parent` - New parent clock handle.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the parent was changed.
    pub fn set_parent(&self, parent: ClkHandle) -> Result<(), ClkError> {
        let clk = self.clk();
        clk.set_parent(parent)?;
        self.state.lock().cached_rate = None;
        Ok(())
    }

    /// Check whether the clock is enabled.
    ///
    /// # Returns
    ///
    /// `true` when the underlying clock reports enabled.
    pub fn is_enabled(&self) -> bool {
        self.clk().is_enabled()
    }

    fn clk(&self) -> Arc<dyn Clk> {
        self.state.lock().clk.clone()
    }

    fn unwind_prepare_after_enable_error(&self, clk: &Arc<dyn Clk>) {
        let call_unprepare = {
            let mut state = self.state.lock();
            if state.prepare_count > 0 {
                state.prepare_count -= 1;
                state.prepare_count == 0
            } else {
                false
            }
        };

        if call_unprepare {
            clk.unprepare();
        }
    }
}

/// Clock provider registered by firmware phandle.
pub trait ClkProvider: Send + Sync {
    /// Return the provider name.
    ///
    /// # Returns
    ///
    /// Static provider name used for diagnostics.
    fn name(&self) -> &'static str;

    /// Return the number of specifier cells after the provider phandle.
    ///
    /// # Returns
    ///
    /// Number of `u32` cells required to identify a clock from this provider.
    fn clock_cells(&self) -> usize;

    /// Resolve a clock by provider-specific specifier cells.
    ///
    /// # Arguments
    ///
    /// * `spec` - Provider-specific clock specifier cells.
    ///
    /// # Returns
    ///
    /// Clock handle for the requested specifier.
    fn get_clk(&self, spec: &[u32]) -> Result<ClkHandle, ClkError>;

    /// Apply an assigned rate to a provider-specific clock.
    ///
    /// # Arguments
    ///
    /// * `spec` - Provider-specific clock specifier cells.
    /// * `rate` - Assigned rate in Hz.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the rate was applied.
    fn apply_assigned_rate(&self, spec: &[u32], rate: u64) -> Result<(), ClkError> {
        self.get_clk(spec)?.set_rate(rate)?;
        Ok(())
    }

    /// Apply an assigned parent to a provider-specific clock.
    ///
    /// # Arguments
    ///
    /// * `spec` - Provider-specific clock specifier cells.
    /// * `parent` - Parent clock to assign.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the parent was applied.
    fn apply_assigned_parent(&self, spec: &[u32], parent: ClkHandle) -> Result<(), ClkError> {
        self.get_clk(spec)?.set_parent(parent)
    }
}

/// Fixed-rate root clock helper.
pub struct ClkFixedRate {
    name: &'static str,
    rate: u64,
}

impl ClkFixedRate {
    /// Create a fixed-rate clock.
    ///
    /// # Arguments
    ///
    /// * `name` - Static clock name.
    /// * `rate` - Fixed clock rate in Hz.
    ///
    /// # Returns
    ///
    /// A fixed-rate clock instance.
    pub const fn new(name: &'static str, rate: u64) -> Self {
        Self { name, rate }
    }
}

impl Clk for ClkFixedRate {
    fn name(&self) -> &'static str {
        self.name
    }

    fn enable(&self) -> Result<(), ClkError> {
        Ok(())
    }

    fn disable(&self) {}

    fn is_enabled(&self) -> bool {
        true
    }

    fn recalc_rate(&self, _parent_rate: u64) -> u64 {
        self.rate
    }

    fn round_rate(&self, _rate: u64, _parent_rate: u64) -> Result<u64, ClkError> {
        Ok(self.rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    struct TestClk {
        name: &'static str,
        flags: ClkFlags,
        parent: Option<ClkHandle>,
        rate: AtomicU64,
        prepared: AtomicUsize,
        enabled: AtomicUsize,
        fail_enable: AtomicBool,
        allow_set_rate: AtomicBool,
    }

    impl TestClk {
        fn new(name: &'static str, rate: u64) -> Self {
            Self {
                name,
                flags: ClkFlags::NONE,
                parent: None,
                rate: AtomicU64::new(rate),
                prepared: AtomicUsize::new(0),
                enabled: AtomicUsize::new(0),
                fail_enable: AtomicBool::new(false),
                allow_set_rate: AtomicBool::new(true),
            }
        }

        fn with_parent(mut self, parent: ClkHandle) -> Self {
            self.parent = Some(parent);
            self
        }

        fn with_flags(mut self, flags: ClkFlags) -> Self {
            self.flags = flags;
            self
        }
    }

    impl Clk for TestClk {
        fn name(&self) -> &'static str {
            self.name
        }

        fn flags(&self) -> ClkFlags {
            self.flags
        }

        fn prepare(&self) -> Result<(), ClkError> {
            self.prepared.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn unprepare(&self) {
            self.prepared.fetch_sub(1, Ordering::SeqCst);
        }

        fn enable(&self) -> Result<(), ClkError> {
            if self.fail_enable.load(Ordering::SeqCst) {
                return Err(ClkError::HardwareError);
            }
            self.enabled.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn disable(&self) {
            self.enabled.fetch_sub(1, Ordering::SeqCst);
        }

        fn is_enabled(&self) -> bool {
            self.enabled.load(Ordering::SeqCst) > 0
        }

        fn recalc_rate(&self, parent_rate: u64) -> u64 {
            self.rate.load(Ordering::SeqCst) + parent_rate
        }

        fn set_rate(&self, rate: u64, _parent_rate: u64) -> Result<u64, ClkError> {
            if !self.allow_set_rate.load(Ordering::SeqCst) {
                return Err(ClkError::Unsupported);
            }
            self.rate.store(rate, Ordering::SeqCst);
            Ok(rate)
        }

        fn parent(&self) -> Option<ClkHandle> {
            self.parent.clone()
        }
    }

    #[test_case]
    fn test_clk_flags_contains_and_bitor() {
        let flags = ClkFlags::SET_RATE_PARENT | ClkFlags::IS_CRITICAL;
        assert!(flags.contains(ClkFlags::SET_RATE_PARENT));
        assert!(flags.contains(ClkFlags::IS_CRITICAL));
        assert_eq!(flags.bits(), 0b11);
    }

    #[test_case]
    fn test_prepare_enable_refcounts() {
        let clk_impl = Arc::new(TestClk::new("test", 1));
        let clk = ClkHandle::new(clk_impl.clone());
        assert!(clk.prepare_enable().is_ok());
        assert!(clk.prepare_enable().is_ok());
        assert_eq!(clk_impl.prepared.load(Ordering::SeqCst), 1);
        assert_eq!(clk_impl.enabled.load(Ordering::SeqCst), 1);
        clk.disable_unprepare();
        assert_eq!(clk_impl.prepared.load(Ordering::SeqCst), 1);
        assert_eq!(clk_impl.enabled.load(Ordering::SeqCst), 1);
        clk.disable_unprepare();
        assert_eq!(clk_impl.prepared.load(Ordering::SeqCst), 0);
        assert_eq!(clk_impl.enabled.load(Ordering::SeqCst), 0);
    }

    #[test_case]
    fn test_prepare_enable_unwinds_on_enable_error() {
        let clk_impl = Arc::new(TestClk::new("test", 1));
        clk_impl.fail_enable.store(true, Ordering::SeqCst);
        let clk = ClkHandle::new(clk_impl.clone());
        assert_eq!(clk.prepare_enable(), Err(ClkError::HardwareError));
        assert_eq!(clk_impl.prepared.load(Ordering::SeqCst), 0);
        assert_eq!(clk_impl.enabled.load(Ordering::SeqCst), 0);
    }

    #[test_case]
    fn test_disable_unprepare_is_idempotent() {
        let clk_impl = Arc::new(TestClk::new("test", 1));
        let clk = ClkHandle::new(clk_impl.clone());
        clk.disable_unprepare();
        clk.disable_unprepare();
        assert_eq!(clk_impl.prepared.load(Ordering::SeqCst), 0);
        assert_eq!(clk_impl.enabled.load(Ordering::SeqCst), 0);
    }

    #[test_case]
    fn test_rate_recalculates_from_parent() {
        let parent = ClkHandle::new(Arc::new(TestClk::new("parent", 24)));
        let child = ClkHandle::new(Arc::new(TestClk::new("child", 2).with_parent(parent)));
        assert_eq!(child.rate(), 26);
    }

    #[test_case]
    fn test_set_rate_parent_propagates_only_with_flag() {
        let parent_impl = Arc::new(TestClk::new("parent", 24));
        let parent = ClkHandle::new(parent_impl.clone());
        let child_impl = Arc::new(
            TestClk::new("child", 2)
                .with_parent(parent)
                .with_flags(ClkFlags::SET_RATE_PARENT),
        );
        child_impl.allow_set_rate.store(false, Ordering::SeqCst);
        let child = ClkHandle::new(child_impl);
        assert_eq!(child.set_rate(48), Err(ClkError::Unsupported));
        assert_eq!(parent_impl.rate.load(Ordering::SeqCst), 48);
    }

    #[test_case]
    fn test_set_rate_without_flag_returns_unsupported() {
        let parent = ClkHandle::new(Arc::new(TestClk::new("parent", 24)));
        let child_impl = Arc::new(TestClk::new("child", 2).with_parent(parent));
        child_impl.allow_set_rate.store(false, Ordering::SeqCst);
        let child = ClkHandle::new(child_impl);
        assert_eq!(child.set_rate(48), Err(ClkError::Unsupported));
    }

    #[test_case]
    fn test_fixed_rate_rate() {
        let clk = ClkHandle::new(Arc::new(ClkFixedRate::new("fixed", 12_000_000)));
        assert_eq!(clk.rate(), 12_000_000);
    }

    #[test_case]
    fn test_fixed_rate_prepare_enable() {
        let clk = ClkHandle::new(Arc::new(ClkFixedRate::new("fixed", 12_000_000)));
        assert!(clk.prepare_enable().is_ok());
        assert!(clk.is_enabled());
        clk.disable_unprepare();
    }

    #[test_case]
    fn test_fixed_rate_set_rate_unsupported() {
        let clk = ClkHandle::new(Arc::new(ClkFixedRate::new("fixed", 12_000_000)));
        assert_eq!(clk.set_rate(24_000_000), Err(ClkError::Unsupported));
    }
}
