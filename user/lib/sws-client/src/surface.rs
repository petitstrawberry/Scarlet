//! Surface (window buffer) management

use crate::Error;
use scarlet_std::ipc::SharedMemory;
use scarlet_std::handle::capability::memory_mapping::flags as mmap_flags;

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
    buffer: &'static mut [u8],
    /// Whether the surface has uncommitted changes
    dirty: bool,
}

impl Surface {
    /// Create a new surface from server-provided resources
    pub(crate) fn new(
        id: u32,
        width: u32,
        height: u32,
        shm: SharedMemory,
    ) -> Result<Self, Error> {
        let (buffer, _addr) = Self::map_shm(&shm, width, height)?;

        Ok(Self {
            id,
            width,
            height,
            shm,
            buffer,
            dirty: false,
        })
    }

    fn map_shm(shm: &SharedMemory, width: u32, height: u32) -> Result<(&'static mut [u8], usize), Error> {
        let buffer_size = (width * height * 4) as usize;

        let addr = shm
            .as_handle()
            .as_memory_mapping()
            .map_err(|_| Error::ShmMapFailed)?
            .mmap(
                0,
                buffer_size,
                scarlet_std::ipc::permissions::READ_WRITE,
                mmap_flags::SHARED,
                0,
            )
            .map_err(|_| Error::ShmMapFailed)?;

        let buffer = unsafe { core::slice::from_raw_parts_mut(addr as *mut u8, buffer_size) };
        Ok((buffer, addr))
    }

    pub(crate) fn remap(&mut self, width: u32, height: u32, shm: SharedMemory) -> Result<(), Error> {
        let (buffer, _addr) = Self::map_shm(&shm, width, height)?;
        self.width = width;
        self.height = height;
        self.shm = shm;
        self.buffer = buffer;
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
        self.buffer
    }

    /// Get mutable access to the pixel buffer
    ///
    /// This marks the surface as dirty, requiring a commit.
    #[inline]
    pub fn buffer_mut(&mut self) -> &mut [u8] {
        self.dirty = true;
        self.buffer
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
        f(self.buffer, self.width, self.height)
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

    /// Get pixel at (x, y) as BGRA
    pub fn get_pixel(&self, x: u32, y: u32) -> Option<(u8, u8, u8, u8)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = ((y * self.width + x) * 4) as usize;
        Some((
            self.buffer[offset],     // B
            self.buffer[offset + 1], // G
            self.buffer[offset + 2], // R
            self.buffer[offset + 3], // A
        ))
    }

    /// Set pixel at (x, y) to BGRA
    pub fn set_pixel(&mut self, x: u32, y: u32, b: u8, g: u8, r: u8, a: u8) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = ((y * self.width + x) * 4) as usize;
        self.buffer[offset] = b;
        self.buffer[offset + 1] = g;
        self.buffer[offset + 2] = r;
        self.buffer[offset + 3] = a;
        self.dirty = true;
    }

    /// Fill entire surface with a color (BGRA)
    pub fn fill(&mut self, b: u8, g: u8, r: u8, a: u8) {
        for chunk in self.buffer.chunks_exact_mut(4) {
            chunk[0] = b;
            chunk[1] = g;
            chunk[2] = r;
            chunk[3] = a;
        }
        self.dirty = true;
    }
}
