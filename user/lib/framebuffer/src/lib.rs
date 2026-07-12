//! Framebuffer control library for Scarlet OS
//!
//! This library provides user-space APIs for framebuffer control operations,
//! including device access, drawing primitives, and display management.

#![no_std]

extern crate alloc;
extern crate scarlet_std as std;

use alloc::vec;
use std::{
    fs::File,
    handle::{
        capability::memory_mapping::{flags, munmap, prot},
        HandleError, HandleResult,
    },
    io::SeekFrom,
};

/// Linux framebuffer ioctl command constants
/// These provide compatibility with Linux framebuffer applications
pub mod commands {
    /// Get variable screen information
    pub const FBIOGET_VSCREENINFO: u32 = 0x4600;
    /// Set variable screen information  
    pub const FBIOPUT_VSCREENINFO: u32 = 0x4601;
    /// Get fixed screen information
    pub const FBIOGET_FSCREENINFO: u32 = 0x4602;
    /// Flush framebuffer to display
    pub const FBIO_FLUSH: u32 = 0x4620;
}

/// Scarlet display surface control command constants.
pub mod display_commands {
    /// Get display surface information.
    pub const DISPLAY_GET_INFO: u32 = 0x5000;
    /// Present the whole display surface.
    pub const DISPLAY_PRESENT: u32 = 0x5001;
    /// Present a display surface region.
    pub const DISPLAY_PRESENT_REGION: u32 = 0x5002;
    /// Get direct scanout swapchain information.
    pub const DISPLAY_GET_SWAPCHAIN: u32 = 0x5003;
    /// Present one direct scanout buffer.
    pub const DISPLAY_PRESENT_BUFFER: u32 = 0x5004;
    /// Wait for the most recently submitted page flip to complete.
    pub const DISPLAY_WAIT_FLIP: u32 = 0x5005;
}

/// 32-bit RGBA pixel layout.
pub const DISPLAY_PIXEL_FORMAT_RGBA8888: u32 = 1;
/// 32-bit BGRA pixel layout.
pub const DISPLAY_PIXEL_FORMAT_BGRA8888: u32 = 2;
/// 32-bit XRGB pixel layout.
pub const DISPLAY_PIXEL_FORMAT_XRGB8888: u32 = 3;
/// 32-bit XBGR pixel layout.
pub const DISPLAY_PIXEL_FORMAT_XBGR8888: u32 = 4;
/// 32-bit XRGB2101010 pixel layout.
pub const DISPLAY_PIXEL_FORMAT_XRGB2101010: u32 = 5;
/// 24-bit RGB pixel layout.
pub const DISPLAY_PIXEL_FORMAT_RGB888: u32 = 6;
/// 16-bit RGB565 pixel layout.
pub const DISPLAY_PIXEL_FORMAT_RGB565: u32 = 7;
/// 16-bit ARGB1555 pixel layout.
pub const DISPLAY_PIXEL_FORMAT_ARGB1555: u32 = 8;
/// 16-bit XRGB1555 pixel layout.
pub const DISPLAY_PIXEL_FORMAT_XRGB1555: u32 = 9;

/// Display surface information.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DisplayInfo {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bytes per row.
    pub stride: u32,
    /// Pixel format, one of `DISPLAY_PIXEL_FORMAT_*`.
    pub format: u32,
    /// Page-aligned size of the mappable display backing store.
    pub buffer_len: u32,
    /// Opaque identifier for the current mappable backing store.
    ///
    /// This value changes when the display surface's mapped backing changes,
    /// even if `buffer_len` remains the same.
    pub backing_id: usize,
}

/// Region argument for DISPLAY_PRESENT_REGION.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DisplayPresentRegion {
    /// Left edge in pixels.
    pub x: u32,
    /// Top edge in pixels.
    pub y: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// Direct scanout swapchain information.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DisplaySwapchainInfo {
    /// Number of scanout buffers.
    pub buffer_count: u32,
    /// Bytes in each mappable buffer.
    pub buffer_len: u32,
    /// mmap offset of the first direct scanout buffer.
    pub first_buffer_offset: usize,
}

/// Argument for `DISPLAY_PRESENT_BUFFER`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DisplayPresentBuffer {
    /// Direct scanout buffer index.
    pub index: u32,
    /// Reserved for future fence flags.
    pub flags: u32,
}

/// Color bit field information
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct FbBitfield {
    /// Bit offset from MSB
    pub offset: u32,
    /// Length in bits
    pub length: u32,
    /// MSB right shift
    pub msb_right: u32,
}

/// Variable screen information structure (Linux fb_var_screeninfo compatible)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FbVarScreenInfo {
    /// Visible resolution width
    pub xres: u32,
    /// Visible resolution height  
    pub yres: u32,
    /// Virtual resolution width
    pub xres_virtual: u32,
    /// Virtual resolution height
    pub yres_virtual: u32,
    /// Offset from virtual to visible resolution
    pub xoffset: u32,
    /// Offset from virtual to visible resolution
    pub yoffset: u32,
    /// Bits per pixel
    pub bits_per_pixel: u32,
    /// Grayscale != 0 means graylevels instead of colors
    pub grayscale: u32,
    /// Red bitfield
    pub red: FbBitfield,
    /// Green bitfield
    pub green: FbBitfield,
    /// Blue bitfield
    pub blue: FbBitfield,
    /// Transparency bitfield
    pub transp: FbBitfield,
    /// Non-zero if not grayscale
    pub nonstd: u32,
    /// Activate settings
    pub activate: u32,
    /// Screen height in mm
    pub height: u32,
    /// Screen width in mm
    pub width: u32,
    /// Acceleration flags
    pub accel_flags: u32,
    /// Pixel clock in picoseconds
    pub pixclock: u32,
    /// Time from sync to picture
    pub left_margin: u32,
    /// Time from picture to sync
    pub right_margin: u32,
    /// Time from sync to picture
    pub upper_margin: u32,
    /// Time from picture to sync
    pub lower_margin: u32,
    /// Length of horizontal sync
    pub hsync_len: u32,
    /// Length of vertical sync
    pub vsync_len: u32,
    /// Sync flags
    pub sync: u32,
    /// Video mode flags
    pub vmode: u32,
    /// Rotation angle (0=normal, 1=90°, 2=180°, 3=270°)
    pub rotate: u32,
    /// Color space for frame buffer
    pub colorspace: u32,
    /// Reserved for future use
    pub reserved: [u32; 4],
}

impl Default for FbVarScreenInfo {
    fn default() -> Self {
        Self {
            xres: 0,
            yres: 0,
            xres_virtual: 0,
            yres_virtual: 0,
            xoffset: 0,
            yoffset: 0,
            bits_per_pixel: 32,
            grayscale: 0,
            red: FbBitfield {
                offset: 16,
                length: 8,
                msb_right: 0,
            },
            green: FbBitfield {
                offset: 8,
                length: 8,
                msb_right: 0,
            },
            blue: FbBitfield {
                offset: 0,
                length: 8,
                msb_right: 0,
            },
            transp: FbBitfield {
                offset: 24,
                length: 8,
                msb_right: 0,
            },
            nonstd: 0,
            activate: 0,
            height: 0,
            width: 0,
            accel_flags: 0,
            pixclock: 0,
            left_margin: 0,
            right_margin: 0,
            upper_margin: 0,
            lower_margin: 0,
            hsync_len: 0,
            vsync_len: 0,
            sync: 0,
            vmode: 0,
            rotate: 0,
            colorspace: 0,
            reserved: [0; 4],
        }
    }
}

/// Fixed screen information structure (Linux fb_fix_screeninfo compatible)
#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct FbFixScreenInfo {
    /// Identification string
    pub id: [u8; 16],
    /// Start of frame buffer memory (physical address)
    pub smem_start: usize,
    /// Length of frame buffer memory
    pub smem_len: u32,
    /// Framebuffer type
    pub type_: u32,
    /// Type of auxiliary display
    pub type_aux: u32,
    /// Visual type
    pub visual: u32,
    /// Horizontal panning step size
    pub xpanstep: u16,
    /// Vertical panning step size
    pub ypanstep: u16,
    /// Y wrapping step size
    pub ywrapstep: u16,
    /// Length of a line in bytes
    pub line_length: u32,
    /// Start of memory-mapped I/O
    pub mmio_start: usize,
    /// Length of memory-mapped I/O
    pub mmio_len: u32,
    /// Acceleration capabilities
    pub accel: u32,
    /// Driver capabilities
    pub capabilities: u16,
    /// Reserved for future compatibility
    pub reserved: [u16; 2],
}

/// Framebuffer device wrapper
///
/// Wraps a File handle to provide framebuffer-specific control operations.
/// Uses memory mapping for efficient framebuffer access when available.
pub struct Framebuffer {
    file: File,
    /// Memory-mapped framebuffer buffer (address, size)
    mapped_buffer: Option<(usize, usize)>,
    mapped_physical_addr: Option<usize>,
}

/// Modern display surface wrapper.
///
/// This type opens `/dev/displayX` scanout endpoints. The current
/// implementation is CPU-composited and mappable, but presentation is explicit
/// and region-based instead of relying on legacy `/dev/fbX` semantics.
pub struct DisplaySurface {
    file: File,
    mapped_buffer: Option<(usize, usize)>,
    mapped_backing_id: usize,
    scratch_line: alloc::vec::Vec<u8>,
    cached_info: Option<DisplayInfo>,
    swapchain_buffers: alloc::vec::Vec<(usize, usize)>,
    swapchain_presented_at: alloc::vec::Vec<Option<u64>>,
    present_sequence: u64,
    draw_buffer: usize,
}

impl DisplaySurface {
    fn scale_component_to_field(value: u8, field: FbBitfield) -> u32 {
        if field.length == 0 {
            return 0;
        }

        let max = (1u32 << field.length) - 1;
        let scaled = ((value as u32) * max + 127) / 255;

        if field.msb_right == 0 {
            scaled
        } else {
            scaled.reverse_bits() >> (u32::BITS - field.length)
        }
    }

    fn pack_bgra_pixel(color: [u8; 4], var_info: &FbVarScreenInfo) -> u32 {
        (Self::scale_component_to_field(color[2], var_info.red) << var_info.red.offset)
            | (Self::scale_component_to_field(color[1], var_info.green) << var_info.green.offset)
            | (Self::scale_component_to_field(color[0], var_info.blue) << var_info.blue.offset)
            | (Self::scale_component_to_field(color[3], var_info.transp) << var_info.transp.offset)
    }

    fn write_packed_pixel_bytes(dst: &mut [u8], color: [u8; 4], var_info: &FbVarScreenInfo) {
        let bytes_per_pixel = Self::display_bytes_per_pixel_from_var(var_info);
        let pixel = Self::pack_bgra_pixel(color, var_info).to_le_bytes();
        dst[..bytes_per_pixel].copy_from_slice(&pixel[..bytes_per_pixel]);
    }

    fn display_bytes_per_pixel_from_var(var_info: &FbVarScreenInfo) -> usize {
        (var_info.bits_per_pixel as usize).div_ceil(8)
    }

    fn expand_8_to_10(value: u8) -> u32 {
        let value = value as u32;
        (value << 2) | (value >> 6)
    }

    fn convert_bgra_to_xrgb2101010_line(src: &[u8], dst: &mut [u8], width: usize) {
        for pixel in 0..width {
            let src_off = pixel * 4;
            let b10 = Self::expand_8_to_10(src[src_off]);
            let g10 = Self::expand_8_to_10(src[src_off + 1]);
            let r10 = Self::expand_8_to_10(src[src_off + 2]);
            let packed = (r10 << 20) | (g10 << 10) | b10;
            dst[pixel * 4..pixel * 4 + 4].copy_from_slice(&packed.to_le_bytes());
        }
    }

    /// Open the primary display surface.
    ///
    /// # Returns
    ///
    /// Display surface instance or HandleError on failure.
    pub fn open_primary() -> HandleResult<Self> {
        Self::open("/dev/display0")
    }

    /// Open a display surface.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the display device (e.g., "/dev/display0").
    ///
    /// # Returns
    ///
    /// Display surface instance or HandleError on failure.
    pub fn open(path: &str) -> HandleResult<Self> {
        let file = File::open(path).map_err(|_| HandleError::NotFound)?;
        let mut display = Self {
            file,
            mapped_buffer: None,
            mapped_backing_id: 0,
            scratch_line: alloc::vec::Vec::new(),
            cached_info: None,
            swapchain_buffers: alloc::vec::Vec::new(),
            swapchain_presented_at: alloc::vec::Vec::new(),
            present_sequence: 0,
            draw_buffer: 0,
        };
        let _ = display.setup_mmap();
        Ok(display)
    }

    fn setup_mmap(&mut self) -> HandleResult<()> {
        let info = self.get_info()?;
        if info.buffer_len == 0 {
            return Err(HandleError::InvalidParameter);
        }

        let handle = self.file.as_handle();
        let mapper = handle.as_memory_mapping()?;
        let mut swapchain = DisplaySwapchainInfo::default();
        if self
            .file
            .as_handle()
            .control(
                display_commands::DISPLAY_GET_SWAPCHAIN,
                &mut swapchain as *mut DisplaySwapchainInfo as usize,
            )
            .is_ok()
            && swapchain.buffer_count >= 2
            && swapchain.buffer_len != 0
        {
            for index in 0..swapchain.buffer_count as usize {
                let offset = swapchain.first_buffer_offset + index * swapchain.buffer_len as usize;
                let address = mapper
                    .mmap(
                        0,
                        swapchain.buffer_len as usize,
                        prot::READ | prot::WRITE,
                        flags::SHARED,
                        offset,
                    )
                    .map_err(|_| HandleError::SystemError(-1))?;
                self.swapchain_buffers
                    .push((address, swapchain.buffer_len as usize));
                self.swapchain_presented_at.push(None);
            }
            // DCP starts with scanout buffer zero front-most.
            self.draw_buffer = 1;
            self.mapped_buffer = Some(self.swapchain_buffers[self.draw_buffer]);
            self.mapped_backing_id = info.backing_id;
            self.cached_info = Some(info);
            return Ok(());
        }

        let mapped_addr = mapper
            .mmap(
                0,
                info.buffer_len as usize,
                prot::READ | prot::WRITE,
                flags::SHARED,
                0,
            )
            .map_err(|_| HandleError::SystemError(-1))?;
        self.mapped_buffer = Some((mapped_addr, info.buffer_len as usize));
        self.mapped_backing_id = info.backing_id;
        self.cached_info = Some(info);
        Ok(())
    }

    fn display_bytes_per_pixel(format: u32) -> usize {
        match format {
            DISPLAY_PIXEL_FORMAT_RGBA8888
            | DISPLAY_PIXEL_FORMAT_BGRA8888
            | DISPLAY_PIXEL_FORMAT_XRGB8888
            | DISPLAY_PIXEL_FORMAT_XBGR8888
            | DISPLAY_PIXEL_FORMAT_XRGB2101010 => 4,
            DISPLAY_PIXEL_FORMAT_RGB888 => 3,
            DISPLAY_PIXEL_FORMAT_RGB565
            | DISPLAY_PIXEL_FORMAT_ARGB1555
            | DISPLAY_PIXEL_FORMAT_XRGB1555 => 2,
            _ => 0,
        }
    }

    fn display_info_to_var_info(info: DisplayInfo) -> FbVarScreenInfo {
        let mut var_info = FbVarScreenInfo {
            xres: info.width,
            yres: info.height,
            xres_virtual: info.width,
            yres_virtual: info.height,
            bits_per_pixel: (Self::display_bytes_per_pixel(info.format) * 8) as u32,
            ..FbVarScreenInfo::default()
        };

        match info.format {
            DISPLAY_PIXEL_FORMAT_RGBA8888 => {
                var_info.red = FbBitfield {
                    offset: 0,
                    length: 8,
                    msb_right: 0,
                };
                var_info.green = FbBitfield {
                    offset: 8,
                    length: 8,
                    msb_right: 0,
                };
                var_info.blue = FbBitfield {
                    offset: 16,
                    length: 8,
                    msb_right: 0,
                };
                var_info.transp = FbBitfield {
                    offset: 24,
                    length: 8,
                    msb_right: 0,
                };
            }
            DISPLAY_PIXEL_FORMAT_BGRA8888 | DISPLAY_PIXEL_FORMAT_XBGR8888 => {
                var_info.blue = FbBitfield {
                    offset: 0,
                    length: 8,
                    msb_right: 0,
                };
                var_info.green = FbBitfield {
                    offset: 8,
                    length: 8,
                    msb_right: 0,
                };
                var_info.red = FbBitfield {
                    offset: 16,
                    length: 8,
                    msb_right: 0,
                };
                var_info.transp = FbBitfield {
                    offset: 24,
                    length: if info.format == DISPLAY_PIXEL_FORMAT_BGRA8888 {
                        8
                    } else {
                        0
                    },
                    msb_right: 0,
                };
            }
            DISPLAY_PIXEL_FORMAT_XRGB8888 => {
                var_info.red = FbBitfield {
                    offset: 0,
                    length: 8,
                    msb_right: 0,
                };
                var_info.green = FbBitfield {
                    offset: 8,
                    length: 8,
                    msb_right: 0,
                };
                var_info.blue = FbBitfield {
                    offset: 16,
                    length: 8,
                    msb_right: 0,
                };
                var_info.transp = FbBitfield {
                    offset: 24,
                    length: 0,
                    msb_right: 0,
                };
            }
            DISPLAY_PIXEL_FORMAT_XRGB2101010 => {
                var_info.red = FbBitfield {
                    offset: 20,
                    length: 10,
                    msb_right: 0,
                };
                var_info.green = FbBitfield {
                    offset: 10,
                    length: 10,
                    msb_right: 0,
                };
                var_info.blue = FbBitfield {
                    offset: 0,
                    length: 10,
                    msb_right: 0,
                };
                var_info.transp = FbBitfield {
                    offset: 30,
                    length: 0,
                    msb_right: 0,
                };
            }
            DISPLAY_PIXEL_FORMAT_RGB888 => {
                var_info.red = FbBitfield {
                    offset: 0,
                    length: 8,
                    msb_right: 0,
                };
                var_info.green = FbBitfield {
                    offset: 8,
                    length: 8,
                    msb_right: 0,
                };
                var_info.blue = FbBitfield {
                    offset: 16,
                    length: 8,
                    msb_right: 0,
                };
            }
            DISPLAY_PIXEL_FORMAT_RGB565 => {
                var_info.red = FbBitfield {
                    offset: 11,
                    length: 5,
                    msb_right: 0,
                };
                var_info.green = FbBitfield {
                    offset: 5,
                    length: 6,
                    msb_right: 0,
                };
                var_info.blue = FbBitfield {
                    offset: 0,
                    length: 5,
                    msb_right: 0,
                };
            }
            DISPLAY_PIXEL_FORMAT_ARGB1555 | DISPLAY_PIXEL_FORMAT_XRGB1555 => {
                var_info.red = FbBitfield {
                    offset: 10,
                    length: 5,
                    msb_right: 0,
                };
                var_info.green = FbBitfield {
                    offset: 5,
                    length: 5,
                    msb_right: 0,
                };
                var_info.blue = FbBitfield {
                    offset: 0,
                    length: 5,
                    msb_right: 0,
                };
                var_info.transp = FbBitfield {
                    offset: 15,
                    length: if info.format == DISPLAY_PIXEL_FORMAT_ARGB1555 {
                        1
                    } else {
                        0
                    },
                    msb_right: 0,
                };
            }
            _ => {}
        }

        var_info
    }

    fn ensure_info(&mut self) -> HandleResult<DisplayInfo> {
        let info = match self.cached_info {
            Some(info) => info,
            None => {
                let info = self.get_info()?;
                self.cached_info = Some(info);
                info
            }
        };
        let line_bytes = info.width as usize * Self::display_bytes_per_pixel(info.format);
        if self.scratch_line.len() < line_bytes {
            self.scratch_line.resize(line_bytes, 0);
        }
        Ok(info)
    }

    /// Get display surface information.
    ///
    /// # Returns
    ///
    /// Display surface information or HandleError on failure.
    pub fn get_info(&self) -> HandleResult<DisplayInfo> {
        let mut info = DisplayInfo::default();
        self.file.as_handle().control(
            display_commands::DISPLAY_GET_INFO,
            &mut info as *mut DisplayInfo as usize,
        )?;
        Ok(info)
    }

    /// Get variable screen information from the display surface.
    ///
    /// # Returns
    ///
    /// Variable screen information or HandleError on failure.
    pub fn get_var_screen_info(&self) -> HandleResult<FbVarScreenInfo> {
        Ok(Self::display_info_to_var_info(self.get_info()?))
    }

    /// Get fixed screen information from the display surface.
    ///
    /// # Returns
    ///
    /// Fixed screen information or HandleError on failure.
    pub fn get_fix_screen_info(&self) -> HandleResult<FbFixScreenInfo> {
        let info = self.get_info()?;
        let mut fix_info = FbFixScreenInfo::default();
        let id = b"display";
        fix_info.id[..id.len()].copy_from_slice(id);
        fix_info.smem_len = info.buffer_len;
        fix_info.line_length = info.stride;
        fix_info.type_ = 0;
        fix_info.visual = 2;
        Ok(fix_info)
    }

    /// Refresh the display memory mapping if the kernel reports a new backing store.
    ///
    /// # Returns
    ///
    /// Success or HandleError on failure.
    pub fn refresh_mapping(&mut self) -> HandleResult<()> {
        let info = self.get_info()?;
        let new_size = info.buffer_len as usize;
        if matches!(self.mapped_buffer, Some((_, mapped_size)) if mapped_size == new_size)
            && self.mapped_backing_id == info.backing_id
        {
            return Ok(());
        }

        if self.swapchain_buffers.is_empty() {
            if let Some((mapped_addr, mapped_size)) = self.mapped_buffer.take() {
                let _ = munmap(mapped_addr, mapped_size);
            }
        } else {
            for (mapped_addr, mapped_size) in self.swapchain_buffers.drain(..) {
                let _ = munmap(mapped_addr, mapped_size);
            }
            self.mapped_buffer = None;
        }
        self.mapped_backing_id = 0;
        self.cached_info = None;

        match self.setup_mmap() {
            Ok(()) => Ok(()),
            Err(_) => Ok(()),
        }
    }

    /// Get memory mapping information if available.
    ///
    /// # Returns
    ///
    /// Mapping address and size if memory mapping is active.
    pub fn get_mapping_info(&self) -> Option<(usize, usize)> {
        self.mapped_buffer
    }

    /// Return whether this surface uses direct scanout buffers.
    ///
    /// # Returns
    ///
    /// `true` when presentation queues a directly mapped scanout buffer.
    pub fn has_swapchain(&self) -> bool {
        !self.swapchain_buffers.is_empty()
    }

    /// Return the age of the currently acquired direct scanout buffer.
    ///
    /// An age of zero means that the buffer contents are undefined and require
    /// a complete redraw. A positive age is the number of successfully
    /// presented frames since this buffer was last presented.
    ///
    /// # Returns
    ///
    /// The current buffer age, or `None` when direct scanout is unavailable.
    pub fn buffer_age(&self) -> Option<u64> {
        if self.swapchain_buffers.is_empty() {
            return None;
        }

        Some(match self.swapchain_presented_at[self.draw_buffer] {
            Some(sequence) => self.present_sequence.saturating_sub(sequence),
            None => 0,
        })
    }

    /// Return the number of direct scanout buffers.
    ///
    /// # Returns
    ///
    /// The swapchain length, or zero when direct scanout is unavailable.
    pub fn swapchain_buffer_count(&self) -> usize {
        self.swapchain_buffers.len()
    }

    /// Present the whole display surface.
    ///
    /// # Returns
    ///
    /// Success or HandleError on failure.
    pub fn present(&mut self) -> HandleResult<()> {
        if !self.swapchain_buffers.is_empty() {
            let request = DisplayPresentBuffer {
                index: self.draw_buffer as u32,
                flags: 0,
            };
            self.file.as_handle().control(
                display_commands::DISPLAY_PRESENT_BUFFER,
                &request as *const DisplayPresentBuffer as usize,
            )?;
            self.present_sequence = self.present_sequence.saturating_add(1);
            self.swapchain_presented_at[self.draw_buffer] = Some(self.present_sequence);
            self.draw_buffer = (self.draw_buffer + 1) % self.swapchain_buffers.len();
            self.mapped_buffer = Some(self.swapchain_buffers[self.draw_buffer]);
            return Ok(());
        }
        self.file
            .as_handle()
            .control(display_commands::DISPLAY_PRESENT, 0)?;
        Ok(())
    }

    /// Wait for the previous page flip to complete.
    ///
    /// Call this before rendering into the back buffer to guarantee the
    /// hardware has finished scanning out from it.
    pub fn wait_for_flip(&self) -> HandleResult<()> {
        self.file
            .as_handle()
            .control(display_commands::DISPLAY_WAIT_FLIP, 0)?;
        Ok(())
    }

    /// Present a display surface region.
    ///
    /// # Arguments
    ///
    /// * `x` - Left edge in pixels.
    /// * `y` - Top edge in pixels.
    /// * `width` - Width in pixels.
    /// * `height` - Height in pixels.
    ///
    /// # Returns
    ///
    /// Success or HandleError on failure.
    pub fn present_region(&mut self, x: u32, y: u32, width: u32, height: u32) -> HandleResult<()> {
        if width == 0 || height == 0 {
            return Ok(());
        }

        if !self.swapchain_buffers.is_empty() {
            return self.present();
        }

        let region = DisplayPresentRegion {
            x,
            y,
            width,
            height,
        };
        self.file.as_handle().control(
            display_commands::DISPLAY_PRESENT_REGION,
            &region as *const DisplayPresentRegion as usize,
        )?;
        Ok(())
    }

    /// Write BGRA source data into a display surface region.
    ///
    /// # Arguments
    ///
    /// * `x` - Destination left edge in pixels.
    /// * `y` - Destination top edge in pixels.
    /// * `width` - Region width in pixels.
    /// * `height` - Region height in pixels.
    /// * `data` - Source BGRA pixel bytes.
    /// * `src_stride_bytes` - Source row stride in bytes.
    ///
    /// # Returns
    ///
    /// Success or HandleError on failure.
    pub fn write_bgra_strided(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        data: &[u8],
        src_stride_bytes: usize,
    ) -> HandleResult<()> {
        if width == 0 || height == 0 {
            return Ok(());
        }

        let info = self.ensure_info()?;
        if x >= info.width || y >= info.height {
            return Err(HandleError::InvalidParameter);
        }
        if x.saturating_add(width) > info.width || y.saturating_add(height) > info.height {
            return Err(HandleError::InvalidParameter);
        }

        let line_length = info.stride as usize;
        let dst_bytes_per_pixel = Self::display_bytes_per_pixel(info.format);
        if dst_bytes_per_pixel == 0 {
            return Err(HandleError::InvalidParameter);
        }
        let src_line_bytes = width as usize * 4;
        if src_stride_bytes < src_line_bytes {
            return Err(HandleError::InvalidParameter);
        }
        let required = (height as usize - 1)
            .saturating_mul(src_stride_bytes)
            .saturating_add(src_line_bytes);
        if required > data.len() {
            return Err(HandleError::InvalidParameter);
        }

        if info.format == DISPLAY_PIXEL_FORMAT_XRGB2101010 {
            let line_bytes = width as usize * 4;
            let mapped_buffer = self.mapped_buffer;

            for row in 0..height as usize {
                let src_off = row.saturating_mul(src_stride_bytes);
                let src_row = &data[src_off..src_off + src_line_bytes];
                {
                    let converted_line = &mut self.scratch_line[..line_bytes];
                    Self::convert_bgra_to_xrgb2101010_line(src_row, converted_line, width as usize);
                }

                let dst_off = (y as usize + row)
                    .saturating_mul(line_length)
                    .saturating_add(x as usize * 4);
                if let Some((mapped_addr, mapped_size)) = mapped_buffer {
                    if dst_off.saturating_add(line_bytes) > mapped_size {
                        return Err(HandleError::InvalidParameter);
                    }
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            self.scratch_line.as_ptr(),
                            (mapped_addr + dst_off) as *mut u8,
                            line_bytes,
                        );
                    }
                } else {
                    self.file
                        .seek(SeekFrom::Start(dst_off as u64))
                        .map_err(|_| HandleError::SystemError(-1))?;
                    self.file
                        .write(&self.scratch_line[..line_bytes])
                        .map_err(|_| HandleError::SystemError(-1))?;
                }
            }

            return Ok(());
        }

        if info.format != DISPLAY_PIXEL_FORMAT_BGRA8888 {
            let var_info = Self::display_info_to_var_info(info);
            let line_bytes = width as usize * dst_bytes_per_pixel;
            let mapped_buffer = self.mapped_buffer;

            for row in 0..height as usize {
                let src_off = row.saturating_mul(src_stride_bytes);
                let src_row = &data[src_off..src_off + src_line_bytes];

                {
                    let converted_line = &mut self.scratch_line[..line_bytes];
                    for pixel in 0..width as usize {
                        let src_pixel_offset = pixel * 4;
                        let dst_pixel_offset = pixel * dst_bytes_per_pixel;
                        let color = [
                            src_row[src_pixel_offset],
                            src_row[src_pixel_offset + 1],
                            src_row[src_pixel_offset + 2],
                            src_row[src_pixel_offset + 3],
                        ];
                        Self::write_packed_pixel_bytes(
                            &mut converted_line
                                [dst_pixel_offset..dst_pixel_offset + dst_bytes_per_pixel],
                            color,
                            &var_info,
                        );
                    }
                }

                let dst_off = (y as usize + row)
                    .saturating_mul(line_length)
                    .saturating_add(x as usize * dst_bytes_per_pixel);
                if let Some((mapped_addr, mapped_size)) = mapped_buffer {
                    if dst_off.saturating_add(line_bytes) > mapped_size {
                        return Err(HandleError::InvalidParameter);
                    }
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            self.scratch_line.as_ptr(),
                            (mapped_addr + dst_off) as *mut u8,
                            line_bytes,
                        );
                    }
                } else {
                    self.file
                        .seek(SeekFrom::Start(dst_off as u64))
                        .map_err(|_| HandleError::SystemError(-1))?;
                    self.file
                        .write(&self.scratch_line[..line_bytes])
                        .map_err(|_| HandleError::SystemError(-1))?;
                }
            }

            return Ok(());
        }

        if let Some((mapped_addr, mapped_size)) = self.mapped_buffer {
            for row in 0..height as usize {
                let dst_off = (y as usize + row)
                    .saturating_mul(line_length)
                    .saturating_add(x as usize * dst_bytes_per_pixel);
                let src_off = row.saturating_mul(src_stride_bytes);
                if dst_off.saturating_add(src_line_bytes) > mapped_size {
                    return Err(HandleError::InvalidParameter);
                }
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        data[src_off..src_off + src_line_bytes].as_ptr(),
                        (mapped_addr + dst_off) as *mut u8,
                        src_line_bytes,
                    );
                }
            }
        } else {
            for row in 0..height as usize {
                let dst_off = (y as usize + row)
                    .saturating_mul(line_length)
                    .saturating_add(x as usize * dst_bytes_per_pixel);
                let src_off = row.saturating_mul(src_stride_bytes);
                self.file
                    .seek(SeekFrom::Start(dst_off as u64))
                    .map_err(|_| HandleError::SystemError(-1))?;
                self.file
                    .write(&data[src_off..src_off + src_line_bytes])
                    .map_err(|_| HandleError::SystemError(-1))?;
            }
        }

        Ok(())
    }
}

impl Framebuffer {
    fn bytes_per_pixel(var_info: &FbVarScreenInfo) -> usize {
        (var_info.bits_per_pixel as usize).div_ceil(8)
    }

    fn scale_component_to_field(value: u8, field: FbBitfield) -> u32 {
        if field.length == 0 {
            return 0;
        }

        let max = (1u32 << field.length) - 1;
        let scaled = ((value as u32) * max + 127) / 255;

        if field.msb_right == 0 {
            scaled
        } else {
            scaled.reverse_bits() >> (u32::BITS - field.length)
        }
    }

    fn pack_bgra_pixel(color: [u8; 4], var_info: &FbVarScreenInfo) -> u32 {
        (Self::scale_component_to_field(color[2], var_info.red) << var_info.red.offset)
            | (Self::scale_component_to_field(color[1], var_info.green) << var_info.green.offset)
            | (Self::scale_component_to_field(color[0], var_info.blue) << var_info.blue.offset)
            | (Self::scale_component_to_field(color[3], var_info.transp) << var_info.transp.offset)
    }

    fn write_packed_pixel_bytes(dst: &mut [u8], color: [u8; 4], var_info: &FbVarScreenInfo) {
        let bytes_per_pixel = Self::bytes_per_pixel(var_info);
        let pixel = Self::pack_bgra_pixel(color, var_info).to_le_bytes();
        dst[..bytes_per_pixel].copy_from_slice(&pixel[..bytes_per_pixel]);
    }

    fn is_native_bgra8888(var_info: &FbVarScreenInfo) -> bool {
        var_info.bits_per_pixel == 32
            && var_info.red.offset == 16
            && var_info.red.length == 8
            && var_info.red.msb_right == 0
            && var_info.green.offset == 8
            && var_info.green.length == 8
            && var_info.green.msb_right == 0
            && var_info.blue.offset == 0
            && var_info.blue.length == 8
            && var_info.blue.msb_right == 0
            && var_info.transp.offset == 24
            && var_info.transp.length == 8
            && var_info.transp.msb_right == 0
    }

    fn populate_line_with_color(
        line: &mut [u8],
        width: usize,
        color: [u8; 4],
        var_info: &FbVarScreenInfo,
    ) {
        let bytes_per_pixel = Self::bytes_per_pixel(var_info);
        for x in 0..width {
            let pixel_offset = x * bytes_per_pixel;
            if pixel_offset + bytes_per_pixel <= line.len() {
                Self::write_packed_pixel_bytes(
                    &mut line[pixel_offset..pixel_offset + bytes_per_pixel],
                    color,
                    var_info,
                );
            }
        }
    }

    /// Open a framebuffer device
    ///
    /// # Arguments
    /// * `path` - Path to the framebuffer device (e.g., "/dev/fb0")
    ///
    /// # Returns
    /// Framebuffer instance or HandleError on failure
    pub fn open(path: &str) -> HandleResult<Self> {
        let file = File::open(path).map_err(|_| HandleError::NotFound)?;

        // Try to get framebuffer info for memory mapping
        let mut framebuffer = Self {
            file,
            mapped_buffer: None,
            mapped_physical_addr: None,
        };

        // Attempt to set up memory mapping
        if framebuffer.setup_mmap().is_err() {
            // If mmap fails, continue with traditional file I/O
            // This provides backward compatibility
        }

        Ok(framebuffer)
    }

    /// Attempt to set up memory mapping for the framebuffer
    fn setup_mmap(&mut self) -> HandleResult<()> {
        // Get framebuffer information
        let fix_info = self.get_fix_screen_info()?;

        // Ensure we have valid framebuffer size
        if fix_info.smem_len == 0 {
            return Err(HandleError::InvalidParameter);
        }

        // Try to map the framebuffer memory
        let handle = self.file.as_handle();
        let mapper = handle.as_memory_mapping()?;
        match mapper.mmap(
            0,                          // Let kernel choose address
            fix_info.smem_len as usize, // Map entire framebuffer
            prot::READ | prot::WRITE,   // Read/write permissions
            flags::SHARED,              // Shared mapping
            0,                          // Offset 0
        ) {
            Ok(mapped_addr) => {
                self.mapped_buffer = Some((mapped_addr, fix_info.smem_len as usize));
                self.mapped_physical_addr = Some(fix_info.smem_start);
                Ok(())
            }
            Err(e) => {
                // Debug output to understand why mmap failed
                std::println!(
                    "mmap failed: handle={}, size={}, error={:?}",
                    handle.as_raw(),
                    fix_info.smem_len,
                    e
                );
                Err(HandleError::SystemError(-1))
            }
        }
    }

    /// Get variable screen information from the framebuffer device
    ///
    /// # Returns
    /// Variable screen information or HandleError on failure
    pub fn get_var_screen_info(&self) -> HandleResult<FbVarScreenInfo> {
        let mut var_info = FbVarScreenInfo::default();
        self.file.as_handle().control(
            commands::FBIOGET_VSCREENINFO,
            &mut var_info as *mut _ as usize,
        )?;
        Ok(var_info)
    }

    /// Get fixed screen information from the framebuffer device
    ///
    /// # Returns
    /// Fixed screen information or HandleError on failure
    pub fn get_fix_screen_info(&self) -> HandleResult<FbFixScreenInfo> {
        let mut fix_info = FbFixScreenInfo::default();
        let ptr = &mut fix_info as *mut FbFixScreenInfo;
        if ptr.is_null() {
            return Err(HandleError::InvalidParameter);
        }
        self.file
            .as_handle()
            .control(commands::FBIOGET_FSCREENINFO, ptr as usize)?;
        Ok(fix_info)
    }

    /// Set variable screen information for the framebuffer device
    ///
    /// # Arguments
    /// * `var_info` - New variable screen information
    ///
    /// # Returns
    /// Success or HandleError on failure
    pub fn set_var_screen_info(&self, var_info: &FbVarScreenInfo) -> HandleResult<()> {
        self.file
            .as_handle()
            .control(commands::FBIOPUT_VSCREENINFO, var_info as *const _ as usize)?;
        Ok(())
    }

    /// Refresh the framebuffer memory mapping if the kernel reports a new backing store.
    ///
    /// # Returns
    /// Success or HandleError on failure
    pub fn refresh_mapping(&mut self) -> HandleResult<()> {
        let fix_info = self.get_fix_screen_info()?;
        let new_size = fix_info.smem_len as usize;
        let mapping_changed = match (self.mapped_buffer, self.mapped_physical_addr) {
            (Some((_, mapped_size)), Some(mapped_phys)) => {
                mapped_size != new_size || mapped_phys != fix_info.smem_start
            }
            (None, _) => true,
            (_, None) => true,
        };

        if !mapping_changed {
            return Ok(());
        }

        if let Some((mapped_addr, mapped_size)) = self.mapped_buffer.take() {
            let _ = munmap(mapped_addr, mapped_size);
        }
        self.mapped_physical_addr = None;

        match self.setup_mmap() {
            Ok(()) => Ok(()),
            Err(_) => Ok(()),
        }
    }

    /// Flush framebuffer to display
    ///
    /// Forces any pending framebuffer changes to be displayed.
    ///
    /// # Returns
    /// Success or HandleError on failure
    pub fn flush(&self) -> HandleResult<()> {
        self.file.as_handle().control(commands::FBIO_FLUSH, 0)?;
        Ok(())
    }

    /// Get the underlying file
    ///
    /// Provides access to the File for other operations
    pub fn file(&mut self) -> &mut File {
        &mut self.file
    }

    /// Check if memory mapping is being used
    ///
    /// Returns true if framebuffer operations use mmap, false if using file I/O
    pub fn is_using_mmap(&self) -> bool {
        self.mapped_buffer.is_some()
    }

    /// Get memory mapping information if available
    ///
    /// Returns (address, size) if memory mapping is active, None otherwise
    pub fn get_mapping_info(&self) -> Option<(usize, usize)> {
        self.mapped_buffer
    }

    /// Write a single pixel to the framebuffer
    ///
    /// # Arguments
    /// * `x` - X coordinate
    /// * `y` - Y coordinate  
    /// * `color` - Pixel color [B, G, R, A]
    ///
    /// # Returns
    /// Success or HandleError on failure
    pub fn write_pixel(&mut self, x: u32, y: u32, color: [u8; 4]) -> HandleResult<()> {
        let var_info = self.get_var_screen_info()?;
        let fix_info = self.get_fix_screen_info()?;

        let bytes_per_pixel = Self::bytes_per_pixel(&var_info);
        let line_length = fix_info.line_length as usize;
        let mut packed_pixel = [0u8; 4];
        Self::write_packed_pixel_bytes(&mut packed_pixel[..bytes_per_pixel], color, &var_info);

        // Calculate pixel offset
        let offset = y as usize * line_length + x as usize * bytes_per_pixel;

        if let Some((mapped_addr, mapped_size)) = self.mapped_buffer {
            // Use memory-mapped access for better performance
            if offset + bytes_per_pixel > mapped_size {
                return Err(HandleError::InvalidParameter);
            }

            unsafe {
                let pixel_ptr = (mapped_addr + offset) as *mut u8;
                core::ptr::copy_nonoverlapping(packed_pixel.as_ptr(), pixel_ptr, bytes_per_pixel);
            }
        } else {
            // Fallback to file I/O if mmap is not available
            self.file
                .seek(SeekFrom::Start(offset as u64))
                .map_err(|_| HandleError::SystemError(-1))?;

            self.file
                .write(&packed_pixel[..bytes_per_pixel])
                .map_err(|_| HandleError::SystemError(-1))?;
        }

        Ok(())
    }

    /// Write a horizontal line to the framebuffer
    ///
    /// # Arguments
    /// * `y` - Y coordinate of the line
    /// * `data` - Pixel data for the entire line
    ///
    /// # Returns
    /// Success or HandleError on failure
    pub fn write_line(&mut self, y: u32, data: &[u8]) -> HandleResult<()> {
        let fix_info = self.get_fix_screen_info()?;
        let line_length = fix_info.line_length as usize;
        let offset = y as usize * line_length;

        if let Some((mapped_addr, mapped_size)) = self.mapped_buffer {
            // Use memory-mapped access for better performance
            let write_len = data.len().min(line_length);
            if offset + write_len > mapped_size {
                return Err(HandleError::InvalidParameter);
            }

            unsafe {
                let line_ptr = (mapped_addr + offset) as *mut u8;
                core::ptr::copy_nonoverlapping(data.as_ptr(), line_ptr, write_len);
            }
        } else {
            // Fallback to file I/O if mmap is not available
            self.file
                .seek(SeekFrom::Start(offset as u64))
                .map_err(|_| HandleError::SystemError(-1))?;

            let write_len = data.len().min(line_length);
            self.file
                .write(&data[..write_len])
                .map_err(|_| HandleError::SystemError(-1))?;
        }

        Ok(())
    }

    /// Write a rectangular block of pixels to the framebuffer
    ///
    /// This is the most efficient way to update a large area.
    ///
    /// # Arguments
    /// * `x` - X coordinate of the block
    /// * `y` - Y coordinate of the block
    /// * `width` - Width of the block in pixels
    /// * `height` - Height of the block in pixels
    /// * `data` - Pixel data (width * height * bytes_per_pixel)
    ///
    /// # Returns
    /// Success or HandleError on failure
    pub fn write_block(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        data: &[u8],
    ) -> HandleResult<()> {
        let var_info = self.get_var_screen_info()?;
        let fix_info = self.get_fix_screen_info()?;

        let bytes_per_pixel = Self::bytes_per_pixel(&var_info);
        let line_length = fix_info.line_length as usize;
        let block_line_bytes = width as usize * bytes_per_pixel;

        if let Some((mapped_addr, mapped_size)) = self.mapped_buffer {
            // Use memory-mapped access for better performance
            // Write line by line
            for row in 0..height {
                let line_y = y + row;
                let line_offset = line_y as usize * line_length + x as usize * bytes_per_pixel;
                let data_offset = row as usize * block_line_bytes;
                let data_end = data_offset + block_line_bytes;

                if line_offset + block_line_bytes > mapped_size || data_end > data.len() {
                    continue; // Skip invalid lines
                }

                unsafe {
                    let line_ptr = (mapped_addr + line_offset) as *mut u8;
                    core::ptr::copy_nonoverlapping(
                        data[data_offset..data_end].as_ptr(),
                        line_ptr,
                        block_line_bytes,
                    );
                }
            }
        } else {
            // Fallback to file I/O if mmap is not available
            for row in 0..height {
                let line_y = y + row;
                let line_offset = line_y as usize * line_length + x as usize * bytes_per_pixel;
                let data_offset = row as usize * block_line_bytes;

                // Seek to start of this line in the block
                self.file
                    .seek(SeekFrom::Start(line_offset as u64))
                    .map_err(|_| HandleError::SystemError(-1))?;

                // Write one line of the block
                let data_end = data_offset + block_line_bytes;
                if data_end <= data.len() {
                    self.file
                        .write(&data[data_offset..data_end])
                        .map_err(|_| HandleError::SystemError(-1))?;
                }
            }
        }

        Ok(())
    }

    /// Write a rectangular block of pixels to the framebuffer from a strided source.
    ///
    /// This is useful when the source is a sub-rectangle of a larger packed buffer
    /// (e.g., copying a window interior while the source stride is the full window width).
    ///
    /// The copy is a simple overwrite (no alpha blending).
    ///
    /// # Arguments
    /// * `x` - Destination X coordinate in pixels
    /// * `y` - Destination Y coordinate in pixels
    /// * `width` - Width of the block in pixels
    /// * `height` - Height of the block in pixels
    /// * `data` - Source pixel data starting at the top-left of the block
    /// * `src_stride_bytes` - Source stride in bytes (bytes between consecutive rows)
    ///
    /// # Returns
    /// Success or HandleError on failure
    pub fn write_block_strided(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        data: &[u8],
        src_stride_bytes: usize,
    ) -> HandleResult<()> {
        if width == 0 || height == 0 {
            return Ok(());
        }

        let var_info = self.get_var_screen_info()?;
        let fix_info = self.get_fix_screen_info()?;

        let bytes_per_pixel = Self::bytes_per_pixel(&var_info);
        let line_length = fix_info.line_length as usize;
        let block_line_bytes = width as usize * bytes_per_pixel;

        if src_stride_bytes < block_line_bytes {
            return Err(HandleError::InvalidParameter);
        }

        let required = (height as usize - 1)
            .saturating_mul(src_stride_bytes)
            .saturating_add(block_line_bytes);
        if required > data.len() {
            return Err(HandleError::InvalidParameter);
        }

        if let Some((mapped_addr, mapped_size)) = self.mapped_buffer {
            for row in 0..height {
                let dst_y = y + row;
                let dst_off = (dst_y as usize)
                    .saturating_mul(line_length)
                    .saturating_add((x as usize).saturating_mul(bytes_per_pixel));
                if dst_off.saturating_add(block_line_bytes) > mapped_size {
                    return Err(HandleError::InvalidParameter);
                }

                let src_off = (row as usize).saturating_mul(src_stride_bytes);
                let src_end = src_off.saturating_add(block_line_bytes);

                unsafe {
                    let dst_ptr = (mapped_addr + dst_off) as *mut u8;
                    core::ptr::copy_nonoverlapping(
                        data[src_off..src_end].as_ptr(),
                        dst_ptr,
                        block_line_bytes,
                    );
                }
            }
        } else {
            for row in 0..height {
                let dst_y = y + row;
                let dst_off = (dst_y as usize)
                    .saturating_mul(line_length)
                    .saturating_add((x as usize).saturating_mul(bytes_per_pixel));
                self.file
                    .seek(SeekFrom::Start(dst_off as u64))
                    .map_err(|_| HandleError::SystemError(-1))?;

                let src_off = (row as usize).saturating_mul(src_stride_bytes);
                let src_end = src_off.saturating_add(block_line_bytes);
                self.file
                    .write(&data[src_off..src_end])
                    .map_err(|_| HandleError::SystemError(-1))?;
            }
        }

        Ok(())
    }

    pub fn write_block_bgra_strided(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        data: &[u8],
        src_stride_bytes: usize,
    ) -> HandleResult<()> {
        if width == 0 || height == 0 {
            return Ok(());
        }

        let var_info = self.get_var_screen_info()?;
        let fix_info = self.get_fix_screen_info()?;
        let dst_bytes_per_pixel = Self::bytes_per_pixel(&var_info);
        let line_length = fix_info.line_length as usize;
        let src_line_bytes = width as usize * 4;

        if Self::is_native_bgra8888(&var_info) {
            return self.write_block_strided(x, y, width, height, data, src_stride_bytes);
        }

        if src_stride_bytes < src_line_bytes {
            return Err(HandleError::InvalidParameter);
        }

        let required = (height as usize - 1)
            .saturating_mul(src_stride_bytes)
            .saturating_add(src_line_bytes);
        if required > data.len() {
            return Err(HandleError::InvalidParameter);
        }

        let mut converted_line = vec![0u8; width as usize * dst_bytes_per_pixel];

        for row in 0..height {
            let src_off = row as usize * src_stride_bytes;
            let src_row = &data[src_off..src_off + src_line_bytes];

            for pixel in 0..width as usize {
                let src_pixel_offset = pixel * 4;
                let dst_pixel_offset = pixel * dst_bytes_per_pixel;
                let color = [
                    src_row[src_pixel_offset],
                    src_row[src_pixel_offset + 1],
                    src_row[src_pixel_offset + 2],
                    src_row[src_pixel_offset + 3],
                ];
                Self::write_packed_pixel_bytes(
                    &mut converted_line[dst_pixel_offset..dst_pixel_offset + dst_bytes_per_pixel],
                    color,
                    &var_info,
                );
            }

            let dst_y = y + row;
            let dst_off = (dst_y as usize)
                .saturating_mul(line_length)
                .saturating_add((x as usize).saturating_mul(dst_bytes_per_pixel));

            if let Some((mapped_addr, mapped_size)) = self.mapped_buffer {
                if dst_off.saturating_add(converted_line.len()) > mapped_size {
                    return Err(HandleError::InvalidParameter);
                }

                unsafe {
                    let dst_ptr = (mapped_addr + dst_off) as *mut u8;
                    core::ptr::copy_nonoverlapping(
                        converted_line.as_ptr(),
                        dst_ptr,
                        converted_line.len(),
                    );
                }
            } else {
                self.file
                    .seek(SeekFrom::Start(dst_off as u64))
                    .map_err(|_| HandleError::SystemError(-1))?;
                self.file
                    .write(&converted_line)
                    .map_err(|_| HandleError::SystemError(-1))?;
            }
        }

        Ok(())
    }

    /// Fill the entire screen with a solid color
    ///
    /// # Arguments
    /// * `color` - Color to fill [B, G, R, A]
    ///
    /// # Returns
    /// Success or HandleError on failure
    pub fn fill_screen(&mut self, color: [u8; 4]) -> HandleResult<()> {
        let var_info = self.get_var_screen_info()?;
        let fix_info = self.get_fix_screen_info()?;

        let width = var_info.xres as usize;
        let height = var_info.yres as usize;
        let line_length = fix_info.line_length as usize;

        // Create a line buffer filled with the color
        let mut line_buffer = vec![0u8; line_length];
        Self::populate_line_with_color(&mut line_buffer, width, color, &var_info);

        // Write the same line to all rows
        for y in 0..height {
            self.write_line(y as u32, &line_buffer)?;
        }

        Ok(())
    }

    /// Fill a rectangular area with a solid color
    ///
    /// # Arguments
    /// * `x` - X coordinate of the rectangle
    /// * `y` - Y coordinate of the rectangle
    /// * `width` - Width of the rectangle
    /// * `height` - Height of the rectangle
    /// * `color` - Color to fill [B, G, R, A]
    ///
    /// # Returns
    /// Success or HandleError on failure
    pub fn fill_rect(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        color: [u8; 4],
    ) -> HandleResult<()> {
        if width == 0 || height == 0 {
            return Ok(());
        }

        let var_info = self.get_var_screen_info()?;
        let fix_info = self.get_fix_screen_info()?;

        let bytes_per_pixel = Self::bytes_per_pixel(&var_info);
        let line_length = fix_info.line_length as usize;

        // Create a single-line buffer for the rectangle width.
        let line_bytes = width as usize * bytes_per_pixel;
        let mut line_buffer = vec![0u8; line_bytes];

        Self::populate_line_with_color(&mut line_buffer, width as usize, color, &var_info);

        if let Some((mapped_addr, mapped_size)) = self.mapped_buffer {
            for row in 0..height {
                let dst_y = y + row;
                let dst_off = (dst_y as usize)
                    .saturating_mul(line_length)
                    .saturating_add((x as usize).saturating_mul(bytes_per_pixel));
                if dst_off.saturating_add(line_bytes) > mapped_size {
                    return Err(HandleError::InvalidParameter);
                }
                unsafe {
                    let dst_ptr = (mapped_addr + dst_off) as *mut u8;
                    core::ptr::copy_nonoverlapping(line_buffer.as_ptr(), dst_ptr, line_bytes);
                }
            }
        } else {
            for row in 0..height {
                let dst_y = y + row;
                let dst_off = (dst_y as usize)
                    .saturating_mul(line_length)
                    .saturating_add((x as usize).saturating_mul(bytes_per_pixel));
                self.file
                    .seek(SeekFrom::Start(dst_off as u64))
                    .map_err(|_| HandleError::SystemError(-1))?;
                self.file
                    .write(&line_buffer)
                    .map_err(|_| HandleError::SystemError(-1))?;
            }
        }

        Ok(())
    }

    /// Create a horizontal gradient with specified colors
    ///
    /// # Arguments
    /// * `start_color` - Starting color [B, G, R, A]
    /// * `end_color` - Ending color [B, G, R, A]
    ///
    /// # Returns
    /// Success or HandleError on failure
    pub fn draw_horizontal_gradient(
        &mut self,
        start_color: [u8; 4],
        end_color: [u8; 4],
    ) -> HandleResult<()> {
        let var_info = self.get_var_screen_info()?;
        let width = var_info.xres as usize;
        let height = var_info.yres as usize;
        let bytes_per_pixel = Self::bytes_per_pixel(&var_info);

        // Create line buffer with horizontal gradient
        let line_bytes = width * bytes_per_pixel;
        let mut line_buffer = vec![0u8; line_bytes];

        for x in 0..width {
            let ratio = (x * 256) / width; // Fixed-point ratio (scaled by 256)
            let ratio_u16 = ratio as u16;
            let inv_ratio_u16 = (256 - ratio) as u16;
            let color = [
                ((start_color[0] as u16 * inv_ratio_u16 + end_color[0] as u16 * ratio_u16) / 256)
                    as u8,
                ((start_color[1] as u16 * inv_ratio_u16 + end_color[1] as u16 * ratio_u16) / 256)
                    as u8,
                ((start_color[2] as u16 * inv_ratio_u16 + end_color[2] as u16 * ratio_u16) / 256)
                    as u8,
                ((start_color[3] as u16 * inv_ratio_u16 + end_color[3] as u16 * ratio_u16) / 256)
                    as u8,
            ];

            let pixel_offset = x * bytes_per_pixel;
            if pixel_offset + bytes_per_pixel <= line_buffer.len() {
                Self::write_packed_pixel_bytes(
                    &mut line_buffer[pixel_offset..pixel_offset + bytes_per_pixel],
                    color,
                    &var_info,
                );
            }
        }

        // Write the same line to all rows
        for y in 0..height {
            self.write_line(y as u32, &line_buffer)?;
        }

        Ok(())
    }

    /// Create a vertical gradient with specified colors
    ///
    /// # Arguments
    /// * `start_color` - Starting color [B, G, R, A]
    /// * `end_color` - Ending color [B, G, R, A]
    ///
    /// # Returns
    /// Success or HandleError on failure
    pub fn draw_vertical_gradient(
        &mut self,
        start_color: [u8; 4],
        end_color: [u8; 4],
    ) -> HandleResult<()> {
        let var_info = self.get_var_screen_info()?;
        let width = var_info.xres as usize;
        let height = var_info.yres as usize;
        let bytes_per_pixel = Self::bytes_per_pixel(&var_info);

        // Create line buffer filled with this color
        let line_bytes = width * bytes_per_pixel;
        let mut line_buffer = vec![0u8; line_bytes];

        for y in 0..height {
            let scale_factor: u32 = 1000; // Scale factor for integer arithmetic
            let ratio: u32 = (y as u32 * scale_factor) / height as u32;
            let color = [
                ((start_color[0] as u32 * (scale_factor - ratio) + end_color[0] as u32 * ratio)
                    / scale_factor) as u8,
                ((start_color[1] as u32 * (scale_factor - ratio) + end_color[1] as u32 * ratio)
                    / scale_factor) as u8,
                ((start_color[2] as u32 * (scale_factor - ratio) + end_color[2] as u32 * ratio)
                    / scale_factor) as u8,
                ((start_color[3] as u32 * (scale_factor - ratio) + end_color[3] as u32 * ratio)
                    / scale_factor) as u8,
            ];

            for x in 0..width {
                let pixel_offset = x * bytes_per_pixel;
                if pixel_offset + bytes_per_pixel <= line_buffer.len() {
                    Self::write_packed_pixel_bytes(
                        &mut line_buffer[pixel_offset..pixel_offset + bytes_per_pixel],
                        color,
                        &var_info,
                    );
                }
            }

            self.write_line(y as u32, &line_buffer)?;
        }

        Ok(())
    }

    /// Draw a gradient rectangle with optimized block writing
    ///
    /// # Arguments
    /// * `x` - X coordinate of the rectangle
    /// * `y` - Y coordinate of the rectangle
    /// * `width` - Width of the rectangle
    /// * `height` - Height of the rectangle
    /// * `start_color` - Starting color [B, G, R, A]
    /// * `end_color` - Ending color [B, G, R, A]
    /// * `horizontal` - If true, gradient goes horizontally; if false, vertically
    ///
    /// # Returns
    /// Success or HandleError on failure
    pub fn draw_gradient_rect(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        start_color: [u8; 4],
        end_color: [u8; 4],
        horizontal: bool,
    ) -> HandleResult<()> {
        let var_info = self.get_var_screen_info()?;
        let bytes_per_pixel = Self::bytes_per_pixel(&var_info);

        if horizontal {
            // Horizontal gradient: create one line buffer and reuse it
            let line_bytes = width as usize * bytes_per_pixel;
            let mut line_buffer = vec![0u8; line_bytes];

            for px in 0..width as usize {
                let ratio = px as f32 / width as f32;
                let color = [
                    (start_color[0] as f32 * (1.0 - ratio) + end_color[0] as f32 * ratio) as u8,
                    (start_color[1] as f32 * (1.0 - ratio) + end_color[1] as f32 * ratio) as u8,
                    (start_color[2] as f32 * (1.0 - ratio) + end_color[2] as f32 * ratio) as u8,
                    (start_color[3] as f32 * (1.0 - ratio) + end_color[3] as f32 * ratio) as u8,
                ];

                let pixel_offset = px * bytes_per_pixel;
                if pixel_offset + bytes_per_pixel <= line_buffer.len() {
                    Self::write_packed_pixel_bytes(
                        &mut line_buffer[pixel_offset..pixel_offset + bytes_per_pixel],
                        color,
                        &var_info,
                    );
                }
            }

            // Write the same line to all rows
            self.write_block(x, y, width, height, &line_buffer)
        } else {
            // Vertical gradient: create each line individually
            for py in 0..height {
                let ratio = py as f32 / height as f32;
                let color = [
                    (start_color[0] as f32 * (1.0 - ratio) + end_color[0] as f32 * ratio) as u8,
                    (start_color[1] as f32 * (1.0 - ratio) + end_color[1] as f32 * ratio) as u8,
                    (start_color[2] as f32 * (1.0 - ratio) + end_color[2] as f32 * ratio) as u8,
                    (start_color[3] as f32 * (1.0 - ratio) + end_color[3] as f32 * ratio) as u8,
                ];

                // Fill line with solid color
                self.fill_rect(x, y + py, width, 1, color)?;
            }

            Ok(())
        }
    }
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        // Clean up memory mapping if it exists
        if let Some((mapped_addr, mapped_size)) = self.mapped_buffer {
            let _ = munmap(mapped_addr, mapped_size);
        }
        self.mapped_physical_addr = None;
    }
}

impl Drop for DisplaySurface {
    fn drop(&mut self) {
        if self.swapchain_buffers.is_empty() {
            if let Some((mapped_addr, mapped_size)) = self.mapped_buffer {
                let _ = munmap(mapped_addr, mapped_size);
            }
        } else {
            for (mapped_addr, mapped_size) in self.swapchain_buffers.drain(..) {
                let _ = munmap(mapped_addr, mapped_size);
            }
        }
    }
}
