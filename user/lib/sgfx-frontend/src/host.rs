//! Window-surface integration for host SGFX backends.

use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use sgfx_core::backend::CommandExecutor;

use crate::{BackendKind, Error, Instance, Result, ir};

/// Selected host window context and presentation surface.
pub struct WindowContext {
    backend: sgfx_backend_wgpu::WindowContext,
}

impl WindowContext {
    /// Create mapped physical targets for one logical resource table.
    ///
    /// # Arguments
    ///
    /// * `resources` - Logical SGFX resource table.
    /// * `targets` - Presentation texture identities to materialize.
    ///
    /// # Returns
    ///
    /// A backend-owned mapped target session.
    pub fn create_mapped_target_session(
        &self,
        resources: alloc::rc::Rc<ir::ResourceTable>,
        targets: &[ir::TextureId],
    ) -> Result<MappedTargetSession> {
        self.backend
            .create_mapped_target_session(resources, targets)
            .map(|backend| MappedTargetSession { backend })
            .map_err(Error::Wgpu)
    }

    /// Reconfigure the presentation surface after a physical resize.
    ///
    /// # Arguments
    ///
    /// * `width` - New non-zero physical width.
    /// * `height` - New non-zero physical height.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.backend.resize(width, height);
    }

    /// Present one mapped target through the selected host backend.
    ///
    /// # Arguments
    ///
    /// * `session` - Session that owns the mapped image.
    /// * `target` - Logical texture identity to present.
    ///
    /// # Returns
    ///
    /// Success after queuing and presenting the surface frame.
    pub fn present(&mut self, session: &MappedTargetSession, target: ir::TextureId) -> Result<()> {
        self.backend
            .present(&session.backend, target)
            .map_err(Error::Wgpu)
    }

    /// Return whether the selected backend supports depth attachments.
    ///
    /// # Returns
    ///
    /// `true` when depth-enabled SGFX canvas passes are available.
    pub const fn supports_depth(&self) -> bool {
        true
    }
}

impl Instance {
    /// Create a host presentation context from borrowed native window handles.
    ///
    /// # Arguments
    ///
    /// * `raw_display_handle` - Display handle retained by the platform window.
    /// * `raw_window_handle` - Window handle retained by the platform window.
    /// * `width` - Initial physical surface width.
    /// * `height` - Initial physical surface height.
    ///
    /// # Returns
    ///
    /// A selected SGFX host window context.
    ///
    /// # Safety
    ///
    /// Both raw handles must remain valid until the returned context is
    /// dropped. The platform must therefore drop its SGFX renderer before the
    /// native window and display objects.
    pub unsafe fn create_window_context(
        &self,
        raw_display_handle: RawDisplayHandle,
        raw_window_handle: RawWindowHandle,
        width: u32,
        height: u32,
    ) -> Result<WindowContext> {
        match self.backend() {
            BackendKind::Wgpu => {
                // SAFETY: the caller upholds the raw-handle lifetime contract.
                unsafe {
                    sgfx_backend_wgpu::WindowContext::new(
                        raw_display_handle,
                        raw_window_handle,
                        width,
                        height,
                    )
                }
                .map(|backend| WindowContext { backend })
                .map_err(Error::Wgpu)
            }
            backend => Err(Error::BackendUnavailable(backend)),
        }
    }
}

/// Backend-owned host mapped-target session.
pub struct MappedTargetSession {
    backend: sgfx_backend_wgpu::MappedTargetSession,
}

impl MappedTargetSession {
    /// Bind the selected backend queue and resources for command execution.
    ///
    /// # Returns
    ///
    /// A frontend executor delegating complete command buffers to WGPU.
    pub fn executor(&mut self) -> Executor<'_> {
        Executor {
            backend: self.backend.executor(),
        }
    }
}

/// Host command executor selected by the SGFX frontend.
pub struct Executor<'a> {
    backend: sgfx_backend_wgpu::Executor<'a>,
}

impl CommandExecutor for Executor<'_> {
    type Error = Error;

    fn execute<'r, 'data>(&mut self, commands: &ir::CommandBuffer<'r, 'data>) -> Result<()> {
        self.backend.execute(commands).map_err(Error::Wgpu)
    }
}
