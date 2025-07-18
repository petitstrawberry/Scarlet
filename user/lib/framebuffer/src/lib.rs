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
    handle::{HandleError, HandleResult},
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
            red: FbBitfield { offset: 16, length: 8, msb_right: 0 },
            green: FbBitfield { offset: 8, length: 8, msb_right: 0 },
            blue: FbBitfield { offset: 0, length: 8, msb_right: 0 },
            transp: FbBitfield { offset: 24, length: 8, msb_right: 0 },
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
#[derive(Debug, Clone)]
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

impl Default for FbFixScreenInfo {
    fn default() -> Self {
        Self {
            id: [0; 16],
            smem_start: 0,
            smem_len: 0,
            type_: 0,
            type_aux: 0,
            visual: 0,
            xpanstep: 0,
            ypanstep: 0,
            ywrapstep: 0,
            line_length: 0,
            mmio_start: 0,
            mmio_len: 0,
            accel: 0,
            capabilities: 0,
            reserved: [0; 2],
        }
    }
}

/// Framebuffer device wrapper
/// 
/// Wraps a File handle to provide framebuffer-specific control operations.
pub struct Framebuffer {
    file: File,
}

impl Framebuffer {
    /// Open a framebuffer device
    /// 
    /// # Arguments
    /// * `path` - Path to the framebuffer device (e.g., "/dev/fb0")
    /// 
    /// # Returns
    /// Framebuffer instance or HandleError on failure
    pub fn open(path: &str) -> HandleResult<Self> {
        let file = File::open(path).map_err(|_| HandleError::NotFound)?;
        Ok(Self { file })
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
        let ptr = &mut fix_info as *mut _ as usize;
        if ptr.is_null() {
            return Err(HandleError::InvalidPointer);
        }
        self.file.as_handle().control(
            commands::FBIOGET_FSCREENINFO,
            ptr,
        )?;
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
        if !is_valid_pointer(var_info as *const _ as *const u8) {
            return Err(HandleError::InvalidPointer);
        }
        self.file.as_handle().control(
            commands::FBIOPUT_VSCREENINFO,
            var_info as *const _ as usize,
        )?;
        Ok(())
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
        
        let bytes_per_pixel = (var_info.bits_per_pixel / 8) as usize;
        let line_length = fix_info.line_length as usize;
        
        // Calculate pixel offset
        let offset = y as usize * line_length + x as usize * bytes_per_pixel;
        
        // Seek to pixel position
        self.file.seek(SeekFrom::Start(offset as u64))
            .map_err(|_| HandleError::SystemError(-1))?;
        
        // Write pixel data
        let write_len = bytes_per_pixel.min(4);
        self.file.write(&color[..write_len])
            .map_err(|_| HandleError::SystemError(-1))?;
        
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
        
        // Seek to line start
        self.file.seek(SeekFrom::Start(offset as u64))
            .map_err(|_| HandleError::SystemError(-1))?;
        
        // Write entire line at once
        let write_len = data.len().min(line_length);
        self.file.write(&data[..write_len])
            .map_err(|_| HandleError::SystemError(-1))?;
        
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
    pub fn write_block(&mut self, x: u32, y: u32, width: u32, height: u32, data: &[u8]) -> HandleResult<()> {
        let var_info = self.get_var_screen_info()?;
        let fix_info = self.get_fix_screen_info()?;
        
        let bytes_per_pixel = (var_info.bits_per_pixel / 8) as usize;
        let line_length = fix_info.line_length as usize;
        let block_line_bytes = width as usize * bytes_per_pixel;
        
        // Write line by line
        for row in 0..height {
            let line_y = y + row;
            let line_offset = line_y as usize * line_length + x as usize * bytes_per_pixel;
            let data_offset = row as usize * block_line_bytes;
            
            // Seek to start of this line in the block
            self.file.seek(SeekFrom::Start(line_offset as u64))
                .map_err(|_| HandleError::SystemError(-1))?;
            
            // Write one line of the block
            let data_end = data_offset + block_line_bytes;
            if data_end <= data.len() {
                self.file.write(&data[data_offset..data_end])
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
        let bytes_per_pixel = (var_info.bits_per_pixel / 8) as usize;
        let line_length = fix_info.line_length as usize;
        
        // Create a line buffer filled with the color
        let mut line_buffer = vec![0u8; line_length];
        
        // Fill line buffer with repeated color pattern
        for x in 0..width {
            let pixel_offset = x * bytes_per_pixel;
            if pixel_offset + bytes_per_pixel <= line_buffer.len() {
                line_buffer[pixel_offset..pixel_offset + bytes_per_pixel.min(4)]
                    .copy_from_slice(&color[..bytes_per_pixel.min(4)]);
            }
        }
        
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
    pub fn fill_rect(&mut self, x: u32, y: u32, width: u32, height: u32, color: [u8; 4]) -> HandleResult<()> {
        let var_info = self.get_var_screen_info()?;
        let bytes_per_pixel = (var_info.bits_per_pixel / 8) as usize;
        
        // Create a line buffer for the rectangle width
        let line_bytes = width as usize * bytes_per_pixel;
        let mut line_buffer = vec![0u8; line_bytes];
        
        // Fill line buffer with repeated color pattern
        for pixel in 0..width as usize {
            let pixel_offset = pixel * bytes_per_pixel;
            if pixel_offset + bytes_per_pixel <= line_buffer.len() {
                line_buffer[pixel_offset..pixel_offset + bytes_per_pixel.min(4)]
                    .copy_from_slice(&color[..bytes_per_pixel.min(4)]);
            }
        }
        
        // Use write_block for efficiency
        self.write_block(x, y, width, height, &line_buffer)
    }

    /// Create a horizontal gradient with specified colors
    /// 
    /// # Arguments
    /// * `start_color` - Starting color [B, G, R, A]
    /// * `end_color` - Ending color [B, G, R, A]
    /// 
    /// # Returns
    /// Success or HandleError on failure
    pub fn draw_horizontal_gradient(&mut self, start_color: [u8; 4], end_color: [u8; 4]) -> HandleResult<()> {
        let var_info = self.get_var_screen_info()?;
        let width = var_info.xres as usize;
        let height = var_info.yres as usize;
        let bytes_per_pixel = (var_info.bits_per_pixel / 8) as usize;
        
        // Create line buffer with horizontal gradient
        let line_bytes = width * bytes_per_pixel;
        let mut line_buffer = vec![0u8; line_bytes];
        
        for x in 0..width {
            let ratio = (x * 256) / width; // Fixed-point ratio (scaled by 256)
            let color = [
                ((start_color[0] as u16 * (256 - ratio) + end_color[0] as u16 * ratio) / 256) as u8,
                ((start_color[1] as u16 * (256 - ratio) + end_color[1] as u16 * ratio) / 256) as u8,
                ((start_color[2] as u16 * (256 - ratio) + end_color[2] as u16 * ratio) / 256) as u8,
                ((start_color[3] as u16 * (256 - ratio) + end_color[3] as u16 * ratio) / 256) as u8,
            ];
            
            let pixel_offset = x * bytes_per_pixel;
            if pixel_offset + bytes_per_pixel <= line_buffer.len() {
                line_buffer[pixel_offset..pixel_offset + bytes_per_pixel.min(4)]
                    .copy_from_slice(&color[..bytes_per_pixel.min(4)]);
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
    pub fn draw_vertical_gradient(&mut self, start_color: [u8; 4], end_color: [u8; 4]) -> HandleResult<()> {
        let var_info = self.get_var_screen_info()?;
        let width = var_info.xres as usize;
        let height = var_info.yres as usize;
        let bytes_per_pixel = (var_info.bits_per_pixel / 8) as usize;
        
        for y in 0..height {
            let scale_factor = 1000; // Scale factor for integer arithmetic
            let ratio = (y * scale_factor) / height;
            let color = [
                ((start_color[0] as u16 * (scale_factor - ratio) + end_color[0] as u16 * ratio) / scale_factor) as u8,
                ((start_color[1] as u16 * (scale_factor - ratio) + end_color[1] as u16 * ratio) / scale_factor) as u8,
                ((start_color[2] as u16 * (scale_factor - ratio) + end_color[2] as u16 * ratio) / scale_factor) as u8,
                ((start_color[3] as u16 * (scale_factor - ratio) + end_color[3] as u16 * ratio) / scale_factor) as u8,
            ];
            
            // Create line buffer filled with this color
            let line_bytes = width * bytes_per_pixel;
            let mut line_buffer = vec![0u8; line_bytes];
            
            for x in 0..width {
                let pixel_offset = x * bytes_per_pixel;
                if pixel_offset + bytes_per_pixel <= line_buffer.len() {
                    line_buffer[pixel_offset..pixel_offset + bytes_per_pixel.min(4)]
                        .copy_from_slice(&color[..bytes_per_pixel.min(4)]);
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
    pub fn draw_gradient_rect(&mut self, x: u32, y: u32, width: u32, height: u32, 
                             start_color: [u8; 4], end_color: [u8; 4], horizontal: bool) -> HandleResult<()> {
        let var_info = self.get_var_screen_info()?;
        let bytes_per_pixel = (var_info.bits_per_pixel / 8) as usize;
        
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
                    line_buffer[pixel_offset..pixel_offset + bytes_per_pixel.min(4)]
                        .copy_from_slice(&color[..bytes_per_pixel.min(4)]);
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
