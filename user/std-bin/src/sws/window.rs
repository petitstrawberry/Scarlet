//! Window management module

use scarlet_os::handle::capability::memory_mapping::{flags as mmap_flags, munmap};
use scarlet_os::ipc::{SharedMemory, permissions};
use std::string::String;
use std::vec::Vec;
use std::{print, println};
use sws_protocol;
pub use sws_protocol::{WindowGeometryInsets, WindowSizeLimits};

macro_rules! window_debug {
    ($($arg:tt)*) => {
        if super::compositor::is_sws_debug_enabled() {
            std::println!($($arg)*);
        }
    };
}

/// Window ID type
pub type WindowId = u32;

pub(super) fn rounded_rect_contains_point(
    rect: (i32, i32, u32, u32),
    radius: u32,
    px: i32,
    py: i32,
) -> bool {
    let (x, y, width, height) = rect;
    if width == 0 || height == 0 {
        return false;
    }
    let right = x.saturating_add(width as i32);
    let bottom = y.saturating_add(height as i32);
    if px < x || px >= right || py < y || py >= bottom {
        return false;
    }

    let radius = radius.min(width / 2).min(height / 2);
    if radius == 0 {
        return true;
    }
    let radius_i32 = radius as i32;
    let inner_left = x.saturating_add(radius_i32);
    let inner_right = right.saturating_sub(radius_i32);
    let inner_top = y.saturating_add(radius_i32);
    let inner_bottom = bottom.saturating_sub(radius_i32);
    if (px >= inner_left && px < inner_right) || (py >= inner_top && py < inner_bottom) {
        return true;
    }

    let center_x = if px < inner_left {
        inner_left
    } else {
        inner_right.saturating_sub(1)
    };
    let center_y = if py < inner_top {
        inner_top
    } else {
        inner_bottom.saturating_sub(1)
    };
    let dx = i64::from(px.saturating_sub(center_x));
    let dy = i64::from(py.saturating_sub(center_y));
    dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy))
        <= i64::from(radius).saturating_mul(i64::from(radius))
}

pub(super) fn rounded_rect_row_span(
    rect: (i32, i32, u32, u32),
    radius: u32,
    row_y: i32,
) -> Option<(i32, i32)> {
    let (x, y, width, height) = rect;
    let right = x.saturating_add(width as i32);
    let bottom = y.saturating_add(height as i32);
    if width == 0 || height == 0 || row_y < y || row_y >= bottom {
        return None;
    }
    let radius = radius.min(width / 2).min(height / 2);
    if radius == 0
        || (row_y >= y.saturating_add(radius as i32)
            && row_y < bottom.saturating_sub(radius as i32))
    {
        return Some((x, right));
    }

    let mut left = x;
    while left < right && !rounded_rect_contains_point(rect, radius, left, row_y) {
        left = left.saturating_add(1);
    }
    let mut right_exclusive = right;
    while right_exclusive > left
        && !rounded_rect_contains_point(rect, radius, right_exclusive - 1, row_y)
    {
        right_exclusive -= 1;
    }
    (right_exclusive > left).then_some((left, right_exclusive))
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
    /// Shell-owned Overview/application background below app scenes.
    ShellBackground,
    /// Pointer-transparent shell decoration above app scenes.
    ShellChrome,
    /// Input-method-owned popup surface.
    ImePopup,
}

impl Default for WindowType {
    fn default() -> Self {
        WindowType::Normal
    }
}

const FOCUSED_NORMAL_WINDOW_MARGIN: u32 = 10;

/// Compositor-only visual rectangle used by Overview and shell transitions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationTransform {
    /// Destination X coordinate in output pixels.
    pub x: i32,
    /// Destination Y coordinate in output pixels.
    pub y: i32,
    /// Destination width in output pixels.
    pub width: u32,
    /// Destination height in output pixels.
    pub height: u32,
    /// Additional compositor opacity multiplier.
    pub opacity: f32,
}

/// Additional read-only projection of one retained window buffer.
///
/// A presentation instance lets Overview show the same client surface in a
/// workspace thumbnail and in the selected-workspace spread without creating a
/// fake window or asking the client for another buffer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationInstance {
    /// Destination transform for this projection.
    pub transform: PresentationTransform,
    /// Optional compositor-space clip rectangle.
    pub clip: Option<(i32, i32, u32, u32)>,
    /// Corner radius applied to [`Self::clip`].
    pub clip_radius: u32,
}

/// Compute the authoritative managed geometry for a maximized window.
///
/// Both atomic window creation and the compositor's runtime policy use this
/// helper so the first client buffer has the same extent as the maximized
/// surface that SWS accepts for SGFX registration.
pub(super) fn maximized_geometry_for(
    window_type: WindowType,
    workarea: Option<(i32, i32, u32, u32)>,
    screen_width: u32,
    screen_height: u32,
) -> (i32, i32, u32, u32) {
    if window_type == WindowType::Normal
        && let Some((work_x, work_y, work_width, work_height)) = workarea
    {
        let margin = FOCUSED_NORMAL_WINDOW_MARGIN;
        return (
            work_x.saturating_add(margin as i32),
            work_y.saturating_add(margin as i32),
            work_width.saturating_sub(margin.saturating_mul(2)).max(1),
            work_height.saturating_sub(margin.saturating_mul(2)).max(1),
        );
    }
    (0, 0, screen_width.max(1), screen_height.max(1))
}

/// Window properties
#[derive(Debug)]
pub struct Window {
    pub id: WindowId,
    /// SWS connection that owns this window.
    pub owner_client_id: Option<usize>,
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
    /// Width of the currently attached CPU/shared-image backing.
    ///
    /// This intentionally differs from `width` while a compositor configure
    /// is waiting for the client to attach its replacement buffer.
    backing_width: u32,
    /// Height of the currently attached CPU/shared-image backing.
    backing_height: u32,
    /// Insets between complete surface bounds and managed visible geometry.
    pub window_geometry_insets: WindowGeometryInsets,
    /// Recenter the managed rectangle when the client advertises it initially.
    pub center_on_first_geometry: bool,
    pub size_limits: WindowSizeLimits,
    pub title: Option<Vec<u8>>,
    pub visible: bool,
    /// Whether workspace presentation currently exposes this surface.
    ///
    /// This compositor-only gate is independent of client visibility and
    /// minimized state, so workspace switches never rewrite public window state.
    pub workspace_visible: bool,
    /// Optional compositor-only visual transform.
    pub presentation_transform: Option<PresentationTransform>,
    /// Additional compositor-only projections of the retained backing.
    pub presentation_instances: Vec<PresentationInstance>,
    /// Optional compositor-only clip for Overview workspace actors.
    pub presentation_clip: Option<(i32, i32, u32, u32)>,
    /// Corner radius applied to [`Self::presentation_clip`].
    pub presentation_clip_radius: u32,
    pub focused: bool,
    /// Window contents buffer (BGRA format, 4 bytes per pixel)
    /// This is used for test/legacy windows.
    pub buffer: Option<Vec<u8>>,
    /// Shared memory object for buffer sharing with clients
    pub shm: Option<SharedMemory>,
    /// Mapped address of the shared memory (for server-side access)
    pub shm_mapped_addr: Option<usize>,
    /// Whether this window, rather than an extension backing registry, owns the mapping.
    pub(super) shm_mapping_owned: bool,
    /// Size of the SHM mapping in bytes (0 when not SHM-backed).
    pub shm_size: usize,
    /// Byte offset into the SHM where pixel data begins.
    pub shm_offset: usize,
    /// Bytes per row for SHM-backed windows.
    pub shm_stride: u32,
    /// Pixel format for SHM-backed windows (Wayland wl_shm format).
    pub shm_format: u32,
    /// Extension-scoped buffer currently selected for this window.
    pub(super) external_buffer_id: Option<u32>,
    /// Exact retained use of [`Self::external_buffer_id`].
    pub(super) external_buffer_commit_serial: Option<u64>,
    /// Window type for Z-order management
    pub window_type: WindowType,
    /// Whether the window is minimized
    pub minimized: bool,
    /// Whether the window is maximized
    pub maximized: bool,
    /// Whether focused-windowing policy, rather than the application or user,
    /// placed this window in the maximized state.
    pub focused_mode_managed: bool,
    /// Whether tablet workspace layout currently owns this surface geometry.
    pub workspace_layout_managed: bool,
    /// Freeform geometry retained while tablet workspace layout is active.
    pub workspace_restore_geometry: Option<(i32, i32, u32, u32)>,
    /// Whether the client has successfully submitted at least one frame.
    ///
    /// Geometry-changing compositor policy is deferred until this becomes
    /// true so the client's initial CPU or SGFX buffer identity still matches
    /// the surface created by SWS.
    pub has_presented_frame: bool,
    /// Global presentation counter observed for the newest submitted frame.
    pub(super) last_frame_submission_counter: Option<u64>,
    /// Whether the retained buffer belongs to the current shell presentation.
    ///
    /// A shell background is withheld after a Home/Overview transition until
    /// its client commits a frame for that new presentation.
    pub presentation_content_ready: bool,
    /// An application-requested maximize waiting for the initial frame.
    pub pending_maximize: bool,
    /// Whether SWS negotiated constraints, decoration insets, and surface size
    /// before allocating this window's first buffer.
    pub initial_size_negotiated: bool,
    /// Saved position and size before maximize (for restore)
    pub saved_geometry: Option<(i32, i32, u32, u32)>,
    /// Whether the window currently occupies the complete output.
    pub fullscreen: bool,
    /// Geometry to restore when leaving fullscreen.
    pub fullscreen_restore_geometry: Option<(i32, i32, u32, u32)>,
    /// Window opacity (0.0 = fully transparent, 1.0 = fully opaque)
    pub opacity: f32,
    /// Whether the window can be resized by the user via interactive resize
    pub resizable: bool,
    /// Cursor requested by the client while the pointer is over this window.
    pub cursor_icon: sws_protocol::CursorIcon,
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
    /// Return the extent of the buffer most recently attached by the client.
    ///
    /// # Returns
    ///
    /// Physical backing width and height. These remain stable across managed
    /// geometry changes until the corresponding buffer resize is received.
    pub(super) const fn backing_extent(&self) -> (u32, u32) {
        (self.backing_width, self.backing_height)
    }

    /// Record a newly attached client-buffer extent.
    ///
    /// # Arguments
    ///
    /// * `width` - New physical backing width.
    /// * `height` - New physical backing height.
    pub(super) fn set_backing_extent(&mut self, width: u32, height: u32) {
        self.backing_width = width.max(1);
        self.backing_height = height.max(1);
    }

    /// Return whether this window currently has a CPU-readable pixel backing.
    pub fn has_pixel_buffer(&self) -> bool {
        self.shm_mapped_addr.is_some() || self.buffer.is_some()
    }

    /// Return the validated CPU-readable BGRA backing store.
    ///
    /// # Returns
    ///
    /// A view beginning at local pixel `(0, 0)`, or an error when the backing
    /// store is unavailable or does not cover the last attached buffer extent.
    pub fn pixels(&self) -> Result<WindowPixels<'_>, &'static str> {
        let (backing_width, backing_height) = self.backing_extent();
        let stride = if self.shm_mapped_addr.is_some() && self.shm_stride != 0 {
            self.shm_stride
        } else {
            backing_width
                .checked_mul(4)
                .ok_or("Window stride overflow")?
        };
        let row_bytes = backing_width
            .checked_mul(4)
            .ok_or("Window row width overflow")?;
        if backing_width == 0 || backing_height == 0 || stride < row_bytes {
            return Err("Window backing stride is invalid");
        }
        let required = (backing_height as usize - 1)
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
                return Err("Window SHM backing does not cover its buffer extent");
            }
            // SAFETY: owned mappings live with this window; borrowed extension
            // mappings live in the compositor registry until every selecting
            // window releases them. The checked `shm_offset..end` range is
            // entirely inside the recorded mapping in either case.
            let mapping =
                unsafe { core::slice::from_raw_parts(mapped_addr as *const u8, mapping_size) };
            return Ok(WindowPixels {
                pixels: &mapping[self.shm_offset..end],
                stride,
                width: backing_width,
                height: backing_height,
            });
        }

        if let Some(buffer) = self.buffer.as_deref() {
            if required > buffer.len() {
                return Err("Window buffer does not cover its buffer extent");
            }
            return Ok(WindowPixels {
                pixels: &buffer[..required],
                stride,
                width: backing_width,
                height: backing_height,
            });
        }

        Err("Window has no pixel buffer")
    }

    /// Return a validated BGRA SharedMemory layout for GPU texture import.
    ///
    /// # Returns
    ///
    /// A SharedMemory owner and fixed pixel layout, or an error when this window
    /// is not backed by BGRA-compatible SharedMemory covering its last attached
    /// buffer extent.
    pub fn shm_layout(&self) -> Result<WindowShmLayout<'_>, &'static str> {
        const WL_SHM_FORMAT_ARGB8888: u32 = 0;

        let (backing_width, backing_height) = self.backing_extent();
        let shared_memory = self
            .shm
            .as_ref()
            .ok_or("Window does not own shared memory")?;
        if backing_width == 0
            || backing_height == 0
            || self.shm_size == 0
            || self.shm_format != WL_SHM_FORMAT_ARGB8888
        {
            return Err("Window shared memory layout is not BGRA-compatible");
        }
        let row_bytes = backing_width
            .checked_mul(4)
            .ok_or("Window shared memory row width overflow")?;
        if self.shm_stride < row_bytes {
            return Err("Window shared memory stride is too small");
        }
        let required = (backing_height as usize - 1)
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
            width: backing_width,
            height: backing_height,
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
            owner_client_id: None,
            app_id: None,
            parent: None,
            transient_flags: 0,
            x,
            y,
            width,
            height,
            backing_width: width,
            backing_height: height,
            window_geometry_insets: WindowGeometryInsets::default(),
            center_on_first_geometry: false,
            size_limits: WindowSizeLimits::default(),
            title: None,
            visible: true,
            workspace_visible: true,
            presentation_transform: None,
            presentation_instances: Vec::new(),
            presentation_clip: None,
            presentation_clip_radius: 0,
            focused: false,
            buffer: None,
            shm: None,
            shm_mapped_addr: None,
            shm_mapping_owned: true,
            shm_size: 0,
            shm_offset: 0,
            shm_stride: 0,
            shm_format: 0,
            external_buffer_id: None,
            external_buffer_commit_serial: None,
            window_type: WindowType::default(),
            minimized: false,
            maximized: false,
            focused_mode_managed: false,
            workspace_layout_managed: false,
            workspace_restore_geometry: None,
            has_presented_frame: false,
            last_frame_submission_counter: None,
            presentation_content_ready: true,
            pending_maximize: false,
            initial_size_negotiated: false,
            saved_geometry: None,
            fullscreen: false,
            fullscreen_restore_geometry: None,
            opacity: 1.0,
            resizable: true, // Default to resizable
            cursor_icon: sws_protocol::CursorIcon::Arrow,
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
            owner_client_id: None,
            app_id: None,
            parent: None,
            transient_flags: 0,
            x,
            y,
            width,
            height,
            backing_width: width,
            backing_height: height,
            window_geometry_insets: WindowGeometryInsets::default(),
            center_on_first_geometry: false,
            size_limits: WindowSizeLimits::default(),
            title: None,
            visible: true,
            workspace_visible: true,
            presentation_transform: None,
            presentation_instances: Vec::new(),
            presentation_clip: None,
            presentation_clip_radius: 0,
            focused: false,
            buffer: Some(buffer),
            shm: None,
            shm_mapped_addr: None,
            shm_mapping_owned: true,
            shm_size: 0,
            shm_offset: 0,
            shm_stride: 0,
            shm_format: 0,
            external_buffer_id: None,
            external_buffer_commit_serial: None,
            window_type: WindowType::default(),
            minimized: false,
            maximized: false,
            focused_mode_managed: false,
            workspace_layout_managed: false,
            workspace_restore_geometry: None,
            has_presented_frame: false,
            last_frame_submission_counter: None,
            presentation_content_ready: true,
            pending_maximize: false,
            initial_size_negotiated: false,
            saved_geometry: None,
            fullscreen: false,
            fullscreen_restore_geometry: None,
            opacity: 1.0,
            resizable: true, // Default to resizable
            cursor_icon: sws_protocol::CursorIcon::Arrow,
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
            owner_client_id: None,
            app_id: None,
            parent: None,
            transient_flags: 0,
            x,
            y,
            width,
            height,
            backing_width: width,
            backing_height: height,
            window_geometry_insets: WindowGeometryInsets::default(),
            center_on_first_geometry: false,
            size_limits: WindowSizeLimits::default(),
            title: None,
            visible: true,
            workspace_visible: true,
            presentation_transform: None,
            presentation_instances: Vec::new(),
            presentation_clip: None,
            presentation_clip_radius: 0,
            focused: false,
            buffer: None,
            shm: Some(shm),
            shm_mapped_addr: Some(mapped_addr),
            shm_mapping_owned: true,
            shm_size: buffer_size,
            shm_offset: 0,
            shm_stride: width.saturating_mul(4),
            shm_format: 0,
            external_buffer_id: None,
            external_buffer_commit_serial: None,
            window_type: WindowType::default(),
            minimized: false,
            maximized: false,
            focused_mode_managed: false,
            workspace_layout_managed: false,
            workspace_restore_geometry: None,
            has_presented_frame: false,
            last_frame_submission_counter: None,
            presentation_content_ready: true,
            pending_maximize: false,
            initial_size_negotiated: false,
            saved_geometry: None,
            fullscreen: false,
            fullscreen_restore_geometry: None,
            opacity: 1.0,
            resizable: true, // Default to resizable
            cursor_icon: sws_protocol::CursorIcon::Arrow,
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

    /// Return whether the surface is logically eligible for presentation.
    ///
    /// This deliberately ignores retained-buffer freshness. A client must be
    /// resumed and receive a frame callback before it can submit the commit
    /// that opens that freshness gate.
    ///
    /// # Returns
    ///
    /// `true` when compositor policy currently exposes the surface.
    pub const fn is_logically_presented(&self) -> bool {
        self.visible && self.workspace_visible && !self.minimized
    }

    /// Return whether the surface participates in composition and hit testing.
    ///
    /// Logical visibility and retained-buffer freshness must both permit it.
    ///
    /// # Returns
    ///
    /// `true` when the retained surface may be sampled and receive pointer input.
    pub const fn is_presented(&self) -> bool {
        self.is_logically_presented() && self.presentation_content_ready
    }

    /// Return the compositor destination rectangle for this surface.
    pub const fn presentation_geometry(&self) -> (i32, i32, u32, u32) {
        match self.presentation_transform {
            Some(transform) => (transform.x, transform.y, transform.width, transform.height),
            None => self.surface_geometry(),
        }
    }

    /// Return opacity after applying a shell presentation transform.
    pub fn presentation_opacity(&self) -> f32 {
        self.presentation_transform
            .map_or(self.opacity, |transform| self.opacity * transform.opacity)
            .clamp(0.0, 1.0)
    }

    /// Test a point against the compositor destination rectangle.
    pub fn contains_presentation_point(&self, px: i32, py: i32) -> bool {
        if let Some(clip) = self.presentation_clip
            && !rounded_rect_contains_point(clip, self.presentation_clip_radius, px, py)
        {
            return false;
        }
        let (x, y, width, height) = self.presentation_geometry();
        px >= x
            && px < x.saturating_add(width as i32)
            && py >= y
            && py < y.saturating_add(height as i32)
    }

    /// Return the complete surface rectangle used for composition and damage.
    pub const fn surface_geometry(&self) -> (i32, i32, u32, u32) {
        (self.x, self.y, self.width, self.height)
    }

    /// Return the managed visible geometry in screen coordinates.
    pub fn window_geometry(&self) -> (i32, i32, u32, u32) {
        // Fullscreen is defined by the output-sized surface itself. Clients can
        // retain their normal-mode decoration insets while they rebuild an
        // undecorated frame, but those insets must not shrink hit testing or
        // pointer-lock bounds in fullscreen.
        if self.fullscreen {
            return self.surface_geometry();
        }
        let insets = self.window_geometry_insets;
        (
            self.x.saturating_add(insets.left as i32),
            self.y.saturating_add(insets.top as i32),
            self.width.saturating_sub(insets.horizontal()).max(1),
            self.height.saturating_sub(insets.vertical()).max(1),
        )
    }

    /// Convert managed geometry into the complete surface rectangle.
    pub fn surface_geometry_for_window_geometry(
        &self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> (i32, i32, u32, u32) {
        let insets = self.window_geometry_insets;
        (
            x.saturating_sub(insets.left as i32),
            y.saturating_sub(insets.top as i32),
            width.saturating_add(insets.horizontal()).max(1),
            height.saturating_add(insets.vertical()).max(1),
        )
    }

    /// Set visible geometry in surface-local coordinates while preserving its
    /// current screen-space origin.
    pub fn set_window_geometry(
        &mut self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<bool, &'static str> {
        if x < 0 || y < 0 || width == 0 || height == 0 {
            return Err("Window geometry must be non-empty and surface-local");
        }
        let left = x as u32;
        let top = y as u32;
        let right_edge = left
            .checked_add(width)
            .ok_or("Window geometry horizontal overflow")?;
        let bottom_edge = top
            .checked_add(height)
            .ok_or("Window geometry vertical overflow")?;
        if right_edge > self.width || bottom_edge > self.height {
            return Err("Window geometry exceeds surface bounds");
        }

        let next = WindowGeometryInsets {
            left,
            top,
            right: self.width - right_edge,
            bottom: self.height - bottom_edge,
        };
        let (next_x, next_y) = if self.fullscreen {
            // A configure response can carry the client's retained normal-mode
            // decoration geometry. Never let that asynchronous response move
            // the fullscreen surface away from the output origin.
            (self.x, self.y)
        } else {
            let (visible_x, visible_y, _, _) = self.window_geometry();
            (
                visible_x.saturating_sub(left as i32),
                visible_y.saturating_sub(top as i32),
            )
        };
        let changed = self.window_geometry_insets != next || self.x != next_x || self.y != next_y;
        self.window_geometry_insets = next;
        self.x = next_x;
        self.y = next_y;
        Ok(changed)
    }

    pub(super) fn reconcile_window_geometry_after_resize(&mut self) {
        let insets = self.window_geometry_insets;
        if insets.horizontal() < self.width && insets.vertical() < self.height {
            return;
        }
        let (visible_x, visible_y, _, _) = self.window_geometry();
        self.window_geometry_insets = WindowGeometryInsets::default();
        self.x = visible_x;
        self.y = visible_y;
    }

    /// Check if a point is inside the managed visible window geometry.
    pub fn contains_point(&self, x: i32, y: i32) -> bool {
        let (window_x, window_y, width, height) = self.window_geometry();
        x >= window_x
            && x < window_x.saturating_add(width as i32)
            && y >= window_y
            && y < window_y.saturating_add(height as i32)
    }

    /// Check if a point is inside the complete composited surface bounds.
    pub fn contains_surface_point(&self, x: i32, y: i32) -> bool {
        x >= self.x
            && x < self.x.saturating_add(self.width as i32)
            && y >= self.y
            && y < self.y.saturating_add(self.height as i32)
    }

    /// Return compositor-visible presentation-state flags.
    ///
    /// Fullscreen and maximized are independent flags. Both are reported while
    /// a maximized window temporarily occupies the complete output.
    ///
    /// # Returns
    ///
    /// A bitset from [`sws_protocol::window_state`].
    pub fn state_flags(&self) -> u32 {
        let mut flags = 0;
        if self.minimized {
            flags |= sws_protocol::window_state::MINIMIZED;
        }
        if self.fullscreen {
            flags |= sws_protocol::window_state::FULLSCREEN;
        }
        if self.maximized {
            flags |= sws_protocol::window_state::MAXIMIZED;
        }
        if !self.is_logically_presented() {
            flags |= sws_protocol::window_state::SUSPENDED;
        }
        flags
    }

    /// Return whether focused windowing may manage this as a full workarea window.
    ///
    /// Top-level normal windows must be resizable and must not advertise a
    /// fixed maximum size. Transients, panels, popups, and fixed-size
    /// compatibility windows remain floating.
    ///
    /// # Returns
    ///
    /// `true` when focused policy may maximize and later restore this window.
    pub fn supports_focused_windowing(&self) -> bool {
        self.window_type == WindowType::Normal
            && self.parent.is_none()
            && self.resizable
            && self.size_limits.max_width == 0
            && self.size_limits.max_height == 0
            && !self.fullscreen
    }

    /// Record that the client has submitted its first usable frame.
    ///
    /// # Returns
    ///
    /// `true` only for the transition from not-yet-presented to ready. Later
    /// submissions return `false`.
    pub fn mark_presented_frame(&mut self) -> bool {
        if self.has_presented_frame {
            false
        } else {
            self.has_presented_frame = true;
            true
        }
    }

    /// Record one submitted frame against the compositor presentation clock.
    ///
    /// # Arguments
    ///
    /// * `presentation_counter` - Global completed-presentation count observed
    ///   when this frame entered the compositor.
    ///
    /// # Returns
    ///
    /// `true` only when this is the window's first usable frame.
    pub(super) fn note_frame_submission(&mut self, presentation_counter: u64) -> bool {
        self.last_frame_submission_counter = Some(presentation_counter);
        self.mark_presented_frame()
    }

    /// Require a fresh commit before composing this shell background.
    ///
    /// # Returns
    ///
    /// `true` when the surface changed from ready to waiting.
    pub fn invalidate_presentation_content(&mut self) -> bool {
        if self.window_type != WindowType::ShellBackground || !self.presentation_content_ready {
            return false;
        }
        self.presentation_content_ready = false;
        true
    }

    /// Mark the retained buffer as valid for the current presentation.
    ///
    /// # Returns
    ///
    /// `true` when a pending shell-background refresh was completed.
    pub fn validate_presentation_content(&mut self) -> bool {
        if self.presentation_content_ready {
            return false;
        }
        self.presentation_content_ready = true;
        true
    }

    /// Return whether this is a top-level normal compatibility window.
    ///
    /// # Returns
    ///
    /// `true` for ordinary top-level windows that cannot participate in
    /// focused resizing and therefore must remain floating.
    pub fn is_focused_compatibility_window(&self) -> bool {
        self.window_type == WindowType::Normal
            && self.parent.is_none()
            && !self.supports_focused_windowing()
            && !self.fullscreen
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
        self.reconcile_window_geometry_after_resize();
    }

    pub(super) fn release_owned_shm_mapping(&mut self) {
        if self.shm_mapping_owned
            && let (Some(addr), size) = (self.shm_mapped_addr.take(), self.shm_size)
            && size != 0
        {
            let _ = munmap(addr, size);
        } else {
            self.shm_mapped_addr = None;
        }
        self.shm = None;
        self.shm_size = 0;
        self.shm_offset = 0;
        self.shm_stride = 0;
        self.shm_mapping_owned = true;
    }

    /// Select a non-owning view into an extension-managed SHM pool.
    ///
    /// # Arguments
    ///
    /// * `buffer_id` - Extension-scoped reusable buffer identifier.
    /// * `commit_serial` - Non-zero identity of this retained buffer use.
    /// * `width` - Buffer width in physical pixels.
    /// * `height` - Buffer height in physical pixels.
    /// * `offset` - Byte offset into the mapped pool.
    /// * `stride` - Bytes between adjacent rows.
    /// * `format` - Wayland SHM pixel format.
    /// * `mapped_addr` - Base address of the compositor-owned pool mapping.
    /// * `mapping_size` - Complete size of that mapping.
    ///
    /// # Returns
    ///
    /// The previously selected extension buffer and retained-use serial, or an
    /// error when the view cannot cover the declared extent.
    pub(super) fn select_external_shm_buffer(
        &mut self,
        buffer_id: u32,
        commit_serial: u64,
        width: u32,
        height: u32,
        offset: usize,
        stride: u32,
        format: u32,
        mapped_addr: usize,
        mapping_size: usize,
    ) -> Result<Option<(u32, u64)>, &'static str> {
        let row_bytes = width.checked_mul(4).ok_or("External row width overflow")?;
        if buffer_id == 0
            || commit_serial == 0
            || width == 0
            || height == 0
            || stride < row_bytes
            || mapped_addr == 0
            || mapping_size == 0
        {
            return Err("Invalid external SHM view");
        }
        let required = (height as usize - 1)
            .checked_mul(stride as usize)
            .and_then(|bytes| bytes.checked_add(row_bytes as usize))
            .ok_or("External SHM view overflow")?;
        let end = offset
            .checked_add(required)
            .ok_or("External SHM view overflow")?;
        if end > mapping_size {
            return Err("External SHM view exceeds its pool");
        }

        let previous = self
            .external_buffer_id
            .zip(self.external_buffer_commit_serial);
        self.release_owned_shm_mapping();
        self.buffer = None;
        self.width = width;
        self.height = height;
        self.set_backing_extent(width, height);
        self.reconcile_window_geometry_after_resize();
        self.shm_mapped_addr = Some(mapped_addr);
        self.shm_mapping_owned = false;
        self.shm_size = mapping_size;
        self.shm_offset = offset;
        self.shm_stride = stride;
        self.shm_format = format;
        self.external_buffer_id = Some(buffer_id);
        self.external_buffer_commit_serial = Some(commit_serial);
        self.has_alpha_content = format == 0;
        self.presentation_content_ready = true;
        Ok(previous)
    }

    /// Unmap the logical contents of an extension-managed surface.
    ///
    /// # Returns
    ///
    /// The previously selected external buffer and retained-use serial, if any.
    pub(super) fn detach_external_buffer(&mut self) -> Option<(u32, u64)> {
        let previous = self
            .external_buffer_id
            .take()
            .zip(self.external_buffer_commit_serial.take());
        self.release_owned_shm_mapping();
        self.buffer = None;
        self.presentation_content_ready = false;
        previous
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        if self.shm_mapping_owned
            && let (Some(addr), size) = (self.shm_mapped_addr.take(), self.shm_size)
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
        self.rebuild_z_order();

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
        self.rebuild_z_order();

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
        self.rebuild_z_order();

        Ok(id)
    }

    /// Create window from IPC event with pre-mapped SHM
    /// This takes ownership of the SharedMemory object passed from the IPC thread
    ///
    /// # Arguments
    ///
    /// * `id` - SWS window identifier assigned by the IPC layer.
    /// * `x` - Initial output-relative X coordinate.
    /// * `y` - Initial output-relative Y coordinate.
    /// * `width` - Initial width in physical pixels.
    /// * `height` - Initial height in physical pixels.
    /// * `shm` - Shared-memory object containing the initial backing buffer.
    /// * `shm_mapped_addr` - Optional server mapping of `shm`.
    /// * `shm_size` - Mapped shared-memory length in bytes.
    /// * `owner_client_id` - IPC connection authorized to manage the window.
    /// * `extension_id` - Registered extension that represents the external client.
    /// * `external_client_id` - Extension-local client or surface identifier.
    ///
    /// # Returns
    ///
    /// The inserted window identifier, or an error when creation fails.
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
        owner_client_id: usize,
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
            owner_client_id: Some(owner_client_id),
            app_id: None,
            parent: None,
            transient_flags: 0,
            x,
            y,
            width,
            height,
            backing_width: width,
            backing_height: height,
            window_geometry_insets: WindowGeometryInsets::default(),
            center_on_first_geometry: false,
            size_limits: WindowSizeLimits::default(),
            title: None,
            visible: true,
            workspace_visible: true,
            presentation_transform: None,
            presentation_instances: Vec::new(),
            presentation_clip: None,
            presentation_clip_radius: 0,
            focused: false,
            buffer: None,
            shm: Some(shm),
            shm_mapped_addr,
            shm_mapping_owned: true,
            shm_size,
            shm_offset: 0,
            shm_stride: width.saturating_mul(4),
            shm_format: 0,
            external_buffer_id: None,
            external_buffer_commit_serial: None,
            window_type: WindowType::default(),
            minimized: false,
            maximized: false,
            focused_mode_managed: false,
            workspace_layout_managed: false,
            workspace_restore_geometry: None,
            has_presented_frame: false,
            last_frame_submission_counter: None,
            presentation_content_ready: true,
            pending_maximize: false,
            initial_size_negotiated: false,
            saved_geometry: None,
            fullscreen: false,
            fullscreen_restore_geometry: None,
            opacity: 1.0,
            resizable: true,
            cursor_icon: sws_protocol::CursorIcon::Arrow,
            active_on_focus: true,
            has_alpha_content: false,
            raise_on_focus: true,
            extension_owner: Some((extension_id, external_client_id)),
        };
        self.windows.push(window);
        self.rebuild_z_order();

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
            owner_client_id: None,
            app_id: None, // Will be set from IPC CREATE_WINDOW message
            parent: None,
            transient_flags: 0,
            x,
            y,
            width,
            height,
            backing_width: width,
            backing_height: height,
            window_geometry_insets: WindowGeometryInsets::default(),
            center_on_first_geometry: false,
            size_limits: WindowSizeLimits::default(),
            title: None, // No title from IPC yet
            visible: true,
            workspace_visible: true,
            presentation_transform: None,
            presentation_instances: Vec::new(),
            presentation_clip: None,
            presentation_clip_radius: 0,
            focused: false, // Will be focused via focus_window below
            buffer: None,   // No Vec buffer
            shm: Some(shm),
            shm_mapped_addr,
            shm_mapping_owned: true,
            shm_size,
            shm_offset: 0,
            shm_stride: width.saturating_mul(4),
            shm_format: 0,
            external_buffer_id: None,
            external_buffer_commit_serial: None,
            window_type: WindowType::default(),
            minimized: false,
            maximized: false,
            focused_mode_managed: false,
            workspace_layout_managed: false,
            workspace_restore_geometry: None,
            has_presented_frame: false,
            last_frame_submission_counter: None,
            presentation_content_ready: true,
            pending_maximize: false,
            initial_size_negotiated: false,
            saved_geometry: None,
            fullscreen: false,
            fullscreen_restore_geometry: None,
            opacity: 1.0,
            resizable: true, // Default to resizable
            cursor_icon: sws_protocol::CursorIcon::Arrow,
            active_on_focus: true,
            has_alpha_content: false, // Default to opaque content
            raise_on_focus: true,     // Default: Normal windows raise on focus
            extension_owner: None,
        };
        self.windows.push(window);
        self.rebuild_z_order();

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

        window.release_owned_shm_mapping();
        window.width = width;
        window.height = height;
        window.set_backing_extent(width, height);
        window.reconcile_window_geometry_after_resize();
        window.buffer = None;
        window.shm = Some(shm);
        window.shm_mapped_addr = shm_mapped_addr;
        window.shm_mapping_owned = true;
        window.shm_size = shm_size;
        window.shm_offset = offset.max(0) as usize;
        window.shm_stride = if stride > 0 {
            stride as u32
        } else {
            width.saturating_mul(4)
        };
        window.shm_format = format;
        window.external_buffer_id = None;
        window.external_buffer_commit_serial = None;
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
        self.rebuild_z_order();

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
            .find(|w| {
                w.window_type != WindowType::ShellChrome
                    && w.is_presented()
                    && w.contains_presentation_point(x, y)
            })
            .map(|w| w.id)
    }

    /// Focus a window.
    ///
    /// If the window is hidden/minimized, it will be shown before focusing.
    /// While fullscreen is active, only that window and its transient
    /// descendants may take focus.
    ///
    /// # Arguments
    ///
    /// * `id` - Window that should receive keyboard focus.
    pub fn focus_window(&mut self, id: WindowId) {
        let system_shell_focus = self
            .get_window(id)
            .is_some_and(|window| window.window_type == WindowType::ShellBackground);
        if let Some(fullscreen_id) = self.fullscreen_window_id()
            && !self.is_in_fullscreen_group(id)
            && !system_shell_focus
        {
            println!(
                "[WindowManager] Window #{} cannot take focus from fullscreen window #{}",
                id, fullscreen_id
            );
            return;
        }

        if self.focused_window == Some(id)
            && self
                .get_window(id)
                .is_some_and(|window| window.focused && window.is_presented())
        {
            return;
        }

        window_debug!("[WindowManager] Focusing window #{}", id);
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

    /// Check if a window type should accept keyboard focus.
    ///
    /// Pointer events are routed independently from keyboard focus. The
    /// taskbar therefore remains interactive without replacing the focused
    /// application when a status control is pressed.
    pub fn window_type_accepts_focus(window_type: WindowType) -> bool {
        match window_type {
            WindowType::Normal => true,
            WindowType::AlwaysOnTop => true,
            WindowType::Taskbar => false,
            WindowType::Desktop => true, // Desktop can now accept focus (for events), but won't raise
            WindowType::ShellBackground => true,
            WindowType::ShellChrome => false,
            WindowType::ImePopup => false,
        }
    }

    /// Check whether a window may accept focus under the current policy.
    ///
    /// # Arguments
    ///
    /// * `id` - Window identifier to inspect.
    ///
    /// # Returns
    ///
    /// `true` when the role is focusable and fullscreen focus confinement, if
    /// active, permits the window.
    pub fn window_accepts_focus(&self, id: WindowId) -> bool {
        let Some(window) = self.get_window(id) else {
            return false;
        };
        if !Self::window_type_accepts_focus(window.window_type) {
            return false;
        }
        self.fullscreen_window_id().is_none() || self.is_in_fullscreen_group(id)
    }

    /// Raise a window and its transient group to the top.
    ///
    /// While fullscreen is active, this delegates to the layer-aware raise
    /// policy so unrelated windows cannot escape above fullscreen.
    ///
    /// # Arguments
    ///
    /// * `id` - Window or transient group member to raise.
    pub fn raise_to_top(&mut self, id: WindowId) {
        if self.fullscreen_window_id().is_some() {
            self.raise_to_top_with_type(id);
            return;
        }

        let root = self.top_level_ancestor(id);
        window_debug!(
            "[WindowManager] Raising window #{} (root #{}) to top",
            id,
            root
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
        if super::compositor::is_sws_debug_enabled() {
            print!("[WindowManager] Current Z-order (bottom to top): ");
            for w in &self.windows {
                print!("#{}({:?}) ", w.id, w.window_type);
            }
            println!();
        }
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
    /// # Arguments
    ///
    /// * `window_id` - Child window whose relationship should change.
    /// * `parent_id` - Parent window, or `None` to clear the relationship.
    ///
    /// # Returns
    ///
    /// `false` if the relationship would create a cycle or references missing
    /// windows; otherwise `true`.
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
            self.rebuild_z_order();
            true
        } else {
            false
        }
    }

    /// Set transient stacking and movement policy flags.
    ///
    /// # Arguments
    ///
    /// * `window_id` - Transient window to update.
    /// * `flags` - Bitset from [`sws_protocol::transient_flags`].
    ///
    /// # Returns
    ///
    /// `true` when the window exists and was updated.
    pub fn set_window_transient_flags(&mut self, window_id: WindowId, flags: u32) -> bool {
        if let Some(w) = self.get_window_mut(window_id) {
            w.transient_flags = flags;
            self.rebuild_z_order();
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
                self.focused_window =
                    self.windows
                        .iter()
                        .rev()
                        .map(|window| window.id)
                        .find(|candidate| {
                            self.window_accepts_focus(*candidate)
                                && self
                                    .get_window(*candidate)
                                    .is_some_and(Window::is_presented)
                        });
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

    /// Set the managed visible window position (absolute).
    pub fn set_window_position(&mut self, id: WindowId, x: i32, y: i32) {
        // Compute the corresponding surface origin and apply the same movement
        // delta to transient descendants.
        let (surface_x, surface_y, dx, dy) = match self.get_window(id) {
            Some(w) => {
                let surface_x = x.saturating_sub(w.window_geometry_insets.left as i32);
                let surface_y = y.saturating_sub(w.window_geometry_insets.top as i32);
                (surface_x, surface_y, surface_x - w.x, surface_y - w.y)
            }
            None => return,
        };

        if let Some(window) = self.get_window_mut(id) {
            window.x = surface_x;
            window.y = surface_y;
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

    /// Set managed visible geometry inside a complete window surface.
    pub fn set_window_geometry(
        &mut self,
        id: WindowId,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<bool, &'static str> {
        self.get_window_mut(id)
            .ok_or("Window not found")?
            .set_window_geometry(x, y, width, height)
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
            w.release_owned_shm_mapping();

            // IMPORTANT: Do not clamp here. The SHM buffer was already allocated
            // for the provided width/height by the IPC thread.
            w.width = width.max(1);
            w.height = height.max(1);
            w.set_backing_extent(width, height);
            w.reconcile_window_geometry_after_resize();
            w.buffer = None;
            w.shm = Some(shm);
            w.shm_mapped_addr = shm_mapped_addr;
            w.shm_mapping_owned = true;
            w.shm_size = shm_size;
            w.shm_offset = 0;
            w.shm_stride = width.saturating_mul(4);
            w.shm_format = 0;
            w.external_buffer_id = None;
            w.external_buffer_commit_serial = None;
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
            w.reconcile_window_geometry_after_resize();
            true
        } else {
            false
        }
    }

    /// Resize a surface so its managed window geometry has the requested size.
    ///
    /// # Arguments
    ///
    /// * `id` - Window identifier to resize.
    /// * `width` - Requested managed geometry width in physical pixels.
    /// * `height` - Requested managed geometry height in physical pixels.
    ///
    /// # Returns
    ///
    /// `true` when the window exists and its surface size was updated.
    pub fn resize_window_geometry_in_place(
        &mut self,
        id: WindowId,
        width: u32,
        height: u32,
    ) -> bool {
        let Some(window) = self.get_window_mut(id) else {
            return false;
        };
        let (_, _, surface_width, surface_height) = window.surface_geometry_for_window_geometry(
            window.x,
            window.y,
            width.max(1),
            height.max(1),
        );
        window.width = surface_width;
        window.height = surface_height;
        true
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

    /// Maximize a window to compositor-selected workarea dimensions.
    ///
    /// # Arguments
    ///
    /// * `id` - Window to maximize.
    /// * `screen_width` - Selected maximized width in physical pixels.
    /// * `screen_height` - Selected maximized height in physical pixels.
    ///
    /// # Returns
    ///
    /// `true` when the state changed to maximized.
    pub fn maximize_window(&mut self, id: WindowId, screen_width: u32, screen_height: u32) -> bool {
        if let Some(w) = self.get_window_mut(id) {
            if w.fullscreen {
                println!(
                    "[WindowManager] Ignoring maximize for fullscreen window #{}",
                    id
                );
                return false;
            }
            if !w.resizable {
                println!(
                    "[WindowManager] Window #{} is not maximizable (fixed-size window)",
                    id
                );
                return false;
            }
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
                let (x, y, surface_width, surface_height) =
                    w.surface_geometry_for_window_geometry(0, 0, screen_width, screen_height);
                let (width, height) = w.size_limits.clamp(surface_width, surface_height);
                w.x = x;
                w.y = y;
                w.width = width;
                w.height = height;
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

    /// Make a window occupy the complete output.
    ///
    /// The current geometry is saved independently from maximize state. If the
    /// window was maximized before entering fullscreen, leaving fullscreen
    /// restores the maximized geometry and keeps the normal restore geometry.
    ///
    /// # Arguments
    ///
    /// * `id` - Window to place in fullscreen.
    /// * `screen_width` - Output width in physical pixels.
    /// * `screen_height` - Output height in physical pixels.
    ///
    /// # Returns
    ///
    /// `true` when fullscreen was entered. Returns `false` for an unknown,
    /// minimized, already-fullscreen window, or while another window owns the
    /// single output's fullscreen state.
    pub fn set_fullscreen_window(
        &mut self,
        id: WindowId,
        screen_width: u32,
        screen_height: u32,
    ) -> bool {
        if self
            .windows
            .iter()
            .any(|window| window.fullscreen && window.id != id)
        {
            return false;
        }

        let Some(window) = self.get_window_mut(id) else {
            return false;
        };
        if window.fullscreen || window.minimized {
            return false;
        }

        window.fullscreen_restore_geometry =
            Some((window.x, window.y, window.width, window.height));
        // Unlike maximize, fullscreen sizes the complete client surface to the
        // output. Expanding the surface by normal-mode shadow outsets makes an
        // undecorated fullscreen frame larger than the output and clips its
        // right and bottom edges.
        window.x = 0;
        window.y = 0;
        window.width = screen_width.max(1);
        window.height = screen_height.max(1);
        window.fullscreen = true;
        println!(
            "[WindowManager] Window #{} entered fullscreen at {}x{}",
            id, window.width, window.height
        );

        self.rebuild_z_order();
        true
    }

    /// Resize the active fullscreen surface to the complete output.
    ///
    /// Normal-mode window geometry insets are deliberately ignored. They can
    /// remain cached for restoration, but must never enlarge or offset the
    /// fullscreen surface.
    ///
    /// # Arguments
    ///
    /// * `id` - Fullscreen window to resize.
    /// * `screen_width` - Updated output width in physical pixels.
    /// * `screen_height` - Updated output height in physical pixels.
    ///
    /// # Returns
    ///
    /// `true` when the window exists in fullscreen state, or `false` otherwise.
    pub fn resize_fullscreen_window(
        &mut self,
        id: WindowId,
        screen_width: u32,
        screen_height: u32,
    ) -> bool {
        let Some(window) = self.get_window_mut(id) else {
            return false;
        };
        if !window.fullscreen {
            return false;
        }

        window.x = 0;
        window.y = 0;
        window.width = screen_width.max(1);
        window.height = screen_height.max(1);
        true
    }

    /// Leave fullscreen and restore the preceding geometry.
    ///
    /// # Arguments
    ///
    /// * `id` - Fullscreen window to restore.
    ///
    /// # Returns
    ///
    /// `true` when fullscreen was left, or `false` when the window is unknown
    /// or not fullscreen.
    pub fn unset_fullscreen_window(&mut self, id: WindowId) -> bool {
        let Some(window) = self.get_window_mut(id) else {
            return false;
        };
        if !window.fullscreen {
            return false;
        }

        let Some((x, y, width, height)) = window.fullscreen_restore_geometry.take() else {
            return false;
        };
        window.x = x;
        window.y = y;
        window.width = width.max(1);
        window.height = height.max(1);
        window.fullscreen = false;
        println!("[WindowManager] Window #{} left fullscreen", id);

        self.rebuild_z_order();
        true
    }

    /// Check whether a window is currently fullscreen.
    ///
    /// # Arguments
    ///
    /// * `id` - Window identifier to inspect.
    ///
    /// # Returns
    ///
    /// `true` only when the window exists and owns fullscreen state.
    pub fn is_fullscreen(&self, id: WindowId) -> bool {
        self.get_window(id)
            .map(|window| window.fullscreen)
            .unwrap_or(false)
    }

    /// Check whether a window belongs to the active fullscreen transient group.
    ///
    /// # Arguments
    ///
    /// * `id` - Window identifier to inspect.
    ///
    /// # Returns
    ///
    /// `true` for the fullscreen window and each of its transient descendants.
    pub fn is_in_fullscreen_group(&self, id: WindowId) -> bool {
        self.fullscreen_window_id()
            .is_some_and(|fullscreen_id| self.top_level_ancestor(id) == fullscreen_id)
    }

    fn fullscreen_window_id(&self) -> Option<WindowId> {
        self.windows
            .iter()
            .find(|window| window.fullscreen)
            .map(|window| window.id)
    }

    /// Restore a window from minimized or maximized state.
    ///
    /// Fullscreen is independent and must be left with
    /// [`Self::unset_fullscreen_window`].
    ///
    /// # Arguments
    ///
    /// * `id` - Window to restore.
    ///
    /// # Returns
    ///
    /// `true` when minimized or maximized state changed.
    pub fn restore_window(&mut self, id: WindowId) -> bool {
        if let Some(w) = self.get_window_mut(id) {
            if w.minimized {
                w.minimized = false;
                w.visible = true;
                println!("[WindowManager] Window #{} restored from minimized", id);
                return true;
            }
            if w.fullscreen {
                return false;
            }
            if w.maximized {
                if let Some((x, y, width, height)) = w.saved_geometry {
                    w.x = x;
                    w.y = y;
                    w.width = width;
                    w.height = height;
                    w.maximized = false;
                    w.focused_mode_managed = false;
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

    /// Set a window role used for Z-order management.
    ///
    /// # Arguments
    ///
    /// * `id` - Window to update.
    /// * `window_type` - New desktop, normal, shell, overlay, or IME role.
    ///
    /// # Returns
    ///
    /// `true` when the window exists and was updated.
    pub fn set_window_type(&mut self, id: WindowId, window_type: WindowType) -> bool {
        if let Some(w) = self.get_window_mut(id) {
            w.window_type = window_type;
            // Set default resizable behavior based on window type
            // Taskbar and Desktop windows should not be resizable by default
            match window_type {
                WindowType::Taskbar
                | WindowType::Desktop
                | WindowType::ShellBackground
                | WindowType::ShellChrome
                | WindowType::ImePopup => {
                    w.resizable = false;
                }
                WindowType::Normal | WindowType::AlwaysOnTop => {
                    w.resizable = true;
                }
            }
            // Set raise_on_focus behavior based on window type
            // Desktop windows should NOT raise when focused (they stay in background)
            match window_type {
                WindowType::Desktop | WindowType::ShellBackground | WindowType::ShellChrome => {
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
            if window_type == WindowType::ShellBackground {
                // The role is assigned before the client's first real frame.
                // Keep any zero-filled or predecessor buffer out of the
                // composition until that commit arrives.
                w.presentation_content_ready = false;
            }
            println!(
                "[WindowManager] Window #{} type set to {:?}, resizable={}, raise_on_focus={}",
                id, window_type, w.resizable, w.raise_on_focus
            );

            // Rebuild Z-order to maintain type and fullscreen layers.
            self.rebuild_z_order();

            true
        } else {
            false
        }
    }

    /// Rebuild type and fullscreen layers while preserving order within each layer.
    fn rebuild_z_order(&mut self) {
        let fullscreen_id = self
            .windows
            .iter()
            .find(|window| window.fullscreen)
            .map(|window| window.id);
        let mut fullscreen_group_ids = Vec::new();
        if let Some(id) = fullscreen_id {
            fullscreen_group_ids.push(id);
            self.collect_descendants_for_raise(id, &mut fullscreen_group_ids);
        }

        let old = core::mem::take(&mut self.windows);
        let mut desktop: Vec<Window> = Vec::new();
        let mut shell_background: Vec<Window> = Vec::new();
        let mut normal: Vec<Window> = Vec::new();
        let mut shell_chrome: Vec<Window> = Vec::new();
        let mut taskbar: Vec<Window> = Vec::new();
        let mut always_on_top: Vec<Window> = Vec::new();
        let mut fullscreen: Vec<Window> = Vec::new();
        let mut ime_popup: Vec<Window> = Vec::new();

        // Fullscreen is a presentation state rather than a window role. It
        // covers shell and always-on-top surfaces but remains below IME UI.
        for w in old {
            if fullscreen_group_ids.iter().any(|id| *id == w.id) {
                fullscreen.push(w);
                continue;
            }
            match w.window_type {
                WindowType::Desktop => desktop.push(w),
                WindowType::ShellBackground => shell_background.push(w),
                WindowType::Normal => normal.push(w),
                WindowType::ShellChrome => shell_chrome.push(w),
                WindowType::Taskbar => taskbar.push(w),
                WindowType::AlwaysOnTop => always_on_top.push(w),
                WindowType::ImePopup => ime_popup.push(w),
            }
        }

        if let Some(id) = fullscreen_id
            && let Some(position) = fullscreen.iter().position(|window| window.id == id)
        {
            let root = fullscreen.remove(position);
            fullscreen.insert(0, root);
        }

        // Reconstruct bottom-to-top layer order.
        self.windows = desktop;
        self.windows.extend(shell_background);
        self.windows.extend(normal);
        self.windows.extend(shell_chrome);
        self.windows.extend(taskbar);
        self.windows.extend(always_on_top);
        self.windows.extend(fullscreen);
        self.windows.extend(ime_popup);

        if super::compositor::is_sws_debug_enabled() {
            println!("[WindowManager] Z-order rebuilt");
            print!("[WindowManager] Current Z-order (bottom to top): ");
            for w in &self.windows {
                print!("#{}({:?}) ", w.id, w.window_type);
            }
            println!();
        }
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

    /// Raise a window while respecting role and fullscreen layers.
    ///
    /// The base order is Desktop < Normal < Taskbar < AlwaysOnTop < Fullscreen
    /// < ImePopup. Fullscreen transient descendants remain with their parent.
    ///
    /// # Arguments
    ///
    /// * `id` - Window or transient group member to raise.
    pub fn raise_to_top_with_type(&mut self, id: WindowId) {
        let (window_type, raise_on_focus) = match self.get_window(id) {
            Some(w) => (w.window_type, w.raise_on_focus),
            None => return,
        };

        if let Some(fullscreen_id) = self
            .windows
            .iter()
            .find(|window| window.fullscreen)
            .map(|window| window.id)
        {
            if self.top_level_ancestor(id) == fullscreen_id {
                self.rebuild_z_order();
            } else {
                println!(
                    "[WindowManager] Window #{} cannot raise above fullscreen window #{}",
                    id, fullscreen_id
                );
            }
            return;
        }

        // If raise_on_focus is false, don't raise the window (e.g., Desktop backgrounds)
        if !raise_on_focus {
            println!(
                "[WindowManager] Window #{} has raise_on_focus=false, not raising",
                id
            );
            return;
        }

        let root = self.top_level_ancestor(id);
        window_debug!(
            "[WindowManager] Raising window #{} (root #{}) with type {:?} to top",
            id,
            root,
            window_type
        );

        // Collect the transient group (root + descendants)
        let mut group_ids: Vec<WindowId> = Vec::new();
        self.collect_descendants_for_raise(root, &mut group_ids);
        group_ids.insert(0, root);

        // Rebuild Z-order respecting window types
        let old = core::mem::take(&mut self.windows);
        let mut desktop: Vec<Window> = Vec::new();
        let mut shell_background: Vec<Window> = Vec::new();
        let mut normal: Vec<Window> = Vec::new();
        let mut shell_chrome: Vec<Window> = Vec::new();
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
                    WindowType::ShellBackground => shell_background.push(w),
                    WindowType::Normal => normal.push(w),
                    WindowType::ShellChrome => shell_chrome.push(w),
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
                self.windows.extend(shell_background);
                self.windows.extend(normal);
                self.windows.extend(shell_chrome);
                self.windows.extend(taskbar);
                self.windows.extend(always_on_top);
                self.windows.extend(ime_popup);
            }
            WindowType::Normal => {
                // Normal: put desktop, then normal, then group at top of Normal layer
                self.windows = desktop;
                self.windows.extend(shell_background);
                self.windows.extend(normal);
                self.windows.extend(group);
                self.windows.extend(shell_chrome);
                self.windows.extend(taskbar);
                self.windows.extend(always_on_top);
                self.windows.extend(ime_popup);
            }
            WindowType::Taskbar => {
                // Taskbar: desktop, normal, taskbar, group at top of Taskbar layer
                self.windows = desktop;
                self.windows.extend(shell_background);
                self.windows.extend(normal);
                self.windows.extend(shell_chrome);
                self.windows.extend(taskbar);
                self.windows.extend(group);
                self.windows.extend(always_on_top);
                self.windows.extend(ime_popup);
            }
            WindowType::AlwaysOnTop => {
                // AlwaysOnTop: desktop, normal, taskbar, always_on_top, group, ime_popup
                self.windows = desktop;
                self.windows.extend(shell_background);
                self.windows.extend(normal);
                self.windows.extend(shell_chrome);
                self.windows.extend(taskbar);
                self.windows.extend(always_on_top);
                self.windows.extend(group);
                self.windows.extend(ime_popup);
            }
            WindowType::ImePopup => {
                // IME popup: always above application and shell UI.
                self.windows = desktop;
                self.windows.extend(shell_background);
                self.windows.extend(normal);
                self.windows.extend(shell_chrome);
                self.windows.extend(taskbar);
                self.windows.extend(always_on_top);
                self.windows.extend(ime_popup);
                self.windows.extend(group);
            }
            WindowType::ShellBackground => {
                self.windows = desktop;
                self.windows.extend(shell_background);
                self.windows.extend(group);
                self.windows.extend(normal);
                self.windows.extend(shell_chrome);
                self.windows.extend(taskbar);
                self.windows.extend(always_on_top);
                self.windows.extend(ime_popup);
            }
            WindowType::ShellChrome => {
                self.windows = desktop;
                self.windows.extend(shell_background);
                self.windows.extend(normal);
                self.windows.extend(shell_chrome);
                self.windows.extend(group);
                self.windows.extend(taskbar);
                self.windows.extend(always_on_top);
                self.windows.extend(ime_popup);
            }
        }

        // Print current Z-order
        if super::compositor::is_sws_debug_enabled() {
            print!("[WindowManager] Current Z-order (bottom to top): ");
            for w in &self.windows {
                print!("#{}({:?}) ", w.id, w.window_type);
            }
            println!();
        }
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
                WindowType::Taskbar
                    | WindowType::Desktop
                    | WindowType::ShellBackground
                    | WindowType::ShellChrome
                    | WindowType::ImePopup
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
                WindowType::ShellBackground => 5,
                WindowType::ShellChrome => 6,
            };

            result.push((
                w.id,
                app_id,
                title,
                window_type,
                w.is_presented(),
                w.focused,
                w.minimized,
            ));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::{WindowManager, WindowType, rounded_rect_contains_point, rounded_rect_row_span};

    #[test]
    fn taskbar_routes_pointer_input_without_taking_keyboard_focus() {
        assert!(!WindowManager::window_type_accepts_focus(
            WindowType::Taskbar
        ));
        assert!(WindowManager::window_type_accepts_focus(
            WindowType::AlwaysOnTop
        ));
    }

    #[test]
    fn closing_popup_restores_focus_past_non_focusable_taskbar() {
        let mut manager = WindowManager::new();
        let app = manager.create_window_no_buffer(0, 32, 800, 600);
        let taskbar = manager.create_window_no_buffer(0, 0, 800, 32);
        let popup = manager.create_window_no_buffer(600, 32, 200, 300);
        assert!(manager.set_window_type(taskbar, WindowType::Taskbar));
        assert!(manager.set_window_type(popup, WindowType::AlwaysOnTop));
        manager.set_focus(popup);

        manager.close_window(popup);

        assert_eq!(manager.get_focused_window_id(), Some(app));
        assert!(manager.get_window(app).is_some_and(|window| window.focused));
        assert!(
            !manager
                .get_window(taskbar)
                .is_some_and(|window| window.focused)
        );
    }

    #[test]
    fn rounded_overview_clip_rejects_only_card_corners() {
        let rect = (10, 20, 100, 60);
        assert!(!rounded_rect_contains_point(rect, 12, 10, 20));
        assert!(!rounded_rect_contains_point(rect, 12, 109, 20));
        assert!(rounded_rect_contains_point(rect, 12, 22, 20));
        assert!(rounded_rect_contains_point(rect, 12, 10, 32));
        assert!(rounded_rect_contains_point(rect, 12, 60, 50));
        assert_eq!(rounded_rect_row_span(rect, 12, 20), Some((22, 98)));
        assert_eq!(rounded_rect_row_span(rect, 12, 50), Some((10, 110)));
    }

    #[test]
    fn client_geometry_preserves_visible_origin_and_excludes_shadow_hit_area() {
        let mut manager = WindowManager::new();
        let id = manager.create_window_no_buffer(100, 80, 324, 260);

        assert!(manager.set_window_geometry(id, 10, 6, 304, 240).unwrap());
        let window = manager.get_window(id).unwrap();
        assert_eq!(window.surface_geometry(), (90, 74, 324, 260));
        assert_eq!(window.window_geometry(), (100, 80, 304, 240));
        assert!(window.contains_surface_point(91, 75));
        assert!(!window.contains_point(91, 75));
        assert!(window.contains_point(100, 80));
    }

    #[test]
    fn managed_position_moves_the_visible_geometry_not_the_shadow_surface() {
        let mut manager = WindowManager::new();
        let id = manager.create_window_no_buffer(100, 80, 324, 260);
        manager.set_window_geometry(id, 10, 6, 304, 240).unwrap();

        manager.set_window_position(id, 200, 150);

        let window = manager.get_window(id).unwrap();
        assert_eq!(window.surface_geometry(), (190, 144, 324, 260));
        assert_eq!(window.window_geometry(), (200, 150, 304, 240));
    }

    #[test]
    fn maximize_sizes_managed_geometry_to_workarea_and_keeps_shadow_outside() {
        let mut manager = WindowManager::new();
        let id = manager.create_window_no_buffer(100, 80, 324, 260);
        manager.set_window_geometry(id, 10, 6, 304, 240).unwrap();

        assert!(manager.maximize_window(id, 1000, 700));
        manager.set_window_position(id, 10, 20);

        let window = manager.get_window(id).unwrap();
        assert_eq!(window.window_geometry(), (10, 20, 1000, 700));
        assert_eq!(window.surface_geometry(), (0, 14, 1020, 720));
    }

    #[test]
    fn asymmetric_shadow_outsets_do_not_shift_maximized_visible_geometry() {
        let mut manager = WindowManager::new();
        let id = manager.create_window_no_buffer(100, 80, 344, 280);
        manager.set_window_geometry(id, 10, 6, 304, 240).unwrap();

        assert!(manager.maximize_window(id, 1000, 700));
        manager.set_window_position(id, 10, 42);

        let window = manager.get_window(id).unwrap();
        assert_eq!(window.window_geometry(), (10, 42, 1000, 700));
        assert_eq!(window.surface_geometry(), (0, 36, 1040, 740));
    }

    #[test]
    fn maximize_never_violates_minimum_surface_constraints() {
        let mut manager = WindowManager::new();
        let id = manager.create_window_no_buffer(100, 80, 324, 260);
        manager.set_window_geometry(id, 10, 6, 304, 240).unwrap();
        assert!(manager.set_window_size_limits(id, 1100, 800, 0, 0));

        assert!(manager.maximize_window(id, 1000, 700));
        manager.set_window_position(id, 10, 42);

        let window = manager.get_window(id).unwrap();
        assert_eq!((window.width, window.height), (1100, 800));
        assert_eq!(window.window_geometry(), (10, 42, 1080, 780));
    }

    #[test]
    fn first_presented_frame_opens_the_geometry_policy_gate_once() {
        let mut manager = WindowManager::new();
        let id = manager.create_window_no_buffer(100, 80, 324, 260);
        let window = manager.get_window_mut(id).unwrap();

        assert!(!window.has_presented_frame);
        assert!(!window.initial_size_negotiated);
        assert!(window.mark_presented_frame());
        assert!(window.has_presented_frame);
        assert!(!window.mark_presented_frame());
    }

    #[test]
    fn non_presented_window_reports_suspended_state() {
        let mut manager = WindowManager::new();
        let id = manager.create_window_no_buffer(100, 80, 324, 260);
        let window = manager.get_window_mut(id).unwrap();

        assert_eq!(window.state_flags(), 0);
        window.workspace_visible = false;
        assert_eq!(window.state_flags(), sws_protocol::window_state::SUSPENDED);
        window.minimized = true;
        assert_eq!(
            window.state_flags(),
            sws_protocol::window_state::MINIMIZED | sws_protocol::window_state::SUSPENDED
        );
    }

    #[test]
    fn stale_shell_content_is_not_composed_but_remains_frame_eligible() {
        let mut manager = WindowManager::new();
        let id = manager.create_window_no_buffer(0, 0, 1920, 1080);
        assert!(manager.set_window_type(id, WindowType::ShellBackground));
        let window = manager.get_window_mut(id).unwrap();

        assert!(window.is_logically_presented());
        assert!(!window.is_presented());
        assert_eq!(window.state_flags(), 0);
        assert!(window.validate_presentation_content());
        assert!(window.is_presented());
        assert!(window.invalidate_presentation_content());
        assert!(!window.is_presented());
        assert!(!window.invalidate_presentation_content());
    }

    #[test]
    fn focused_windowing_accepts_only_resizable_top_level_normal_windows() {
        let mut manager = WindowManager::new();
        let parent_id = manager.create_window_no_buffer(20, 20, 640, 480);
        assert!(
            manager
                .get_window(parent_id)
                .unwrap()
                .supports_focused_windowing()
        );

        let fixed_id = manager.create_window_no_buffer(40, 40, 320, 200);
        assert!(manager.set_window_resizable(fixed_id, false));
        let fixed = manager.get_window(fixed_id).unwrap();
        assert!(!fixed.supports_focused_windowing());
        assert!(fixed.is_focused_compatibility_window());
        assert!(!manager.maximize_window(fixed_id, 1000, 700));

        let transient_id = manager.create_window_no_buffer(60, 60, 240, 160);
        assert!(manager.set_window_parent(transient_id, Some(parent_id)));
        let transient = manager.get_window(transient_id).unwrap();
        assert!(!transient.supports_focused_windowing());
        assert!(!transient.is_focused_compatibility_window());
    }

    #[test]
    fn explicit_maximum_size_keeps_window_in_compatibility_floating_class() {
        let mut manager = WindowManager::new();
        let id = manager.create_window_no_buffer(10, 10, 480, 320);
        assert!(manager.set_window_size_limits(id, 320, 240, 640, 480));

        let window = manager.get_window(id).unwrap();
        assert!(!window.supports_focused_windowing());
        assert!(window.is_focused_compatibility_window());
        assert!(!manager.maximize_window(id, 1000, 700));
    }

    #[test]
    fn fullscreen_round_trip_restores_normal_geometry() {
        let mut manager = WindowManager::new();
        let id = manager.create_window_no_buffer(25, 30, 640, 480);

        assert!(manager.set_fullscreen_window(id, 1920, 1080));
        let fullscreen = manager.get_window(id).unwrap();
        assert_eq!(
            (
                fullscreen.x,
                fullscreen.y,
                fullscreen.width,
                fullscreen.height
            ),
            (0, 0, 1920, 1080)
        );
        assert_eq!(
            fullscreen.state_flags(),
            sws_protocol::window_state::FULLSCREEN
        );

        assert!(manager.unset_fullscreen_window(id));
        let restored = manager.get_window(id).unwrap();
        assert_eq!(
            (restored.x, restored.y, restored.width, restored.height),
            (25, 30, 640, 480)
        );
        assert_eq!(restored.state_flags(), 0);
    }

    #[test]
    fn fullscreen_uses_the_output_surface_extent_despite_normal_decoration_insets() {
        let mut manager = WindowManager::new();
        let id = manager.create_window_no_buffer(25, 30, 640, 480);
        assert!(manager.set_window_geometry(id, 10, 6, 620, 460).unwrap());
        let normal_surface = manager.get_window(id).unwrap().surface_geometry();
        let normal_window = manager.get_window(id).unwrap().window_geometry();

        assert!(manager.set_fullscreen_window(id, 1920, 1080));
        let fullscreen = manager.get_window(id).unwrap();
        assert_eq!(fullscreen.surface_geometry(), (0, 0, 1920, 1080));
        assert_eq!(fullscreen.window_geometry(), (0, 0, 1920, 1080));

        // ScarletUI responds to configure with its cached normal decoration
        // insets. That response must not offset the fullscreen surface.
        assert!(!manager.set_window_geometry(id, 10, 6, 1900, 1060).unwrap());
        assert_eq!(
            manager.get_window(id).unwrap().surface_geometry(),
            (0, 0, 1920, 1080)
        );

        assert!(manager.resize_fullscreen_window(id, 2560, 1440));
        assert!(!manager.set_window_geometry(id, 10, 6, 2540, 1420).unwrap());
        let resized = manager.get_window(id).unwrap();
        assert_eq!(resized.surface_geometry(), (0, 0, 2560, 1440));
        assert_eq!(resized.window_geometry(), (0, 0, 2560, 1440));

        assert!(manager.unset_fullscreen_window(id));
        let restored = manager.get_window(id).unwrap();
        assert_eq!(restored.surface_geometry(), normal_surface);
        assert_eq!(restored.window_geometry(), normal_window);
        assert!(!manager.resize_fullscreen_window(id, 1280, 720));
    }

    #[test]
    fn fullscreen_round_trip_preserves_maximized_restore_chain() {
        let mut manager = WindowManager::new();
        let id = manager.create_window_no_buffer(25, 30, 640, 480);

        assert!(manager.maximize_window(id, 1000, 700));
        manager.set_window_position(id, 10, 20);
        assert!(manager.set_fullscreen_window(id, 1920, 1080));
        assert_eq!(
            manager.get_window(id).unwrap().state_flags(),
            sws_protocol::window_state::MAXIMIZED | sws_protocol::window_state::FULLSCREEN
        );

        assert!(manager.unset_fullscreen_window(id));
        let maximized = manager.get_window(id).unwrap();
        assert_eq!(
            (maximized.x, maximized.y, maximized.width, maximized.height),
            (10, 20, 1000, 700)
        );
        assert!(maximized.maximized);

        assert!(manager.restore_window(id));
        let restored = manager.get_window(id).unwrap();
        assert_eq!(
            (restored.x, restored.y, restored.width, restored.height),
            (25, 30, 640, 480)
        );
    }

    #[test]
    fn restored_geometry_keeps_the_last_attached_backing_extent() {
        let mut manager = WindowManager::new();
        let id = manager.create_window(25, 30, 1844, 1284);

        assert!(manager.maximize_window(id, 2184, 1400));
        let maximized = manager.get_window_mut(id).unwrap();
        assert_eq!((maximized.width, maximized.height), (2184, 1400));
        maximized.set_backing_extent(2184, 1400);

        assert!(manager.restore_window(id));
        let restored = manager.get_window(id).unwrap();
        assert_eq!((restored.width, restored.height), (1844, 1284));
        assert_eq!(restored.backing_extent(), (2184, 1400));
    }

    #[test]
    fn pixels_use_attached_backing_while_managed_geometry_waits_for_resize() {
        let mut manager = WindowManager::new();
        let id = manager.create_window(25, 30, 640, 480);

        assert!(manager.maximize_window(id, 1000, 700));
        let window = manager.get_window(id).unwrap();
        assert_eq!((window.width, window.height), (1000, 700));
        assert_eq!(window.backing_extent(), (640, 480));

        let pixels = window.pixels().unwrap();
        assert_eq!((pixels.width(), pixels.height()), (640, 480));
        assert_eq!(pixels.stride(), 640 * 4);
        assert_eq!(pixels.bytes().len(), 640 * 480 * 4);
    }

    #[test]
    fn fullscreen_is_exclusive_and_keeps_transients_above_parent() {
        let mut manager = WindowManager::new();
        let fullscreen_id = manager.create_window_no_buffer(0, 0, 640, 480);
        let other_id = manager.create_window_no_buffer(50, 50, 320, 240);

        assert!(manager.set_fullscreen_window(fullscreen_id, 1920, 1080));
        manager.focus_window(fullscreen_id);
        assert!(!manager.set_fullscreen_window(other_id, 1920, 1080));

        let transient_id = manager.create_window_no_buffer(100, 100, 200, 100);
        assert!(manager.set_window_parent(transient_id, Some(fullscreen_id)));
        let order: Vec<u32> = manager
            .get_windows()
            .iter()
            .map(|window| window.id)
            .collect();
        assert_eq!(order[order.len() - 2..], [fullscreen_id, transient_id]);

        manager.focus_window(other_id);
        assert_eq!(manager.get_focused_window_id(), Some(fullscreen_id));
        manager.focus_window(transient_id);
        assert_eq!(manager.get_focused_window_id(), Some(transient_id));
    }
}
