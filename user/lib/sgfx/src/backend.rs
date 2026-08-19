//! Backend-owned execution contracts for portable SGFX command buffers.

use crate::ir::CommandBuffer;

/// Executes portable SGFX command buffers using backend-owned state.
///
/// An implementation binds the queue, resource cache, platform context, and
/// any transport-specific state needed to validate, lower, and submit the IR.
/// Portable renderer code only records a complete [`CommandBuffer`] and does
/// not account for backend command sizes or submission boundaries.
pub trait CommandExecutor {
    /// Error returned when validation, lowering, or submission fails.
    type Error;

    /// Validate, lower, and execute one complete command buffer.
    ///
    /// # Arguments
    ///
    /// * `commands` - Portable commands and borrowed upload data to execute.
    ///
    /// # Returns
    ///
    /// Success after the backend's submission contract is satisfied, or the
    /// backend-specific validation, allocation, transport, or device error.
    fn execute<'r, 'data>(
        &mut self,
        commands: &CommandBuffer<'r, 'data>,
    ) -> Result<(), Self::Error>;
}
