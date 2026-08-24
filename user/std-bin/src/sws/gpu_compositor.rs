//! Optional GPU-backed scene composition for SWS.

use super::cursor::Cursor;
use super::window::{Window, WindowId};
use framebuffer::{DisplayPresentRegion, DisplaySurface};
use scarlet_os::handle::Handle;
use sgfx::ir::{Color, LoadOp, PixelRect, TextureId};
use std::vec::Vec;

use crate::sgfx_ir_support::{
    MappedTarget, Quad, QuadRenderer, SampledRect, define_bgra_texture, upload_bgra,
};

type DamageRect = (u32, u32, u32, u32);

/// Complete identity of one client-owned shared SGFX buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SgfxBufferIdentity {
    pub(super) window_id: u32,
    pub(super) buffer_id: u32,
    pub(super) generation: u32,
    pub(super) compositor_epoch: u32,
}

/// One exact accepted use of a shared SGFX buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SgfxCommitToken {
    pub(super) identity: SgfxBufferIdentity,
    pub(super) commit_serial: u64,
}

/// Typed failure returned by the shared SGFX buffer state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SgfxBufferError {
    Unavailable,
    InvalidBuffer,
    StaleGeneration,
    BufferBusy,
    ImportFailed,
}

struct CachedWindowTexture {
    window_id: WindowId,
    width: u32,
    height: u32,
    texture: TextureId,
    pending_damage: Option<DamageRect>,
}

/// GPU resources and texture cache used by the internal SWS compositor.
pub(super) struct GpuCompositor {
    target: MappedTarget,
    quad_renderer: QuadRenderer,
    cursor_textures: Vec<TextureId>,
    cursor_width: u32,
    cursor_height: u32,
    cursor_texture_generation: u64,
    textures: Vec<CachedWindowTexture>,
    rebuild_pending: bool,
    rebuild_extent: Option<(u32, u32)>,
    force_full_repaint: bool,
}

impl GpuCompositor {
    /// Create the optional GPU compositor after confirming required capabilities.
    pub(super) fn new(width: u32, height: u32, cursor: &Cursor) -> Result<Self, &'static str> {
        let mut target =
            MappedTarget::open(width, height).map_err(|_| "Failed to create mapped GPU target")?;
        let quad_renderer = QuadRenderer::define(target.resources.as_ref(), 96)
            .map_err(|_| "Failed to define GPU composition resources")?;
        let cursor_textures = create_cursor_textures(&mut target, cursor)?;

        Ok(Self {
            target,
            quad_renderer,
            cursor_textures,
            cursor_width: cursor.width,
            cursor_height: cursor.height,
            cursor_texture_generation: cursor.texture_generation(),
            textures: Vec::new(),
            rebuild_pending: false,
            rebuild_extent: None,
            force_full_repaint: false,
        })
    }

    /// Import one client-owned shared image into the compositor context.
    pub(super) fn register_shared_buffer(
        &mut self,
        identity: SgfxBufferIdentity,
        width: u32,
        height: u32,
        handle: Handle,
    ) -> Result<(), SgfxBufferError> {
        if width == 0 || height == 0 {
            return Err(SgfxBufferError::InvalidBuffer);
        }
        let _ = (identity, handle);
        Err(SgfxBufferError::ImportFailed)
    }

    /// Atomically publish a registered buffer and bounded damage list.
    pub(super) fn commit_shared_buffer(
        &mut self,
        identity: SgfxBufferIdentity,
        commit_serial: u64,
        damage_rects: &[sws_protocol::SgfxDamageRect],
    ) -> Result<Vec<DamageRect>, SgfxBufferError> {
        let _ = (identity, commit_serial, damage_rects);
        Err(SgfxBufferError::InvalidBuffer)
    }

    /// Remove a registered shared buffer that is not retained by SWS.
    pub(super) fn destroy_shared_buffer(
        &mut self,
        identity: SgfxBufferIdentity,
    ) -> Result<(), SgfxBufferError> {
        let _ = identity;
        Err(SgfxBufferError::InvalidBuffer)
    }

    /// Rebuild private composition resources before the next output frame.
    pub(super) fn resize_target(&mut self, width: u32, height: u32) -> Result<(), &'static str> {
        if width == 0 || height == 0 {
            return Err("Invalid GPU composition target extent");
        }
        self.rebuild_pending = true;
        self.rebuild_extent = Some((width, height));
        Ok(())
    }

    /// Force the next composition to rebuild its private resource table.
    ///
    /// This is required when a complete theme replacement starts its cursor
    /// generation counter from the same value as the preceding theme.
    pub(super) fn invalidate_cursor_texture(&mut self) {
        self.rebuild_pending = true;
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

    /// Retire a compositor-owned upload texture before CPU backing changes.
    ///
    /// `ResourceTable` has no remove operation, so retirement must rebuild the
    /// private table rather than dropping only the cache entry and leaking its
    /// resource slot.
    pub(super) fn remove_window_texture(
        &mut self,
        window_id: WindowId,
    ) -> Result<(), &'static str> {
        if self
            .textures
            .iter()
            .any(|entry| entry.window_id == window_id)
        {
            self.rebuild_pending = true;
        }
        Ok(())
    }

    /// Retire all GPU resources after the window itself is closed.
    pub(super) fn remove_window(&mut self, window_id: WindowId) -> Result<(), &'static str> {
        self.remove_window_texture(window_id)?;
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
        cursor_visible: bool,
        damage: Option<DamageRect>,
    ) -> Result<Vec<SgfxCommitToken>, &'static str> {
        super::trace::set_compositor_stage(super::trace::STAGE_GPU_SYNC_WINDOWS);
        self.rebuild_if_needed(cursor, windows)?;
        let force_full_repaint = self.force_full_repaint;
        let damage = if force_full_repaint { None } else { damage };
        for window in windows {
            super::trace::set_gpu_window(window.id);
            // CPU-backed texture extent changes are detected before this loop
            // and cause a fresh private resource table. Each active window is
            // therefore re-uploaded into that replacement table here.
            let has_current_backing = window.pixels().is_ok();
            if window.visible && has_current_backing {
                self.sync_window_texture(window)?;
            }
        }

        super::trace::set_gpu_window(0);
        super::trace::set_compositor_stage(super::trace::STAGE_GPU_ENCODE);
        let clear_color = bgra_color(background);
        let damage_clip = damage
            .map(|(x, y, width, height)| PixelRect::new(x, y, width, height))
            .transpose()
            .map_err(|_| "Invalid GPU composition damage")?;
        let mut operations = Vec::new();
        if let Some(clip) = damage_clip {
            operations.push(Quad::Solid {
                destination: clip,
                color: clear_color,
                clip: None,
            });
        }
        for window in windows {
            if !window.visible {
                continue;
            }

            let has_cached_texture = self
                .textures
                .iter()
                .any(|entry| entry.window_id == window.id);
            let has_current_backing = window.pixels().is_ok();
            if has_cached_texture || has_current_backing {
                let texture = self
                    .textures
                    .iter()
                    .find(|entry| entry.window_id == window.id)
                    .ok_or("GPU window texture cache is missing")?;
                // A geometry change can temporarily precede its next CPU
                // upload. Keep sampling the private cache within its extent.
                let content_width = window.width.min(texture.width);
                let content_height = window.height.min(texture.height);
                let Some((destination, source)) = clipped_rect(
                    window.x,
                    window.y,
                    content_width,
                    content_height,
                    self.target.width,
                    self.target.height,
                ) else {
                    continue;
                };
                if damage_clip.is_some_and(|clip| !pixel_rects_intersect(destination, clip)) {
                    continue;
                }
                operations.push(Quad::Sampled(SampledRect {
                    texture: texture.texture,
                    texture_width: texture.width,
                    texture_height: texture.height,
                    destination,
                    source,
                    tint: Color::rgba(1.0, 1.0, 1.0, window.opacity)
                        .map_err(|_| "Invalid window opacity")?,
                    ignore_source_alpha: !window.has_alpha_content,
                    clip: damage_clip,
                }));
            } else {
                let Some((destination, _)) = clipped_rect(
                    window.x,
                    window.y,
                    window.width,
                    window.height,
                    self.target.width,
                    self.target.height,
                ) else {
                    continue;
                };
                if damage_clip.is_some_and(|clip| !pixel_rects_intersect(destination, clip)) {
                    continue;
                }
                let color = if window.focused {
                    bgra_color([150, 150, 200, 255])
                } else {
                    bgra_color([180, 180, 180, 255])
                };
                operations.push(Quad::Solid {
                    destination,
                    color,
                    clip: damage_clip,
                });
            }
        }

        if let Some(outline) = resize_outline {
            append_outline(
                &mut operations,
                outline,
                self.target.width,
                self.target.height,
                damage_clip,
            )?;
        }
        let (cursor_x, cursor_y) = cursor.draw_position();
        if cursor_visible
            && let Some((destination, source)) = clipped_rect(
                cursor_x,
                cursor_y,
                self.cursor_width,
                self.cursor_height,
                self.target.width,
                self.target.height,
            )
            && damage_clip.is_none_or(|clip| pixel_rects_intersect(destination, clip))
        {
            let cursor_texture = self
                .cursor_textures
                .get(cursor.active_frame_index())
                .ok_or("GPU cursor frame texture is missing")?;
            operations.push(Quad::Sampled(SampledRect {
                texture: *cursor_texture,
                texture_width: self.cursor_width,
                texture_height: self.cursor_height,
                destination,
                source,
                tint: Color::rgba(1.0, 1.0, 1.0, 1.0).map_err(|_| "Invalid cursor tint")?,
                ignore_source_alpha: false,
                clip: damage_clip,
            }));
        }

        super::trace::set_compositor_stage(super::trace::STAGE_GPU_SUBMIT);
        let load = if damage_clip.is_some() {
            LoadOp::Load
        } else {
            LoadOp::Clear(clear_color)
        };
        self.quad_renderer
            .submit(&mut self.target, load, &operations)?;
        super::trace::set_compositor_stage(super::trace::STAGE_GPU_PRESENT);
        let region = damage.map(|(x, y, width, height)| DisplayPresentRegion {
            x,
            y,
            width,
            height,
        });
        self.target.present(display, region)?;
        self.force_full_repaint = false;
        super::trace::set_compositor_stage(super::trace::STAGE_GPU_COLLECT_RELEASES);
        Ok(Vec::new())
    }

    fn sync_window_texture(&mut self, window: &Window) -> Result<(), &'static str> {
        let pixels = window.pixels().ok();
        let pixels = pixels
            .as_ref()
            .ok_or("Window has no CPU-readable pixel backing")?;
        let (width, height) = (pixels.width(), pixels.height());
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
            Some(_) => return Err("GPU resource rebuild did not complete before extent change"),
            None => self.create_window_texture(window, width, height)?,
        };
        let entry = &self.textures[texture_index];
        let Some(damage) = entry.pending_damage else {
            return Ok(());
        };
        let texture = entry.texture;
        let damage_rect = PixelRect::new(damage.0, damage.1, damage.2, damage.3)
            .map_err(|_| "Invalid GPU window damage")?;
        let source = pixels.damage_bytes(damage.0, damage.1, damage.2, damage.3)?;
        upload_bgra(
            &mut self.target,
            texture,
            damage_rect,
            pixels.stride(),
            source,
        )?;
        self.textures[texture_index].pending_damage = None;
        Ok(())
    }

    fn create_window_texture(
        &mut self,
        _window: &Window,
        width: u32,
        height: u32,
    ) -> Result<usize, &'static str> {
        let texture = define_bgra_texture(self.target.resources.as_ref(), width, height)
            .map_err(|_| "Failed to define private GPU window texture")?;
        self.textures.push(CachedWindowTexture {
            window_id: _window.id,
            width,
            height,
            texture,
            pending_damage: Some((0, 0, width, height)),
        });
        Ok(self.textures.len() - 1)
    }

    fn rebuild_if_needed(
        &mut self,
        cursor: &Cursor,
        windows: &[Window],
    ) -> Result<(), &'static str> {
        let cursor_changed = self.cursor_texture_generation != cursor.texture_generation()
            || self.cursor_width != cursor.width
            || self.cursor_height != cursor.height
            || self.cursor_textures.len() != cursor.frame_count();
        let texture_extent_changed = self.textures.iter().any(|texture| {
            windows
                .iter()
                .find(|window| window.id == texture.window_id)
                .is_none_or(|window| {
                    window.pixels().ok().is_some_and(|pixels| {
                        pixels.width() != texture.width || pixels.height() != texture.height
                    })
                })
        });
        if !self.rebuild_pending && !cursor_changed && !texture_extent_changed {
            return Ok(());
        }

        let (width, height) = self
            .rebuild_extent
            .unwrap_or((self.target.width, self.target.height));
        let mut target =
            MappedTarget::open(width, height).map_err(|_| "Failed to rebuild mapped GPU target")?;
        let quad_renderer = QuadRenderer::define(target.resources.as_ref(), 96)
            .map_err(|_| "Failed to redefine GPU composition resources")?;
        let cursor_textures = create_cursor_textures(&mut target, cursor)?;

        // A replacement session owns a fresh table, so no retired cursor or
        // window texture can consume another logical resource slot. The same
        // frame re-uploads every active CPU-backed window below and clears the
        // entire target before presenting it.
        self.target = target;
        self.quad_renderer = quad_renderer;
        self.cursor_textures = cursor_textures;
        self.cursor_width = cursor.width;
        self.cursor_height = cursor.height;
        self.cursor_texture_generation = cursor.texture_generation();
        self.textures.clear();
        self.rebuild_pending = false;
        self.rebuild_extent = None;
        self.force_full_repaint = true;
        Ok(())
    }
}

fn create_cursor_textures(
    target: &mut MappedTarget,
    cursor: &Cursor,
) -> Result<Vec<TextureId>, &'static str> {
    let frame_count = cursor.frame_count();
    if frame_count == 0 {
        return Err("Cursor image has no GPU-uploadable frames");
    }

    let mut textures = Vec::new();
    textures
        .try_reserve_exact(frame_count)
        .map_err(|_| "Failed to reserve GPU cursor frame textures")?;
    for frame_index in 0..frame_count {
        let Some(pixels) = cursor.frame_bgra_pixels(frame_index) else {
            return Err("Cursor frame pixels are missing");
        };
        let texture = define_bgra_texture(target.resources.as_ref(), cursor.width, cursor.height)
            .map_err(|_| "Failed to define GPU cursor frame texture")?;
        let area = PixelRect::new(0, 0, cursor.width, cursor.height)
            .map_err(|_| "Invalid GPU cursor frame extent")?;
        upload_bgra(
            target,
            texture,
            area,
            cursor.width.saturating_mul(4),
            pixels,
        )?;
        textures.push(texture);
    }
    Ok(textures)
}

fn bgra_color(color: [u8; 4]) -> Color {
    Color::rgba(
        color[2] as f32 / 255.0,
        color[1] as f32 / 255.0,
        color[0] as f32 / 255.0,
        color[3] as f32 / 255.0,
    )
    .expect("8-bit colors are valid normalized SGFX colors")
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
        PixelRect::new(left as u32, top as u32, width, height).ok()?,
        PixelRect::new((left - x) as u32, (top - y) as u32, width, height).ok()?,
    ))
}

fn pixel_rects_intersect(a: PixelRect, b: PixelRect) -> bool {
    let a_right = a.x().saturating_add(a.width());
    let a_bottom = a.y().saturating_add(a.height());
    let b_right = b.x().saturating_add(b.width());
    let b_bottom = b.y().saturating_add(b.height());
    a.x() < b_right && b.x() < a_right && a.y() < b_bottom && b.y() < a_bottom
}

fn append_outline(
    operations: &mut Vec<Quad>,
    rect: (i32, i32, u32, u32),
    target_width: u32,
    target_height: u32,
    clip: Option<PixelRect>,
) -> Result<(), &'static str> {
    append_outline_ring(
        operations,
        rect,
        bgra_color([0, 0, 0, 255]),
        target_width,
        target_height,
        clip,
    )?;
    if rect.2 > 2 && rect.3 > 2 {
        append_outline_ring(
            operations,
            (rect.0 + 1, rect.1 + 1, rect.2 - 2, rect.3 - 2),
            bgra_color([255, 255, 255, 255]),
            target_width,
            target_height,
            clip,
        )?;
    }
    Ok(())
}

fn append_outline_ring(
    operations: &mut Vec<Quad>,
    rect: (i32, i32, u32, u32),
    color: Color,
    target_width: u32,
    target_height: u32,
    clip: Option<PixelRect>,
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
            if clip.is_some_and(|clip| !pixel_rects_intersect(destination, clip)) {
                continue;
            }
            operations.push(Quad::Solid {
                destination,
                color,
                clip,
            });
        }
    }
    Ok(())
}
