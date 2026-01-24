//! SWS (Scarlet Window Server) backend for PlatformWindow
//!
//! This implementation uses the sws-client library to create and manage windows.

use crate::geometry::Size;
use crate::buffer::Buffer;
use crate::event::Event;
use crate::error::Result;
use crate::platform::PlatformWindow;
use sws_client as sws;

/// SWS platform window implementation
pub struct SWSPlatformWindow {
    conn: sws::Connection,
    surface_id: u32,
    current_size: Size,
}

impl SWSPlatformWindow {
    /// Get the surface ID
    pub fn surface_id(&self) -> u32 {
        self.surface_id
    }

    /// Get the connection
    pub fn connection(&self) -> &sws::Connection {
        &self.conn
    }

    /// Get mutable reference to the connection
    pub fn connection_mut(&mut self) -> &mut sws::Connection {
        &mut self.conn
    }
}

impl PlatformWindow for SWSPlatformWindow {
    fn new(app_id: &str, title: &str, size: Size) -> Result<Self> {
        // Connect to SWS
        let mut conn = sws::Connection::connect("/tmp/sws.sock")
            .map_err(|_| crate::error::Error::ConnectionFailed)?;

        // Create surface
        let surface_id = conn.create_surface(
            app_id,
            title,
            "",
            size.width as u32,
            size.height as u32,
        ).map_err(|_| crate::error::Error::SurfaceCreationFailed)?;

        Ok(Self {
            conn,
            surface_id,
            current_size: size,
        })
    }

    fn poll_event(&mut self) -> Option<Event> {
        // Dispatch events
        let _ = self.conn.dispatch().ok();

        // Check for events
        // Note: Full event polling implementation will come later
        // For now, return None to avoid blocking
        None
    }

    fn present(&mut self, buffer: &Buffer) {
        // Get the surface and copy pixels
        if let Some(surface) = self.conn.surface_mut(self.surface_id) {
            // Get the shared memory buffer
            surface.with_buffer(|shm_buf, width, height| {
                // SWS shared memory is width * height * 4 bytes (BGRA u8 array)
                let src_data = buffer.data(); // &[u8]
                let shm_len = (width * height * 4) as usize;
                let dst_data = unsafe {
                    core::slice::from_raw_parts_mut(shm_buf.as_mut_ptr(), shm_len)
                };

                // Copy u8 bytes directly
                let copy_len = src_data.len().min(shm_len);
                dst_data[..copy_len].copy_from_slice(&src_data[..copy_len]);
            });
        }

        // Commit the surface
        let _ = self.conn.commit(self.surface_id);
    }

    fn set_title(&mut self, title: &str) {
        // Note: sws-client doesn't have a set_surface_title method
        // The title is set during surface creation
        let _ = title;
    }

    fn size(&self) -> Size {
        self.current_size
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        // Note: sws-client doesn't have a resize_surface method
        // Resize would need to be implemented in the protocol
        // For now, just update our tracked size
        self.current_size = Size {
            width: width as f32,
            height: height as f32,
        };

        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        // Destroy the surface
        self.conn.destroy_surface(self.surface_id)
            .map_err(|_| crate::error::Error::IoError)?;

        Ok(())
    }
}
