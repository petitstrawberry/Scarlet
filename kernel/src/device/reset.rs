//! Reset controller registration and handle abstractions.

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;

/// Reset line handle resolved from a firmware reset specifier.
#[derive(Clone)]
pub struct ResetHandle {
    controller: Arc<dyn ResetController>,
    spec: Vec<u32>,
}

impl ResetHandle {
    /// Create a reset handle from a controller and provider-specific cells.
    ///
    /// # Arguments
    ///
    /// * `controller` - Reset controller that owns the reset line.
    /// * `spec` - Provider-specific reset specifier cells.
    ///
    /// # Returns
    ///
    /// A handle that can assert or deassert the referenced reset line.
    pub fn new(controller: Arc<dyn ResetController>, spec: Vec<u32>) -> Self {
        Self { controller, spec }
    }

    /// Assert this reset line.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the controller accepted the assert operation.
    pub fn assert(&self) -> Result<(), &'static str> {
        self.controller.assert_reset(&self.spec)
    }

    /// Deassert this reset line.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the controller accepted the deassert operation.
    pub fn deassert(&self) -> Result<(), &'static str> {
        self.controller.deassert_reset(&self.spec)
    }

    /// Assert and then deassert this reset line.
    ///
    /// # Returns
    ///
    /// `Ok(())` when both operations completed.
    pub fn reset(&self) -> Result<(), &'static str> {
        self.controller.reset(&self.spec)
    }
}

/// Reset controller exposed by firmware phandle.
pub trait ResetController: Send + Sync {
    /// Return the provider name.
    ///
    /// # Returns
    ///
    /// Static provider name used for diagnostics.
    fn name(&self) -> &'static str;

    /// Return the number of cells after the provider phandle.
    ///
    /// # Returns
    ///
    /// Number of `u32` cells required to identify a reset line.
    fn reset_cells(&self) -> usize;

    /// Assert a provider-specific reset line.
    ///
    /// # Arguments
    ///
    /// * `spec` - Provider-specific reset specifier cells.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the reset line is asserted.
    fn assert_reset(&self, spec: &[u32]) -> Result<(), &'static str>;

    /// Deassert a provider-specific reset line.
    ///
    /// # Arguments
    ///
    /// * `spec` - Provider-specific reset specifier cells.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the reset line is deasserted.
    fn deassert_reset(&self, spec: &[u32]) -> Result<(), &'static str>;

    /// Assert and then deassert a provider-specific reset line.
    ///
    /// # Arguments
    ///
    /// * `spec` - Provider-specific reset specifier cells.
    ///
    /// # Returns
    ///
    /// `Ok(())` when both operations completed.
    fn reset(&self, spec: &[u32]) -> Result<(), &'static str> {
        self.assert_reset(spec)?;
        self.deassert_reset(spec)
    }
}
