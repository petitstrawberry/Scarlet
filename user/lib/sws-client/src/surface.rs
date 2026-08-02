//! Surface (window buffer) management

use crate::Error;
use crate::os::{SharedMemory, mmap_flags, munmap, permissions};

/// A surface represents a drawable window buffer
///
/// The surface owns the shared memory buffer that is shared with the compositor.
/// Drawing is done directly to this buffer, then committed to notify the server.
pub struct Surface {
    /// Surface ID assigned by the server
    id: u32,
    /// Width in pixels
    width: u32,
    /// Height in pixels
    height: u32,
    /// Shared memory backing the pixel buffer
    shm: SharedMemory,
    /// Mapped buffer pointer
    buffer_ptr: *mut u8,
    /// Mapped buffer length in bytes
    buffer_len: usize,
    /// Whether the surface has uncommitted changes
    dirty: bool,
}

// SAFETY: `Surface` owns its mapping and never exposes the raw pointer without
// borrowing itself. Shared connections guard every surface with a mutex, so a
// surface can be transferred between threads without concurrent mutable access.
unsafe impl Send for Surface {}

impl Surface {
    /// Create a new surface from server-provided resources
    pub(crate) fn new(id: u32, width: u32, height: u32, shm: SharedMemory) -> Result<Self, Error> {
        let (buffer_ptr, buffer_len, _addr) = Self::map_shm(&shm, width, height)?;

        Ok(Self {
            id,
            width,
            height,
            shm,
            buffer_ptr,
            buffer_len,
            dirty: false,
        })
    }

    fn map_shm(
        shm: &SharedMemory,
        width: u32,
        height: u32,
    ) -> Result<(*mut u8, usize, usize), Error> {
        let buffer_size = (width * height * 4) as usize;

        let addr = shm
            .as_handle()
            .as_memory_mapping()
            .map_err(|_| Error::ShmMapFailed)?
            .mmap(
                0,
                buffer_size,
                permissions::READ_WRITE,
                mmap_flags::SHARED,
                0,
            )
            .map_err(|_| Error::ShmMapFailed)?;

        Ok((addr as *mut u8, buffer_size, addr))
    }

    pub(crate) fn remap(
        &mut self,
        width: u32,
        height: u32,
        shm: SharedMemory,
    ) -> Result<(), Error> {
        let (buffer_ptr, buffer_len, _addr) = Self::map_shm(&shm, width, height)?;
        let _ = munmap(self.buffer_ptr as usize, self.buffer_len);
        self.width = width;
        self.height = height;
        self.shm = shm;
        self.buffer_ptr = buffer_ptr;
        self.buffer_len = buffer_len;
        self.dirty = true;
        Ok(())
    }

    /// Get the surface ID
    #[inline]
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Get surface width in pixels
    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Get surface height in pixels
    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Get read-only access to the pixel buffer
    #[inline]
    pub fn buffer(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.buffer_ptr as *const u8, self.buffer_len) }
    }

    /// Get mutable access to the pixel buffer
    ///
    /// This marks the surface as dirty, requiring a commit.
    #[inline]
    pub fn buffer_mut(&mut self) -> &mut [u8] {
        self.dirty = true;
        unsafe { core::slice::from_raw_parts_mut(self.buffer_ptr, self.buffer_len) }
    }

    /// Execute a closure with mutable access to the buffer
    ///
    /// This is the preferred way to draw to the surface.
    #[inline]
    pub fn with_buffer<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8], u32, u32) -> R,
    {
        self.dirty = true;
        let buf = unsafe { core::slice::from_raw_parts_mut(self.buffer_ptr, self.buffer_len) };
        f(buf, self.width, self.height)
    }

    /// Check if surface has uncommitted changes
    #[inline]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Clear the dirty flag (called after commit)
    pub(crate) fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Get pixel at (x, y) as BGRA
    pub fn get_pixel(&self, x: u32, y: u32) -> Option<(u8, u8, u8, u8)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = ((y * self.width + x) * 4) as usize;
        Some((
            self.buffer()[offset],     // B
            self.buffer()[offset + 1], // G
            self.buffer()[offset + 2], // R
            self.buffer()[offset + 3], // A
        ))
    }

    /// Set pixel at (x, y) to BGRA
    pub fn set_pixel(&mut self, x: u32, y: u32, b: u8, g: u8, r: u8, a: u8) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = ((y * self.width + x) * 4) as usize;
        let buf = self.buffer_mut();
        buf[offset] = b;
        buf[offset + 1] = g;
        buf[offset + 2] = r;
        buf[offset + 3] = a;
        self.dirty = true;
    }

    /// Fill entire surface with a color (BGRA)
    pub fn fill(&mut self, b: u8, g: u8, r: u8, a: u8) {
        for chunk in self.buffer_mut().chunks_exact_mut(4) {
            chunk[0] = b;
            chunk[1] = g;
            chunk[2] = r;
            chunk[3] = a;
        }
        self.dirty = true;
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        let _ = munmap(self.buffer_ptr as usize, self.buffer_len);
    }
}
