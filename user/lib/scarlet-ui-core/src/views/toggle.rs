//! Toggle View - On/off switch control
//!
//! Toggle is a switch control that can be on or off.

use crate::buffer::Buffer;
use crate::color::{Color, ColorPalette};
use crate::element::{Element, ElementRenderObject, RenderElement};
use crate::geometry::Size;
use crate::graphics;
use crate::state::State;
use crate::view::View;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::any::Any;

/// Toggle View - on/off switch
#[derive(Clone)]
pub struct Toggle {
    is_on: State<bool>,
}

impl Toggle {
    /// Create a new Toggle
    pub fn new(is_on: State<bool>) -> Self {
        Self { is_on }
    }

    /// Get the is_on state
    pub fn get_is_on(&self) -> &State<bool> {
        &self.is_on
    }
}

impl View for Toggle {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            ToggleRenderObject::new(self.is_on.get()),
        ))
    }

    fn listenables(&self) -> Vec<&dyn crate::state::Listenable> {
        alloc::vec![&self.is_on]
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Toggle RenderObject
///
/// Design matching iOS/Android switch:
/// - Width: 51px, Height: 31px
/// - On: Green background (#34C759)
/// - Off: Gray background (#767680)
/// - Thumb: White circle, 27px diameter
/// - Corner radius: 15.5px (half height)
pub struct ToggleRenderObject {
    is_on: bool,
    size: Size,
    buffer: Option<Buffer>,
}

impl ToggleRenderObject {
    fn fill_rounded_rect(
        canvas: &mut graphics::Canvas<'_>,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        radius: u32,
        color: Color,
    ) {
        let r = radius as i32;
        if r <= 0 {
            canvas.fill_rect(x, y, width, height, color);
            return;
        }

        let w = width as i32;
        let h = height as i32;
        let r_max = (width.min(height) / 2) as i32;
        let r = r.min(r_max);
        let r_sq = (r - 1).max(0);
        let r_sq = r_sq * r_sq;

        for py in 0..h {
            for px in 0..w {
                let mut inside = true;

                if px < r && py < r {
                    let dx = px - (r - 1);
                    let dy = py - (r - 1);
                    inside = dx * dx + dy * dy <= r_sq;
                } else if px >= w - r && py < r {
                    let dx = px - (w - r);
                    let dy = py - (r - 1);
                    inside = dx * dx + dy * dy <= r_sq;
                } else if px < r && py >= h - r {
                    let dx = px - (r - 1);
                    let dy = py - (h - r);
                    inside = dx * dx + dy * dy <= r_sq;
                } else if px >= w - r && py >= h - r {
                    let dx = px - (w - r);
                    let dy = py - (h - r);
                    inside = dx * dx + dy * dy <= r_sq;
                }

                if inside {
                    canvas.put_pixel(x + px, y + py, color);
                }
            }
        }
    }

    fn fill_circle(
        canvas: &mut graphics::Canvas<'_>,
        center_x: i32,
        center_y: i32,
        radius: i32,
        color: Color,
    ) {
        if radius <= 0 {
            return;
        }
        let r_sq = radius * radius;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy <= r_sq {
                    canvas.put_pixel(center_x + dx, center_y + dy, color);
                }
            }
        }
    }

    fn downsample_2x(
        src: &[u8],
        src_width: u32,
        src_height: u32,
        dst: &mut [u8],
        dst_width: u32,
        dst_height: u32,
    ) {
        if src_width != dst_width * 2 || src_height != dst_height * 2 {
            return;
        }

        for y in 0..dst_height {
            for x in 0..dst_width {
                let sx = x * 2;
                let sy = y * 2;
                let mut sum = [0u32; 4];

                for oy in 0..2 {
                    for ox in 0..2 {
                        let idx = ((sy + oy) * src_width + (sx + ox)) as usize * 4;
                        sum[0] += src[idx] as u32;
                        sum[1] += src[idx + 1] as u32;
                        sum[2] += src[idx + 2] as u32;
                        sum[3] += src[idx + 3] as u32;
                    }
                }

                let dst_idx = (y * dst_width + x) as usize * 4;
                dst[dst_idx] = (sum[0] / 4) as u8;
                dst[dst_idx + 1] = (sum[1] / 4) as u8;
                dst[dst_idx + 2] = (sum[2] / 4) as u8;
                dst[dst_idx + 3] = (sum[3] / 4) as u8;
            }
        }
    }

    fn blend_bgra_over(dst: &mut [u8], src: &[u8], width: u32, height: u32) {
        for y in 0..height {
            for x in 0..width {
                let idx = ((y * width + x) * 4) as usize;
                let bgra = u32::from_le_bytes([src[idx], src[idx + 1], src[idx + 2], src[idx + 3]]);
                let color = Color::from_bgra(bgra);
                if color.a <= 0.0 {
                    continue;
                }
                let dst_bgra =
                    u32::from_le_bytes([dst[idx], dst[idx + 1], dst[idx + 2], dst[idx + 3]]);
                let dst_color = Color::from_bgra(dst_bgra);
                let out = color.blend_over(dst_color);
                let out_bytes = out.to_bgra().to_le_bytes();
                dst[idx..idx + 4].copy_from_slice(&out_bytes);
            }
        }
    }

    /// Create a new ToggleRenderObject
    pub fn new(is_on: bool) -> Self {
        // iOS-style toggle dimensions
        const TOGGLE_WIDTH: f32 = 51.0;
        const TOGGLE_HEIGHT: f32 = 31.0;

        Self {
            is_on,
            size: Size::new(TOGGLE_WIDTH, TOGGLE_HEIGHT),
            buffer: None,
        }
    }

    /// Get is_on state
    pub fn get_is_on(&self) -> bool {
        self.is_on
    }

    /// Set is_on state
    pub fn set_is_on(&mut self, is_on: bool) {
        self.is_on = is_on;
    }

    /// Draw toggle using Canvas API (iOS-style design)
    fn draw_toggle(&mut self) {
        let width = libm::ceilf(self.size.width) as usize;
        let height = libm::ceilf(self.size.height) as usize;
        let w = width as u32;
        let h = height as u32;

        // Create or resize buffer
        let needs_resize = self
            .buffer
            .as_ref()
            .map_or(true, |b| b.logical_width() != w || b.logical_height() != h);
        if needs_resize {
            self.buffer = Some(Buffer::from_logical_dimensions(w, h));
        }

        if let Some(ref mut buffer) = self.buffer {
            let physical_w = buffer.width();
            let physical_h = buffer.height();
            let ui_scale = (buffer.scale_milli() as f32) / 1000.0;
            let palette = ColorPalette::default();
            let bg_color = if self.is_on {
                palette.green().base
            } else {
                palette.surface_variant().darken(0.06)
            };
            let border_color = palette.border().with_opacity(0.7);
            let thumb_color = palette.surface();

            let data = buffer.data_mut();
            data.fill(0);

            let aa_scale = 2u32;
            let w_hi = physical_w * aa_scale;
            let h_hi = physical_h * aa_scale;
            let mut track_hi = alloc::vec![0u8; (w_hi * h_hi * 4) as usize];
            let mut canvas_hi = graphics::Canvas::new(&mut track_hi, w_hi, h_hi);
            let radius_hi = (h_hi / 2).max(1);
            Self::fill_rounded_rect(&mut canvas_hi, 0, 0, w_hi, h_hi, radius_hi, border_color);

            let inset = aa_scale;
            let inner_w = w_hi.saturating_sub(inset * 2);
            let inner_h = h_hi.saturating_sub(inset * 2);
            if inner_w > 0 && inner_h > 0 {
                let radius_inner = radius_hi.saturating_sub(inset);
                Self::fill_rounded_rect(
                    &mut canvas_hi,
                    inset as i32,
                    inset as i32,
                    inner_w,
                    inner_h,
                    radius_inner,
                    bg_color,
                );
            }

            let mut track = alloc::vec![0u8; (physical_w * physical_h * 4) as usize];
            Self::downsample_2x(&track_hi, w_hi, h_hi, &mut track, physical_w, physical_h);
            data.copy_from_slice(&track);

            // Thumb position: on = right side, off = left side
            let thumb_diameter = self.size.height - 4.0;
            let thumb_offset = if self.is_on {
                self.size.width - self.size.height
            } else {
                0.0
            };
            let thumb_x = ((thumb_offset + 2.0) * ui_scale * aa_scale as f32) as i32;
            let thumb_y = (2.0 * ui_scale * aa_scale as f32) as i32;
            let thumb_size = libm::ceilf(thumb_diameter * ui_scale * aa_scale as f32) as i32;
            let radius = (thumb_size / 2).max(1);
            let center_x = thumb_x + radius;
            let center_y = thumb_y + radius;
            let mut thumb_hi = alloc::vec![0u8; (w_hi * h_hi * 4) as usize];
            let mut thumb_canvas = graphics::Canvas::new(&mut thumb_hi, w_hi, h_hi);
            Self::fill_circle(&mut thumb_canvas, center_x, center_y, radius, thumb_color);

            let mut thumb = alloc::vec![0u8; (physical_w * physical_h * 4) as usize];
            Self::downsample_2x(&thumb_hi, w_hi, h_hi, &mut thumb, physical_w, physical_h);
            Self::blend_bgra_over(data, &thumb, physical_w, physical_h);
        }
    }
}

impl ElementRenderObject for ToggleRenderObject {
    fn layout(&mut self, _constraints: crate::element::LayoutConstraints) -> Size {
        // Toggle has fixed size (51x31), ignore constraints
        let width = self.size.width; // 51.0
        let height = self.size.height; // 31.0

        self.size = Size { width, height };

        // Create buffer
        let w = libm::ceilf(width) as u32;
        let h = libm::ceilf(height) as u32;
        let needs_resize = self
            .buffer
            .as_ref()
            .map_or(true, |b| b.logical_width() != w || b.logical_height() != h);
        if needs_resize {
            self.buffer = Some(Buffer::from_logical_dimensions(w, h));
        }

        self.size
    }

    fn size(&self) -> Size {
        self.size
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn render(&mut self) {
        self.draw_toggle();
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn clear_buffer(&mut self) {
        self.buffer = None;
    }

    fn update(&mut self, new_view: &dyn crate::view::View) -> crate::element::UpdateResult {
        if let Some(toggle) = new_view.as_any().downcast_ref::<Toggle>() {
            let new_is_on = toggle.is_on.get();
            if self.is_on != new_is_on {
                self.is_on = new_is_on;
                crate::element::UpdateResult::Updated
            } else {
                crate::element::UpdateResult::NoChange
            }
        } else {
            crate::element::UpdateResult::Replaced
        }
    }
}
