//! Optional GPU-backed scene composition for SWS.

use super::cursor::Cursor;
use super::window::{Window, WindowId};
use framebuffer::DisplaySurface;
use sgfx::{
    Color, CompositionPass, Context, Device, Image, PixelRect, Queue, SourceAlpha, Texture,
};
use std::vec::Vec;

type DamageRect = (u32, u32, u32, u32);

enum WindowTextureBacking {
    Imported,
    Private,
}

struct CachedWindowTexture {
    window_id: WindowId,
    width: u32,
    height: u32,
    texture: Texture,
    backing: WindowTextureBacking,
    pending_damage: Option<DamageRect>,
}

/// GPU resources and texture cache used by the internal SWS compositor.
pub(super) struct GpuCompositor {
    _context: Context,
    queue: Queue,
    target: Image,
    cursor_texture: Texture,
    cursor_width: u32,
    cursor_height: u32,
    textures: Vec<CachedWindowTexture>,
}

impl GpuCompositor {
    /// Create the optional GPU compositor after confirming required capabilities.
    pub(super) fn new(width: u32, height: u32, cursor: &Cursor) -> Result<Self, &'static str> {
        let device = Device::open("/dev/gpu0").map_err(|_| "Failed to open GPU")?;
        let capabilities = device.capabilities();
        if !capabilities.supports_rendering()
            || !capabilities.supports_presentation()
            || !capabilities.supports_image_upload()
        {
            return Err("GPU lacks SWS composition capabilities");
        }

        let context = device
            .create_context()
            .map_err(|_| "Failed to create GPU context")?;
        let queue = context
            .create_queue()
            .map_err(|_| "Failed to create GPU queue")?;
        let target = context
            .create_image(width, height)
            .map_err(|_| "Failed to create GPU render target")?;
        let cursor_texture = context
            .create_sampled_bgra_texture(cursor.width, cursor.height)
            .map_err(|_| "Failed to create GPU cursor texture")?;
        let cursor_pixels = cursor.bgra_pixels();
        context
            .upload_texture_bgra(
                &cursor_texture,
                &cursor_pixels,
                cursor.width.saturating_mul(4),
                PixelRect::new(0, 0, cursor.width, cursor.height),
            )
            .map_err(|_| "Failed to upload GPU cursor texture")?;

        Ok(Self {
            _context: context,
            queue,
            target,
            cursor_texture,
            cursor_width: cursor.width,
            cursor_height: cursor.height,
            textures: Vec::new(),
        })
    }

    /// Recreate the screen-sized target after an output resize.
    pub(super) fn resize_target(&mut self, width: u32, height: u32) -> Result<(), &'static str> {
        self.target = self
            ._context
            .create_image(width, height)
            .map_err(|_| "Failed to resize GPU render target")?;
        Ok(())
    }

    /// Mark a local window rectangle for texture upload before the next frame.
    pub(super) fn mark_window_damage(
        &mut self,
        window_id: WindowId,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) {
        if width == 0 || height == 0 {
            return;
        }
        let Some(entry) = self
            .textures
            .iter_mut()
            .find(|entry| entry.window_id == window_id)
        else {
            return;
        };
        let left = i64::from(x).max(0).min(i64::from(entry.width));
        let top = i64::from(y).max(0).min(i64::from(entry.height));
        let right = i64::from(x)
            .saturating_add(i64::from(width))
            .max(0)
            .min(i64::from(entry.width));
        let bottom = i64::from(y)
            .saturating_add(i64::from(height))
            .max(0)
            .min(i64::from(entry.height));
        if right <= left || bottom <= top {
            return;
        }
        let x = left as u32;
        let y = top as u32;
        let width = (right - left) as u32;
        let height = (bottom - top) as u32;
        entry.pending_damage = Some(match entry.pending_damage {
            Some((old_x, old_y, old_width, old_height)) => {
                let right = old_x.saturating_add(old_width).max(x.saturating_add(width));
                let bottom = old_y
                    .saturating_add(old_height)
                    .max(y.saturating_add(height));
                (
                    old_x.min(x),
                    old_y.min(y),
                    right - old_x.min(x),
                    bottom - old_y.min(y),
                )
            }
            None => (x, y, width, height),
        });
    }

    /// Detach and forget a texture before its backing store changes or disappears.
    pub(super) fn remove_window(&mut self, window_id: WindowId) -> Result<(), &'static str> {
        if let Some(index) = self
            .textures
            .iter()
            .position(|entry| entry.window_id == window_id)
        {
            self.release_window_texture(index)?;
        }
        Ok(())
    }

    /// Upload all changed window textures, compose the complete scene, and present it.
    pub(super) fn compose_and_present(
        &mut self,
        display: &DisplaySurface,
        windows: &[Window],
        cursor: &Cursor,
        background: [u8; 4],
        resize_outline: Option<(i32, i32, u32, u32)>,
    ) -> Result<(), &'static str> {
        for window in windows {
            if window.visible && (window.has_pixel_buffer() || window.shm_layout().is_ok()) {
                self.sync_window_texture(window)?;
            }
        }

        let clear_color = bgra_color(background);
        let mut composition = CompositionPass::new(&self.target, clear_color)
            .map_err(|_| "Failed to begin GPU composition")?;
        for window in windows {
            if !window.visible {
                continue;
            }
            let Some((destination, source)) = clipped_rect(
                window.x,
                window.y,
                window.width,
                window.height,
                self.target.width(),
                self.target.height(),
            ) else {
                continue;
            };

            if window.has_pixel_buffer() || window.shm_layout().is_ok() {
                let texture = self
                    .textures
                    .iter()
                    .find(|entry| entry.window_id == window.id)
                    .map(|entry| &entry.texture)
                    .ok_or("GPU window texture cache is missing")?;
                let source_alpha = if window.has_alpha_content {
                    SourceAlpha::Respect
                } else {
                    SourceAlpha::Ignore
                };
                composition
                    .draw_textured_rect(
                        texture,
                        destination,
                        source,
                        window.opacity,
                        source_alpha,
                        None,
                    )
                    .map_err(|_| "Failed to compose GPU window texture")?;
            } else {
                let color = if window.focused {
                    bgra_color([150, 150, 200, 255])
                } else {
                    bgra_color([180, 180, 180, 255])
                };
                composition
                    .draw_solid_rect(destination, color, None)
                    .map_err(|_| "Failed to compose GPU window placeholder")?;
            }
        }

        if let Some(outline) = resize_outline {
            append_outline(
                &mut composition,
                outline,
                self.target.width(),
                self.target.height(),
            )?;
        }
        if let Some((destination, source)) = clipped_rect(
            cursor.x,
            cursor.y,
            self.cursor_width,
            self.cursor_height,
            self.target.width(),
            self.target.height(),
        ) {
            composition
                .draw_textured_rect(
                    &self.cursor_texture,
                    destination,
                    source,
                    1.0,
                    SourceAlpha::Respect,
                    None,
                )
                .map_err(|_| "Failed to compose GPU cursor")?;
        }

        self.queue
            .submit_composition(&composition)
            .map_err(|_| "Failed to submit GPU composition")?;
        self.target
            .present(display)
            .map_err(|_| "Failed to present GPU composition")
    }

    fn sync_window_texture(&mut self, window: &Window) -> Result<(), &'static str> {
        let imported_layout = window.shm_layout().ok();
        let pixels = window.pixels().ok();
        let (width, height) = match imported_layout.as_ref() {
            Some(layout) => (layout.width(), layout.height()),
            None => {
                let pixels = pixels
                    .as_ref()
                    .ok_or("Window has no importable or CPU-readable pixel backing")?;
                (pixels.width(), pixels.height())
            }
        };
        let matching_index = self
            .textures
            .iter()
            .position(|entry| entry.window_id == window.id);
        let texture_index = match matching_index {
            Some(index)
                if self.textures[index].width == width && self.textures[index].height == height =>
            {
                index
            }
            Some(index) => {
                self.release_window_texture(index)?;
                self.create_window_texture(window, width, height)?
            }
            None => self.create_window_texture(window, width, height)?,
        };
        let entry = &mut self.textures[texture_index];
        let Some(damage) = entry.pending_damage else {
            return Ok(());
        };
        let damage_rect = PixelRect::new(damage.0, damage.1, damage.2, damage.3);
        let transfer_result = match &entry.backing {
            WindowTextureBacking::Imported => self
                ._context
                .transfer_imported_bgra_rect(&entry.texture, damage_rect)
                .map_err(|_| "Failed to transfer imported GPU window texture"),
            WindowTextureBacking::Private => {
                let pixels = pixels
                    .as_ref()
                    .ok_or("Fallback GPU texture has no CPU-readable pixel backing")?;
                let source = pixels.damage_bytes(damage.0, damage.1, damage.2, damage.3)?;
                self._context
                    .upload_texture_bgra(&entry.texture, source, pixels.stride(), damage_rect)
                    .map_err(|_| "Failed to upload GPU window texture")
            }
        };
        transfer_result?;
        entry.pending_damage = None;
        Ok(())
    }

    fn create_window_texture(
        &mut self,
        window: &Window,
        width: u32,
        height: u32,
    ) -> Result<usize, &'static str> {
        let (texture, backing) = match window.shm_layout() {
            Ok(layout) if layout.size() != 0 && layout.format() == 0 => {
                match self._context.create_imported_bgra_texture(
                    layout.shared_memory(),
                    layout.width(),
                    layout.height(),
                    layout.offset(),
                    layout.stride(),
                ) {
                    Ok(texture) => (texture, WindowTextureBacking::Imported),
                    Err(_) => (
                        self._context
                            .create_sampled_bgra_texture(width, height)
                            .map_err(|_| "Failed to create fallback GPU window texture")?,
                        WindowTextureBacking::Private,
                    ),
                }
            }
            Err(_) => (
                self._context
                    .create_sampled_bgra_texture(width, height)
                    .map_err(|_| "Failed to create GPU window texture")?,
                WindowTextureBacking::Private,
            ),
            Ok(_) => (
                self._context
                    .create_sampled_bgra_texture(width, height)
                    .map_err(|_| "Failed to create fallback GPU window texture")?,
                WindowTextureBacking::Private,
            ),
        };
        self.textures.push(CachedWindowTexture {
            window_id: window.id,
            width,
            height,
            texture,
            backing,
            pending_damage: Some((0, 0, width, height)),
        });
        Ok(self.textures.len() - 1)
    }

    fn release_window_texture(&mut self, index: usize) -> Result<(), &'static str> {
        let entry = self.textures.remove(index);
        self._context
            .release_texture(entry.texture)
            .map_err(|_| "Failed to detach GPU window texture")
    }
}

fn bgra_color(color: [u8; 4]) -> Color {
    Color::rgba(
        color[2] as f32 / 255.0,
        color[1] as f32 / 255.0,
        color[0] as f32 / 255.0,
        color[3] as f32 / 255.0,
    )
}

fn clipped_rect(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    target_width: u32,
    target_height: u32,
) -> Option<(PixelRect, PixelRect)> {
    let right = x.checked_add(width as i32)?;
    let bottom = y.checked_add(height as i32)?;
    let left = x.max(0).min(target_width as i32);
    let top = y.max(0).min(target_height as i32);
    let right = right.max(0).min(target_width as i32);
    let bottom = bottom.max(0).min(target_height as i32);
    if right <= left || bottom <= top {
        return None;
    }
    let width = (right - left) as u32;
    let height = (bottom - top) as u32;
    Some((
        PixelRect::new(left as u32, top as u32, width, height),
        PixelRect::new((left - x) as u32, (top - y) as u32, width, height),
    ))
}

fn append_outline(
    composition: &mut CompositionPass<'_>,
    rect: (i32, i32, u32, u32),
    target_width: u32,
    target_height: u32,
) -> Result<(), &'static str> {
    append_outline_ring(
        composition,
        rect,
        bgra_color([0, 0, 0, 255]),
        target_width,
        target_height,
    )?;
    if rect.2 > 2 && rect.3 > 2 {
        append_outline_ring(
            composition,
            (rect.0 + 1, rect.1 + 1, rect.2 - 2, rect.3 - 2),
            bgra_color([255, 255, 255, 255]),
            target_width,
            target_height,
        )?;
    }
    Ok(())
}

fn append_outline_ring(
    composition: &mut CompositionPass<'_>,
    rect: (i32, i32, u32, u32),
    color: Color,
    target_width: u32,
    target_height: u32,
) -> Result<(), &'static str> {
    let (x, y, width, height) = rect;
    let edges = [
        (x, y, width, 1),
        (x, y.saturating_add(height as i32 - 1), width, 1),
        (x, y, 1, height),
        (x.saturating_add(width as i32 - 1), y, 1, height),
    ];
    for (x, y, width, height) in edges {
        if let Some((destination, _)) =
            clipped_rect(x, y, width, height, target_width, target_height)
        {
            composition
                .draw_solid_rect(destination, color, None)
                .map_err(|_| "Failed to compose GPU resize outline")?;
        }
    }
    Ok(())
}
