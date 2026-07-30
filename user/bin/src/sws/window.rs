//! Window management module

use std::handle::capability::memory_mapping::{flags as mmap_flags, munmap};
use std::ipc::{SharedMemory, permissions};
use std::string::String;
use std::vec::Vec;
use std::{print, println};
use sws_protocol;

/// Window ID type
pub type WindowId = u32;

/// Per-window size constraints.
///
/// All values are in pixels.
/// - `0` means "unset".
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowSizeLimits {
    pub min_width: u32,
    pub min_height: u32,
    pub max_width: u32,
    pub max_height: u32,
}

impl WindowSizeLimits {
    pub fn clamp(&self, width: u32, height: u32) -> (u32, u32) {
        let mut w = width.max(1);
        let mut h = height.max(1);

        if self.min_width != 0 {
            w = w.max(self.min_width.max(1));
        }
        if self.min_height != 0 {
            h = h.max(self.min_height.max(1));
        }

        let effective_max_width = if self.max_width == 0 {
            0
        } else if self.min_width != 0 {
            self.max_width.max(self.min_width.max(1))
        } else {
            self.max_width.max(1)
        };
        let effective_max_height = if self.max_height == 0 {
            0
        } else if self.min_height != 0 {
            self.max_height.max(self.min_height.max(1))
        } else {
            self.max_height.max(1)
        };

        if effective_max_width != 0 {
            w = w.min(effective_max_width);
        }
        if effective_max_height != 0 {
            h = h.min(effective_max_height);
        }

        (w, h)
    }
}

/// Window type for Z-order management
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowType {
    /// Normal application window
    Normal,
    /// Always stays on top of normal windows
    AlwaysOnTop,
    /// Taskbar window (stays above desktop, below normal)
    Taskbar,
    /// Desktop background window (bottom layer)
    Desktop,
    /// Input-method-owned popup surface.
    ImePopup,
}

impl Default for WindowType {
    fn default() -> Self {
        WindowType::Normal
    }
}

/// Window properties
#[derive(Debug)]
pub struct Window {
    pub id: WindowId,
    /// Application identifier (e.g., "org.scarlet-os.desktop.settings")
    pub app_id: Option<Vec<u8>>,
    /// Optional logical parent window (transient relationship).
    ///
    /// When set, the compositor may keep this window stacked above its parent and
    /// move it together during interactive operations.
    pub parent: Option<WindowId>,
    /// Transient behavior flags (bitset).
    pub transient_flags: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub size_limits: WindowSizeLimits,
    pub title: Option<Vec<u8>>,
    pub visible: bool,
    pub focused: bool,
    /// Window contents buffer (BGRA format, 4 bytes per pixel)
    /// This is used for test/legacy windows.
    pub buffer: Option<Vec<u8>>,
    /// Shared memory object for buffer sharing with clients
    pub shm: Option<SharedMemory>,
    /// Mapped address of the shared memory (for server-side access)
    pub shm_mapped_addr: Option<usize>,
    /// Size of the SHM mapping in bytes (0 when not SHM-backed).
    pub shm_size: usize,
    /// Byte offset into the SHM where pixel data begins.
    pub shm_offset: usize,
    /// Bytes per row for SHM-backed windows.
    pub shm_stride: u32,
    /// Pixel format for SHM-backed windows (Wayland wl_shm format).
    pub shm_format: u32,
    /// Window type for Z-order management
    pub window_type: WindowType,
    /// Whether the window is minimized
    pub minimized: bool,
    /// Whether the window is maximized
    pub maximized: bool,
    /// Saved position and size before maximize (for restore)
    pub saved_geometry: Option<(i32, i32, u32, u32)>,
    /// Window opacity (0.0 = fully transparent, 1.0 = fully opaque)
    pub opacity: f32,
    /// Whether the window can be resized by the user via interactive resize
    pub resizable: bool,
    /// Whether focusing this window should make it the active application
    pub active_on_focus: bool,
    /// Whether the window content contains alpha channel (semi-transparent pixels)
    ///
    /// This is separate from window.opacity - this controls whether pixel alpha
    /// values in the window buffer should be respected during composition.
    ///
    /// - false: Window content is fully opaque, use fast copy path (default)
    /// - true: Window content has semi-transparent pixels, use alpha blending
    pub has_alpha_content: bool,
    /// Whether focusing this window should raise it in Z-order
    ///
    /// - true: Focusing raises the window to top of its layer (Normal, Taskbar, AlwaysOnTop)
    /// - false: Focusing does not change Z-order (Desktop, wallpapers)
    pub raise_on_focus: bool,
    /// Extension owner information for windows created by extension clients (e.g., wayland_bridge)
    ///
    /// Format: (extension_id, external_client_id)
    /// - extension_id: The client ID of the extension that owns this window
    /// - external_client_id: The external client ID assigned by the extension (e.g., Wayland surface ID)
    pub extension_owner: Option<(u32, u32)>,
}

/// Validated SharedMemory layout suitable for importing a window as BGRA texture backing.
pub struct WindowShmLayout<'a> {
    shared_memory: &'a SharedMemory,
    width: u32,
    height: u32,
    offset: usize,
    stride: u32,
    size: usize,
    format: u32,
}

impl<'a> WindowShmLayout<'a> {
    /// Return the SharedMemory owner for this layout.
    ///
    /// # Returns
    ///
    /// The live SharedMemory reference whose handle authorizes import.
    pub fn shared_memory(&self) -> &'a SharedMemory {
        self.shared_memory
    }

    /// Return the imported texture width in pixels.
    ///
    /// # Returns
    ///
    /// The validated width.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Return the imported texture height in pixels.
    ///
    /// # Returns
    ///
    /// The validated height.
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Return the byte offset of pixel `(0, 0)` in SharedMemory.
    ///
    /// # Returns
    ///
    /// The validated byte offset.
    pub const fn offset(&self) -> usize {
        self.offset
    }

    /// Return the number of bytes between SharedMemory pixel rows.
    ///
    /// # Returns
    ///
    /// The validated row stride.
    pub const fn stride(&self) -> u32 {
        self.stride
    }

    /// Return the size of the mapped SharedMemory backing in bytes.
    ///
    /// # Returns
    ///
    /// The recorded mapping size used to validate this layout.
    pub const fn size(&self) -> usize {
        self.size
    }

    /// Return the Wayland shared-memory pixel format.
    ///
    /// # Returns
    ///
    /// The validated BGRA-compatible format value.
    pub const fn format(&self) -> u32 {
        self.format
    }
}

/// Validated BGRA pixel view for a window backing store.
pub struct WindowPixels<'a> {
    pixels: &'a [u8],
    stride: u32,
    width: u32,
    height: u32,
}

impl<'a> WindowPixels<'a> {
    /// Return the complete validated backing slice, beginning at pixel `(0, 0)`.
    pub fn bytes(&self) -> &'a [u8] {
        self.pixels
    }

    /// Return the number of bytes between pixel rows.
    pub const fn stride(&self) -> u32 {
        self.stride
    }

    /// Return the validated backing width.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Return the validated backing height.
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Return a source slice beginning at the requested local BGRA rectangle.
    pub fn damage_bytes(
        &self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<&'a [u8], &'static str> {
        if width == 0
            || height == 0
            || x.checked_add(width).is_none_or(|right| right > self.width)
            || y.checked_add(height)
                .is_none_or(|bottom| bottom > self.height)
        {
            return Err("Window damage is outside its backing store");
        }

        let offset = (y as usize)
            .checked_mul(self.stride as usize)
            .and_then(|offset| offset.checked_add(x as usize * 4))
            .ok_or("Window damage offset overflow")?;
        let row_bytes = (width as usize)
            .checked_mul(4)
            .ok_or("Window damage row width overflow")?;
        let end = (height as usize - 1)
            .checked_mul(self.stride as usize)
            .and_then(|last_row_offset| last_row_offset.checked_add(row_bytes))
            .and_then(|damage_len| offset.checked_add(damage_len))
            .ok_or("Window damage size overflow")?;
        if end > self.pixels.len() {
            return Err("Window damage exceeds backing store");
        }
        Ok(&self.pixels[offset..end])
    }
}

#[allow(dead_code)]
impl Window {
    /// Return whether this window currently has a CPU-readable pixel backing.
    pub fn has_pixel_buffer(&self) -> bool {
        self.shm_mapped_addr.is_some() || self.buffer.is_some()
    }

    /// Return the validated CPU-readable BGRA backing store.
    ///
    /// # Returns
    ///
    /// A view beginning at local pixel `(0, 0)`, or an error when the backing
    /// store is unavailable or does not cover the window geometry.
    pub fn pixels(&self) -> Result<WindowPixels<'_>, &'static str> {
        let stride = if self.shm_mapped_addr.is_some() && self.shm_stride != 0 {
            self.shm_stride
        } else {
            self.width.checked_mul(4).ok_or("Window stride overflow")?
        };
        let row_bytes = self
            .width
            .checked_mul(4)
            .ok_or("Window row width overflow")?;
        if self.width == 0 || self.height == 0 || stride < row_bytes {
            return Err("Window backing stride is invalid");
        }
        let required = (self.height as usize - 1)
            .checked_mul(stride as usize)
            .and_then(|offset| offset.checked_add(row_bytes as usize))
            .ok_or("Window backing size overflow")?;

        if let Some(mapped_addr) = self.shm_mapped_addr {
            let mapping_size = self.shm_size;
            let end = self
                .shm_offset
                .checked_add(required)
                .ok_or("Window SHM range overflow")?;
            if mapping_size == 0 || end > mapping_size {
                return Err("Window SHM backing does not cover its geometry");
            }
            // SAFETY: `mapped_addr` is owned by this live window, and the checked
            // `shm_offset..end` range is entirely inside its recorded SHM mapping.
            let mapping =
                unsafe { core::slice::from_raw_parts(mapped_addr as *const u8, mapping_size) };
            return Ok(WindowPixels {
                pixels: &mapping[self.shm_offset..end],
                stride,
                width: self.width,
                height: self.height,
            });
        }

        if let Some(buffer) = self.buffer.as_deref() {
            if required > buffer.len() {
                return Err("Window buffer does not cover its geometry");
            }
            return Ok(WindowPixels {
                pixels: &buffer[..required],
                stride,
                width: self.width,
                height: self.height,
            });
        }

        Err("Window has no pixel buffer")
    }

    /// Return a validated BGRA SharedMemory layout for GPU texture import.
    ///
    /// # Returns
    ///
    /// A SharedMemory owner and fixed pixel layout, or an error when this window
    /// is not backed by BGRA-compatible SharedMemory covering its geometry.
    pub fn shm_layout(&self) -> Result<WindowShmLayout<'_>, &'static str> {
        const WL_SHM_FORMAT_ARGB8888: u32 = 0;

        let shared_memory = self
            .shm
            .as_ref()
            .ok_or("Window does not own shared memory")?;
        if self.width == 0
            || self.height == 0
            || self.shm_size == 0
            || self.shm_format != WL_SHM_FORMAT_ARGB8888
        {
            return Err("Window shared memory layout is not BGRA-compatible");
        }
        let row_bytes = self
            .width
            .checked_mul(4)
            .ok_or("Window shared memory row width overflow")?;
        if self.shm_stride < row_bytes {
            return Err("Window shared memory stride is too small");
        }
        let required = (self.height as usize - 1)
            .checked_mul(self.shm_stride as usize)
            .and_then(|offset| offset.checked_add(row_bytes as usize))
            .ok_or("Window shared memory layout overflows")?;
        let end = self
            .shm_offset
            .checked_add(required)
            .ok_or("Window shared memory layout overflows")?;
        if end > self.shm_size {
            return Err("Window shared memory layout exceeds its mapping");
        }
        Ok(WindowShmLayout {
            shared_memory,
            width: self.width,
            height: self.height,
            offset: self.shm_offset,
            stride: self.shm_stride,
            size: self.shm_size,
            format: self.shm_format,
        })
    }

    /// Create a new window
    pub fn new(id: WindowId, x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            id,
            app_id: None,
            parent: None,
            transient_flags: 0,
            x,
            y,
            width,
            height,
            size_limits: WindowSizeLimits::default(),
            title: None,
            visible: true,
            focused: false,
            buffer: None,
            shm: None,
            shm_mapped_addr: None,
            shm_size: 0,
            shm_offset: 0,
            shm_stride: 0,
            shm_format: 0,
            window_type: WindowType::default(),
            minimized: false,
            maximized: false,
            saved_geometry: None,
            opacity: 1.0,
            resizable: true, // Default to resizable
            active_on_focus: true,
            has_alpha_content: false, // Default to opaque content
            raise_on_focus: true,     // Default: Normal windows raise on focus
            extension_owner: None,
        }
    }

    /// Create window with internal buffer
    pub fn new_with_buffer(id: WindowId, x: i32, y: i32, width: u32, height: u32) -> Self {
        let buffer_size = (width * height * 4) as usize;
        let mut buffer = Vec::new();
        buffer.resize(buffer_size, 0);

        Self {
            id,
            app_id: None,
            parent: None,
            transient_flags: 0,
            x,
            y,
            width,
            height,
            size_limits: WindowSizeLimits::default(),
            title: None,
            visible: true,
            focused: false,
            buffer: Some(buffer),
            shm: None,
            shm_mapped_addr: None,
            shm_size: 0,
            shm_offset: 0,
            shm_stride: 0,
            shm_format: 0,
            window_type: WindowType::default(),
            minimized: false,
            maximized: false,
            saved_geometry: None,
            opacity: 1.0,
            resizable: true, // Default to resizable
            active_on_focus: true,
            has_alpha_content: false, // Default to opaque content
            raise_on_focus: true,     // Default: Normal windows raise on focus
            extension_owner: None,
        }
    }

    /// Create window with shared memory buffer (server allocates SHM)
    pub fn new_with_shm(
        id: WindowId,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<Self, &'static str> {
        let buffer_size = (width * height * 4) as usize;
        let shm = SharedMemory::create(buffer_size, permissions::READ_WRITE)
            .map_err(|_| "Failed to create shared memory")?;

        // Map SHM into server's address space so compositor can read it
        let mapper = shm
            .as_handle()
            .as_memory_mapping()
            .map_err(|_| "SharedMemory does not support mapping")?;
        let mapped_addr = mapper
            .mmap(
                0,
                buffer_size,
                permissions::READ_WRITE,
                mmap_flags::SHARED,
                0,
            )
            .map_err(|_| "Failed to mmap shared memory")?;

        println!(
            "[Window] Window #{} SHM created: size={} mapped_addr=0x{:x}",
            id, buffer_size, mapped_addr
        );

        Ok(Self {
            id,
            app_id: None,
            parent: None,
            transient_flags: 0,
            x,
            y,
            width,
            height,
            size_limits: WindowSizeLimits::default(),
            title: None,
            visible: true,
            focused: false,
            buffer: None,
            shm: Some(shm),
            shm_mapped_addr: Some(mapped_addr),
            shm_size: buffer_size,
            shm_offset: 0,
            shm_stride: width.saturating_mul(4),
            shm_format: 0,
            window_type: WindowType::default(),
            minimized: false,
            maximized: false,
            saved_geometry: None,
            opacity: 1.0,
            resizable: true, // Default to resizable
            active_on_focus: true,
            has_alpha_content: false, // Default to opaque content
            raise_on_focus: true,     // Default: Normal windows raise on focus
            extension_owner: None,
        })
    }

    /// Get buffer size in bytes
    pub fn buffer_size(&self) -> usize {
        if self.shm_size != 0 {
            self.shm_size
        } else {
            (self.width * self.height * 4) as usize
        }
    }

    /// Set window title
    pub fn set_title(&mut self, title: &str) {
        self.title = Some(title.as_bytes().to_vec());
    }

    /// Check if point is inside window bounds
    pub fn contains_point(&self, x: i32, y: i32) -> bool {
        x >= self.x
            && x < self.x + self.width as i32
            && y >= self.y
            && y < self.y + self.height as i32
    }

    /// Move window to new position
    pub fn move_to(&mut self, x: i32, y: i32) {
        self.x = x;
        self.y = y;
    }

    /// Resize window
    pub fn resize(&mut self, width: u32, height: u32) {
        let (w, h) = self.size_limits.clamp(width, height);
        self.width = w;
        self.height = h;
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        if let (Some(addr), size) = (self.shm_mapped_addr.take(), self.shm_size)
            && size != 0
        {
            let _ = munmap(addr, size);
        }
    }
}

/// Window manager - manages multiple windows with Z-order
pub struct WindowManager {
    windows: Vec<Window>,
    next_window_id: WindowId,
    focused_window: Option<WindowId>,
    workarea: Option<(i32, i32, u32, u32)>,
}

impl WindowManager {
    /// Create a new window manager
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
            next_window_id: 1,
            focused_window: None,
            workarea: None,
        }
    }

    /// Create a new window and add it to the manager
    pub fn create_window(&mut self, x: i32, y: i32, width: u32, height: u32) -> WindowId {
        let id = self.next_window_id;
        self.next_window_id += 1;

        println!(
            "[WindowManager] Creating window #{} at ({}, {}) with buffer",
            id, x, y
        );
        let window = Window::new_with_buffer(id, x, y, width, height);
        self.windows.push(window);

        // Focus the new window
        self.focus_window(id);

        id
    }

    /// Create a new window with a specified ID (used for IPC ID consistency)
    pub fn create_window_with_id(
        &mut self,
        id: WindowId,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> WindowId {
        if id >= self.next_window_id {
            self.next_window_id = id + 1;
        }

        println!(
            "[WindowManager] Creating window #{} at ({}, {}) with buffer (fixed id)",
            id, x, y
        );
        let window = Window::new_with_buffer(id, x, y, width, height);
        self.windows.push(window);

        // Focus the new window
        self.focus_window(id);

        id
    }

    /// Create a new window with shared memory buffer (server allocates, client maps)
    #[allow(dead_code)]
    pub fn create_window_with_shm(
        &mut self,
        id: WindowId,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<WindowId, &'static str> {
        if id >= self.next_window_id {
            self.next_window_id = id + 1;
        }

        println!(
            "[WindowManager] Creating SHM-backed window #{} at ({}, {}) {}x{}",
            id, x, y, width, height
        );
        let window = Window::new_with_shm(id, x, y, width, height)?;
        self.windows.push(window);

        Ok(id)
    }

    /// Create window from IPC event with pre-mapped SHM
    /// This takes ownership of the SharedMemory object passed from the IPC thread
    pub fn create_extension_window(
        &mut self,
        id: WindowId,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        shm: SharedMemory,
        shm_mapped_addr: Option<usize>,
        shm_size: usize,
        extension_id: u32,
        external_client_id: u32,
    ) -> Result<WindowId, &'static str> {
        if id >= self.next_window_id {
            self.next_window_id = id + 1;
        }

        println!(
            "[WindowManager] Creating extension window #{} from IPC event with SHM at 0x{:x?} (ext_id={}, ext_client_id={})",
            id, shm_mapped_addr, extension_id, external_client_id
        );

        let window = Window {
            id,
            app_id: None,
            parent: None,
            transient_flags: 0,
            x,
            y,
            width,
            height,
            size_limits: WindowSizeLimits::default(),
            title: None,
            visible: true,
            focused: false,
            buffer: None,
            shm: Some(shm),
            shm_mapped_addr,
            shm_size,
            shm_offset: 0,
            shm_stride: width.saturating_mul(4),
            shm_format: 0,
            window_type: WindowType::default(),
            minimized: false,
            maximized: false,
            saved_geometry: None,
            opacity: 1.0,
            resizable: true,
            active_on_focus: true,
            has_alpha_content: false,
            raise_on_focus: true,
            extension_owner: Some((extension_id, external_client_id)),
        };
        self.windows.push(window);

        Ok(id)
    }

    /// Create window from IPC event with pre-mapped SHM
    /// This takes ownership of the SharedMemory object passed from the IPC thread
    pub fn create_window_with_shm_from_event(
        &mut self,
        id: WindowId,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        shm: SharedMemory,
        shm_mapped_addr: Option<usize>,
        shm_size: usize,
    ) -> Result<WindowId, &'static str> {
        if id >= self.next_window_id {
            self.next_window_id = id + 1;
        }

        println!(
            "[WindowManager] Creating window #{} from IPC event with SHM at 0x{:x?}",
            id, shm_mapped_addr
        );

        let window = Window {
            id,
            app_id: None, // Will be set from IPC CREATE_WINDOW message
            parent: None,
            transient_flags: 0,
            x,
            y,
            width,
            height,
            size_limits: WindowSizeLimits::default(),
            title: None, // No title from IPC yet
            visible: true,
            focused: false, // Will be focused via focus_window below
            buffer: None,   // No Vec buffer
            shm: Some(shm),
            shm_mapped_addr,
            shm_size,
            shm_offset: 0,
            shm_stride: width.saturating_mul(4),
            shm_format: 0,
            window_type: WindowType::default(),
            minimized: false,
            maximized: false,
            saved_geometry: None,
            opacity: 1.0,
            resizable: true, // Default to resizable
            active_on_focus: true,
            has_alpha_content: false, // Default to opaque content
            raise_on_focus: true,     // Default: Normal windows raise on focus
            extension_owner: None,
        };
        self.windows.push(window);

        Ok(id)
    }

    /// Replace the SHM backing store for an existing window.
    pub fn replace_window_shm_from_event(
        &mut self,
        id: WindowId,
        width: u32,
        height: u32,
        offset: i32,
        stride: i32,
        format: u32,
        shm: SharedMemory,
        shm_mapped_addr: Option<usize>,
        shm_size: usize,
    ) -> Result<(), &'static str> {
        let window = self
            .windows
            .iter_mut()
            .find(|window| window.id == id)
            .ok_or("Window not found")?;

        if let (Some(old_addr), old_size) = (window.shm_mapped_addr.take(), window.shm_size)
            && old_size != 0
        {
            let _ = munmap(old_addr, old_size);
        }
        window.width = width;
        window.height = height;
        window.buffer = None;
        window.shm = Some(shm);
        window.shm_mapped_addr = shm_mapped_addr;
        window.shm_size = shm_size;
        window.shm_offset = offset.max(0) as usize;
        window.shm_stride = if stride > 0 {
            stride as u32
        } else {
            width.saturating_mul(4)
        };
        window.shm_format = format;
        window.has_alpha_content = format == 0;
        Ok(())
    }

    /// Create window without buffer (for testing)
    #[allow(dead_code)]
    pub fn create_window_no_buffer(&mut self, x: i32, y: i32, width: u32, height: u32) -> WindowId {
        let id = self.next_window_id;
        self.next_window_id += 1;

        println!(
            "[WindowManager] Creating window #{} at ({}, {}) without buffer",
            id, x, y
        );
        let window = Window::new(id, x, y, width, height);
        self.windows.push(window);

        // Focus the new window
        self.focus_window(id);

        id
    }

    /// Get window by ID
    pub fn get_window(&self, id: WindowId) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    /// Get mutable window by ID
    pub fn get_window_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    /// Find window at point (top-most)
    pub fn window_at_point(&self, x: i32, y: i32) -> Option<WindowId> {
        // Iterate in reverse order (top to bottom)
        self.windows
            .iter()
            .rev()
            .find(|w| w.visible && w.contains_point(x, y))
            .map(|w| w.id)
    }

    /// Focus a window
    /// If the window is hidden/minimized, it will be shown before focusing
    pub fn focus_window(&mut self, id: WindowId) {
        println!("[WindowManager] Focusing window #{}", id);
        // Unfocus all windows
        for window in &mut self.windows {
            window.focused = false;
        }

        // Focus the specified window
        if let Some(window) = self.get_window_mut(id) {
            // Show the window if it's hidden/minimized
            if window.minimized || !window.visible {
                window.minimized = false;
                window.visible = true;
                println!(
                    "[WindowManager] Window #{} shown (was minimized/hidden)",
                    id
                );
            }
            window.focused = true;
            self.focused_window = Some(id);
        }
    }

    /// Set focus to a window (alias for focus_window)
    pub fn set_focus(&mut self, id: WindowId) {
        self.focus_window(id);
    }

    /// Check if a window type should accept keyboard focus
    /// Desktop windows can accept focus (to receive events) but won't raise (raise_on_focus=false)
    pub fn window_type_accepts_focus(window_type: WindowType) -> bool {
        match window_type {
            WindowType::Normal => true,
            WindowType::AlwaysOnTop => true,
            WindowType::Taskbar => true,
            WindowType::Desktop => true, // Desktop can now accept focus (for events), but won't raise
            WindowType::ImePopup => false,
        }
    }

    /// Check if a window should accept focus
    pub fn window_accepts_focus(&self, id: WindowId) -> bool {
        if let Some(window) = self.get_window(id) {
            Self::window_type_accepts_focus(window.window_type)
        } else {
            false
        }
    }

    /// Raise window to top (bring to front in Z-order)
    pub fn raise_to_top(&mut self, id: WindowId) {
        let root = self.top_level_ancestor(id);
        println!(
            "[WindowManager] Raising window #{} (root #{}) to top",
            id, root
        );

        // Raise the entire transient group (root + all descendants) together.
        let mut group_ids: Vec<WindowId> = Vec::new();
        self.collect_descendants_for_raise(root, &mut group_ids);
        group_ids.insert(0, root);

        // Snapshot and rebuild Z-order.
        let old = core::mem::take(&mut self.windows);
        let mut remaining: Vec<Window> = Vec::new();
        let mut group: Vec<Window> = Vec::new();

        for w in old {
            if group_ids.iter().any(|gid| *gid == w.id) {
                group.push(w);
            } else {
                remaining.push(w);
            }
        }

        // Ensure root is below its descendants within the group.
        if let Some(pos) = group.iter().position(|w| w.id == root) {
            let root_w = group.remove(pos);
            group.insert(0, root_w);
        }

        self.windows = remaining;
        self.windows.extend(group);

        // Print current Z-order
        print!("[WindowManager] Current Z-order (bottom to top): ");
        for w in &self.windows {
            print!("#{}({:?}) ", w.id, w.window_type);
        }
        println!();
    }

    /// Internal helper: return the top-level ancestor for a window (follows parent links).
    fn top_level_ancestor(&self, mut id: WindowId) -> WindowId {
        // Follow parents until none or broken link.
        // This intentionally tolerates inconsistent states.
        for _ in 0..32 {
            let parent = self.get_window(id).and_then(|w| w.parent);
            match parent {
                Some(p) if p != id => id = p,
                _ => break,
            }
        }
        id
    }

    /// Internal helper: collect all descendants of `id` into `out`.
    fn collect_descendants_follow_move(&self, id: WindowId, out: &mut Vec<WindowId>) {
        for w in &self.windows {
            if w.parent == Some(id) {
                if (w.transient_flags & sws_protocol::transient_flags::FOLLOW_PARENT_MOVE) != 0 {
                    out.push(w.id);
                    self.collect_descendants_follow_move(w.id, out);
                }
            }
        }
    }

    fn collect_descendants_for_raise(&self, id: WindowId, out: &mut Vec<WindowId>) {
        for w in &self.windows {
            if w.parent == Some(id) {
                if (w.transient_flags & sws_protocol::transient_flags::RAISE_WITH_PARENT) != 0 {
                    out.push(w.id);
                    self.collect_descendants_for_raise(w.id, out);
                }
            }
        }
    }

    /// Set (or clear) a window parent.
    ///
    /// Returns `false` if the relationship would create a cycle or references missing windows.
    pub fn set_window_parent(&mut self, window_id: WindowId, parent_id: Option<WindowId>) -> bool {
        if self.get_window(window_id).is_none() {
            return false;
        }

        if let Some(pid) = parent_id {
            if pid == 0 || pid == window_id {
                return false;
            }
            if self.get_window(pid).is_none() {
                return false;
            }

            // Cycle check: ensure `pid` is not a descendant of `window_id`.
            let mut cur = Some(pid);
            for _ in 0..32 {
                match cur {
                    Some(x) if x == window_id => return false,
                    Some(x) => cur = self.get_window(x).and_then(|w| w.parent),
                    None => break,
                }
            }
        }

        if let Some(w) = self.get_window_mut(window_id) {
            w.parent = parent_id;
            // Default transient policy when a parent is set (Apple-like):
            // keep stacking with parent, but do not automatically follow moves.
            if w.parent.is_some() {
                w.transient_flags = sws_protocol::transient_flags::RAISE_WITH_PARENT;
            } else {
                w.transient_flags = 0;
            }
            true
        } else {
            false
        }
    }

    pub fn set_window_transient_flags(&mut self, window_id: WindowId, flags: u32) -> bool {
        if let Some(w) = self.get_window_mut(window_id) {
            w.transient_flags = flags;
            true
        } else {
            false
        }
    }

    pub fn set_window_size_limits(
        &mut self,
        window_id: WindowId,
        min_width: u32,
        min_height: u32,
        max_width: u32,
        max_height: u32,
    ) -> bool {
        if let Some(w) = self.get_window_mut(window_id) {
            w.size_limits = WindowSizeLimits {
                min_width,
                min_height,
                max_width,
                max_height,
            };
            true
        } else {
            false
        }
    }

    pub fn clamp_size_for_window(
        &self,
        window_id: WindowId,
        width: u32,
        height: u32,
    ) -> (u32, u32) {
        match self.get_window(window_id) {
            Some(w) => w.size_limits.clamp(width, height),
            None => (width.max(1), height.max(1)),
        }
    }

    /// Get focused window ID
    pub fn get_focused_window_id(&self) -> Option<WindowId> {
        self.focused_window
    }

    /// Get focused window reference
    #[allow(dead_code)]
    pub fn get_focused_window(&self) -> Option<&Window> {
        if let Some(focused_id) = self.focused_window {
            self.get_window(focused_id)
        } else {
            None
        }
    }

    /// Get all windows in Z-order (bottom to top)
    pub fn get_windows(&self) -> &[Window] {
        &self.windows
    }

    /// Close window
    pub fn close_window(&mut self, id: WindowId) {
        if let Some(index) = self.windows.iter().position(|w| w.id == id) {
            self.windows.remove(index);

            // Update focus if closed window was focused
            if self.focused_window == Some(id) {
                self.focused_window = self.windows.last().map(|w| w.id);
                if let Some(new_focus) = self.focused_window {
                    if let Some(window) = self.get_window_mut(new_focus) {
                        window.focused = true;
                    }
                }
            }
        }
    }

    /// Move window by delta
    #[allow(dead_code)]
    pub fn move_window(&mut self, id: WindowId, dx: i32, dy: i32) {
        if let Some(window) = self.get_window_mut(id) {
            window.x += dx;
            window.y += dy;
        }

        if dx != 0 || dy != 0 {
            let mut descendants: Vec<WindowId> = Vec::new();
            self.collect_descendants_follow_move(id, &mut descendants);
            for cid in descendants {
                if let Some(w) = self.get_window_mut(cid) {
                    w.x += dx;
                    w.y += dy;
                }
            }
        }
    }

    /// Set window position (absolute)
    pub fn set_window_position(&mut self, id: WindowId, x: i32, y: i32) {
        // Compute delta from current position, then apply to descendants as well.
        let (dx, dy) = match self.get_window(id) {
            Some(w) => (x - w.x, y - w.y),
            None => return,
        };

        if let Some(window) = self.get_window_mut(id) {
            window.x = x;
            window.y = y;
        }

        if dx != 0 || dy != 0 {
            let mut descendants: Vec<WindowId> = Vec::new();
            self.collect_descendants_follow_move(id, &mut descendants);
            for cid in descendants {
                if let Some(w) = self.get_window_mut(cid) {
                    w.x += dx;
                    w.y += dy;
                }
            }
        }
    }

    pub fn resize_window_with_shm(
        &mut self,
        id: WindowId,
        width: u32,
        height: u32,
        shm: SharedMemory,
        shm_mapped_addr: Option<usize>,
        shm_size: usize,
    ) -> bool {
        if let Some(w) = self.get_window_mut(id) {
            if let (Some(old_addr), old_size) = (w.shm_mapped_addr.take(), w.shm_size)
                && old_size != 0
            {
                let _ = munmap(old_addr, old_size);
            }

            // IMPORTANT: Do not clamp here. The SHM buffer was already allocated
            // for the provided width/height by the IPC thread.
            w.width = width.max(1);
            w.height = height.max(1);
            w.buffer = None;
            w.shm = Some(shm);
            w.shm_mapped_addr = shm_mapped_addr;
            w.shm_size = shm_size;
            w.shm_offset = 0;
            w.shm_stride = width.saturating_mul(4);
            w.shm_format = 0;
            true
        } else {
            false
        }
    }

    /// Resize a window in-place (compositor-side only).
    ///
    /// Updates the window's internal dimensions without replacing the buffer.
    /// The client is expected to handle the corresponding WINDOW_CONFIGURE
    /// message and provide a new buffer via ResizeWindow.
    pub fn resize_window_in_place(&mut self, id: WindowId, width: u32, height: u32) -> bool {
        if let Some(w) = self.get_window_mut(id) {
            w.width = width.max(1);
            w.height = height.max(1);
            true
        } else {
            false
        }
    }

    /// Minimize a window (hide from display but keep in window list)
    pub fn minimize_window(&mut self, id: WindowId) -> bool {
        if let Some(w) = self.get_window_mut(id) {
            w.minimized = true;
            w.visible = false;
            println!("[WindowManager] Window #{} minimized", id);
            true
        } else {
            false
        }
    }

    /// Maximize a window to screen dimensions
    pub fn maximize_window(&mut self, id: WindowId, screen_width: u32, screen_height: u32) -> bool {
        if let Some(w) = self.get_window_mut(id) {
            // Policy: windows with an explicit max size are not maximizable.
            // (max_* != 0 means "set")
            if w.size_limits.max_width != 0 || w.size_limits.max_height != 0 {
                println!(
                    "[WindowManager] Window #{} is not maximizable (max size limits set)",
                    id
                );
                return false;
            }
            if !w.maximized {
                // Save current geometry for restore
                w.saved_geometry = Some((w.x, w.y, w.width, w.height));
                w.x = 0;
                w.y = 0;
                w.width = screen_width;
                w.height = screen_height;
                w.maximized = true;
                println!(
                    "[WindowManager] Window #{} maximized to {}x{}",
                    id, screen_width, screen_height
                );
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    /// Restore a window from minimized or maximized state
    pub fn restore_window(&mut self, id: WindowId) -> bool {
        if let Some(w) = self.get_window_mut(id) {
            if w.minimized {
                w.minimized = false;
                w.visible = true;
                println!("[WindowManager] Window #{} restored from minimized", id);
                return true;
            }
            if w.maximized {
                if let Some((x, y, width, height)) = w.saved_geometry {
                    w.x = x;
                    w.y = y;
                    w.width = width;
                    w.height = height;
                    w.maximized = false;
                    w.saved_geometry = None;
                    println!("[WindowManager] Window #{} restored from maximized", id);
                    return true;
                }
            }
            false
        } else {
            false
        }
    }

    /// Check if a window is minimized
    pub fn is_minimized(&self, id: WindowId) -> bool {
        self.get_window(id).map(|w| w.minimized).unwrap_or(false)
    }

    /// Set window type for Z-order management
    pub fn set_window_type(&mut self, id: WindowId, window_type: WindowType) -> bool {
        if let Some(w) = self.get_window_mut(id) {
            w.window_type = window_type;
            // Set default resizable behavior based on window type
            // Taskbar and Desktop windows should not be resizable by default
            match window_type {
                WindowType::Taskbar | WindowType::Desktop | WindowType::ImePopup => {
                    w.resizable = false;
                }
                WindowType::Normal | WindowType::AlwaysOnTop => {
                    w.resizable = true;
                }
            }
            // Set raise_on_focus behavior based on window type
            // Desktop windows should NOT raise when focused (they stay in background)
            match window_type {
                WindowType::Desktop => {
                    w.raise_on_focus = false;
                }
                WindowType::Normal | WindowType::Taskbar | WindowType::AlwaysOnTop => {
                    w.raise_on_focus = true;
                }
                WindowType::ImePopup => {
                    w.raise_on_focus = false;
                    w.visible = false;
                }
            }
            println!(
                "[WindowManager] Window #{} type set to {:?}, resizable={}, raise_on_focus={}",
                id, window_type, w.resizable, w.raise_on_focus
            );

            // Rebuild Z-order to maintain proper layering: Desktop < Normal < Taskbar < AlwaysOnTop
            self.rebuild_z_order_for_type_change(id, window_type);

            true
        } else {
            false
        }
    }

    /// Internal helper: rebuild Z-order after window type change
    /// This ensures windows are in the correct layer: Desktop < Normal < Taskbar < AlwaysOnTop
    fn rebuild_z_order_for_type_change(&mut self, _id: WindowId, _new_type: WindowType) {
        let old = core::mem::take(&mut self.windows);
        let mut desktop: Vec<Window> = Vec::new();
        let mut normal: Vec<Window> = Vec::new();
        let mut taskbar: Vec<Window> = Vec::new();
        let mut always_on_top: Vec<Window> = Vec::new();
        let mut ime_popup: Vec<Window> = Vec::new();

        // Sort windows into layers by type
        for w in old {
            match w.window_type {
                WindowType::Desktop => desktop.push(w),
                WindowType::Normal => normal.push(w),
                WindowType::Taskbar => taskbar.push(w),
                WindowType::AlwaysOnTop => always_on_top.push(w),
                WindowType::ImePopup => ime_popup.push(w),
            }
        }

        // Reconstruct in proper Z-order: desktop -> normal -> taskbar -> always_on_top -> ime_popup
        self.windows = desktop;
        self.windows.extend(normal);
        self.windows.extend(taskbar);
        self.windows.extend(always_on_top);
        self.windows.extend(ime_popup);

        println!("[WindowManager] Z-order rebuilt after type change");
        print!("[WindowManager] Current Z-order (bottom to top): ");
        for w in &self.windows {
            print!("#{}({:?}) ", w.id, w.window_type);
        }
        println!();
    }

    /// Set window opacity (0.0 = transparent, 1.0 = opaque)
    pub fn set_window_opacity(&mut self, id: WindowId, opacity: f32) -> bool {
        if let Some(w) = self.get_window_mut(id) {
            w.opacity = opacity.max(0.0).min(1.0);
            println!(
                "[WindowManager] Window #{} opacity set to {}",
                id, w.opacity
            );
            true
        } else {
            false
        }
    }

    /// Set whether a window can be resized by the user via interactive resize
    pub fn set_window_resizable(&mut self, id: WindowId, resizable: bool) -> bool {
        if let Some(w) = self.get_window_mut(id) {
            w.resizable = resizable;
            println!(
                "[WindowManager] Window #{} resizable set to {}",
                id, resizable
            );
            true
        } else {
            false
        }
    }

    /// Set whether window content contains alpha channel (semi-transparent pixels)
    ///
    /// This is separate from window.opacity - this controls whether pixel alpha
    /// values in the window buffer should be respected during composition.
    pub fn set_window_has_alpha_content(&mut self, id: WindowId, has_alpha: bool) -> bool {
        if let Some(w) = self.get_window_mut(id) {
            w.has_alpha_content = has_alpha;
            println!(
                "[WindowManager] Window #{} has_alpha_content set to {}",
                id, has_alpha
            );
            true
        } else {
            false
        }
    }

    /// Raise window to top, respecting window types
    /// Desktop < Normal < Taskbar < AlwaysOnTop
    pub fn raise_to_top_with_type(&mut self, id: WindowId) {
        let (window_type, raise_on_focus) = match self.get_window(id) {
            Some(w) => (w.window_type, w.raise_on_focus),
            None => return,
        };

        // If raise_on_focus is false, don't raise the window (e.g., Desktop backgrounds)
        if !raise_on_focus {
            println!(
                "[WindowManager] Window #{} has raise_on_focus=false, not raising",
                id
            );
            return;
        }

        let root = self.top_level_ancestor(id);
        println!(
            "[WindowManager] Raising window #{} (root #{}) with type {:?} to top",
            id, root, window_type
        );

        // Collect the transient group (root + descendants)
        let mut group_ids: Vec<WindowId> = Vec::new();
        self.collect_descendants_for_raise(root, &mut group_ids);
        group_ids.insert(0, root);

        // Rebuild Z-order respecting window types
        let old = core::mem::take(&mut self.windows);
        let mut desktop: Vec<Window> = Vec::new();
        let mut normal: Vec<Window> = Vec::new();
        let mut taskbar: Vec<Window> = Vec::new();
        let mut always_on_top: Vec<Window> = Vec::new();
        let mut ime_popup: Vec<Window> = Vec::new();
        let mut group: Vec<Window> = Vec::new();

        for w in old {
            if group_ids.iter().any(|gid| *gid == w.id) {
                group.push(w);
            } else {
                match w.window_type {
                    WindowType::Desktop => desktop.push(w),
                    WindowType::Normal => normal.push(w),
                    WindowType::Taskbar => taskbar.push(w),
                    WindowType::AlwaysOnTop => always_on_top.push(w),
                    WindowType::ImePopup => ime_popup.push(w),
                }
            }
        }

        // Ensure root is below its descendants within the group
        if let Some(pos) = group.iter().position(|w| w.id == root) {
            let root_w = group.remove(pos);
            group.insert(0, root_w);
        }

        // Reconstruct in proper Z-order: desktop -> normal -> taskbar -> always_on_top
        // Each window type is constrained to its own layer
        match window_type {
            WindowType::Desktop => {
                // Desktop: put group at BOTTOM of Desktop layer (below all other Desktop windows)
                // This ensures focused Desktop stays below all Normal windows
                self.windows.extend(group);
                self.windows.extend(desktop);
                self.windows.extend(normal);
                self.windows.extend(taskbar);
                self.windows.extend(always_on_top);
                self.windows.extend(ime_popup);
            }
            WindowType::Normal => {
                // Normal: put desktop, then normal, then group at top of Normal layer
                self.windows = desktop;
                self.windows.extend(normal);
                self.windows.extend(group);
                self.windows.extend(taskbar);
                self.windows.extend(always_on_top);
                self.windows.extend(ime_popup);
            }
            WindowType::Taskbar => {
                // Taskbar: desktop, normal, taskbar, group at top of Taskbar layer
                self.windows = desktop;
                self.windows.extend(normal);
                self.windows.extend(taskbar);
                self.windows.extend(group);
                self.windows.extend(always_on_top);
                self.windows.extend(ime_popup);
            }
            WindowType::AlwaysOnTop => {
                // AlwaysOnTop: desktop, normal, taskbar, always_on_top, group, ime_popup
                self.windows = desktop;
                self.windows.extend(normal);
                self.windows.extend(taskbar);
                self.windows.extend(always_on_top);
                self.windows.extend(group);
                self.windows.extend(ime_popup);
            }
            WindowType::ImePopup => {
                // IME popup: always above application and shell UI.
                self.windows = desktop;
                self.windows.extend(normal);
                self.windows.extend(taskbar);
                self.windows.extend(always_on_top);
                self.windows.extend(ime_popup);
                self.windows.extend(group);
            }
        }

        // Print current Z-order
        print!("[WindowManager] Current Z-order (bottom to top): ");
        for w in &self.windows {
            print!("#{}({:?}) ", w.id, w.window_type);
        }
        println!();
    }

    /// Set workarea for window positioning
    pub fn set_workarea(&mut self, x: i32, y: i32, width: u32, height: u32) {
        self.workarea = Some((x, y, width, height));
        println!(
            "[WindowManager] Workarea set: x={}, y={}, width={}, height={}",
            x, y, width, height
        );
    }

    /// Calculate default position for a window within workarea
    pub fn calculate_default_position(&self, width: u32, height: u32) -> (i32, i32) {
        match self.workarea {
            Some((wx, wy, ww, wh)) => {
                // Place window at workarea origin without padding
                let x = wx;
                let y = wy;
                println!(
                    "[WindowManager] Calculated default position: ({}, {}) within workarea",
                    x, y
                );
                (x, y)
            }
            None => {
                // Fallback to screen center
                println!("[WindowManager] No workarea, using default position (100, 100)");
                (100, 100)
            }
        }
    }

    /// Get the first window ID (for sending messages to the first client)
    pub fn get_first_window_id(&self) -> Option<u32> {
        self.windows.first().map(|w| w.id)
    }

    /// Get window list for menu bar display
    /// Returns vector of (window_id, app_id, title, window_type, visible, focused, minimized)
    pub fn get_window_list(&self) -> Vec<(u32, String, String, u32, bool, bool, bool)> {
        let mut result = Vec::new();
        for w in &self.windows {
            // Skip taskbar/desktop windows from the list
            if matches!(
                w.window_type,
                WindowType::Taskbar | WindowType::Desktop | WindowType::ImePopup
            ) {
                continue;
            }

            let app_id = w
                .app_id
                .as_ref()
                .and_then(|bytes| core::str::from_utf8(bytes).ok())
                .unwrap_or("");
            let app_id = String::from(app_id);

            let title = w
                .title
                .as_ref()
                .and_then(|bytes| core::str::from_utf8(bytes).ok())
                .unwrap_or("Untitled");
            let title = String::from(title);

            let window_type = match w.window_type {
                WindowType::Normal => 0,
                WindowType::AlwaysOnTop => 1,
                WindowType::Taskbar => 2,
                WindowType::Desktop => 3,
                WindowType::ImePopup => 4,
            };

            result.push((
                w.id,
                app_id,
                title,
                window_type,
                w.visible,
                w.focused,
                w.minimized,
            ));
        }
        result
    }
}
