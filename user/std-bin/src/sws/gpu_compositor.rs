//! Optional GPU-backed scene composition for SWS.

use super::cursor::Cursor;
use super::window::{Window, WindowId};
use framebuffer::{DisplayPresentRegion, DisplaySurface};
use scarlet_os::handle::Handle;
use sgfx::ir::{Color, LoadOp, PixelRect, TextureId};
use std::vec::Vec;

use crate::sgfx_ir_support::{
    CopiedRect, MappedTarget, Quad, QuadRenderer, SampledRect, define_bgra_texture, upload_bgra,
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

struct SharedWindowTexture {
    identity: SgfxBufferIdentity,
    width: u32,
    height: u32,
    texture: TextureId,
    handle: Handle,
}

struct SharedWindowState {
    window_id: WindowId,
    latest_generation: u32,
    compositor_epoch: u32,
    presented: Option<SgfxCommitToken>,
    pending: Option<SgfxCommitToken>,
    retire_after_present: Option<SgfxCommitToken>,
}

struct CursorTextureSet {
    width: u32,
    height: u32,
    frames: Vec<TextureId>,
}

/// GPU resources and texture cache used by the internal SWS compositor.
pub(super) struct GpuCompositor {
    target: MappedTarget,
    quad_renderer: QuadRenderer,
    cursor_images: Vec<CursorTextureSet>,
    textures: Vec<CachedWindowTexture>,
    shared_textures: Vec<SharedWindowTexture>,
    shared_windows: Vec<SharedWindowState>,
    rebuild_pending: bool,
    rebuild_extent: Option<(u32, u32)>,
    force_full_repaint: bool,
}

impl GpuCompositor {
    /// Create the optional GPU compositor after confirming required capabilities.
    pub(super) fn new(width: u32, height: u32, cursor: &Cursor) -> Result<Self, &'static str> {
        let mut target = MappedTarget::open_swapchain(width, height)
            .map_err(|_| "Failed to create mapped GPU swapchain")?;
        let quad_renderer = QuadRenderer::define(target.resources.as_ref(), 96)
            .map_err(|_| "Failed to define GPU composition resources")?;
        // Upload the complete theme once. Pointer-shape changes are common
        // during hover and resize, and must only select another texture rather
        // than rebuilding the GPU context and every live window resource.
        let cursor_images = create_cursor_images(&mut target, cursor)?;

        Ok(Self {
            target,
            quad_renderer,
            cursor_images,
            textures: Vec::new(),
            shared_textures: Vec::new(),
            shared_windows: Vec::new(),
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
        if self
            .shared_textures
            .iter()
            .any(|entry| entry.identity == identity)
        {
            return Err(SgfxBufferError::InvalidBuffer);
        }

        let state_index = self
            .shared_windows
            .iter()
            .position(|state| state.window_id == identity.window_id);
        if let Some(index) = state_index {
            let state = &self.shared_windows[index];
            if identity.compositor_epoch != state.compositor_epoch
                || identity.generation < state.latest_generation
            {
                return Err(SgfxBufferError::StaleGeneration);
            }
        }

        // Keep the received capability as the durable owner. The mapped
        // session consumes a duplicate so target/session rebuilds can reimport
        // every still-registered client image without changing its identity.
        let imported_handle = handle
            .duplicate()
            .map_err(|_| SgfxBufferError::ImportFailed)?;
        let texture = self
            .target
            .import_shared_bgra_texture(width, height, imported_handle)
            .map_err(|_| SgfxBufferError::ImportFailed)?;
        match state_index {
            Some(index) => {
                self.shared_windows[index].latest_generation = identity.generation;
            }
            None => self.shared_windows.push(SharedWindowState {
                window_id: identity.window_id,
                latest_generation: identity.generation,
                compositor_epoch: identity.compositor_epoch,
                presented: None,
                pending: None,
                retire_after_present: None,
            }),
        }
        self.shared_textures.push(SharedWindowTexture {
            identity,
            width,
            height,
            texture,
            handle,
        });
        Ok(())
    }

    /// Atomically publish a registered buffer and bounded damage list.
    pub(super) fn commit_shared_buffer(
        &mut self,
        identity: SgfxBufferIdentity,
        commit_serial: u64,
        damage_rects: &[sws_protocol::SgfxDamageRect],
    ) -> Result<Vec<DamageRect>, SgfxBufferError> {
        if commit_serial == 0 {
            return Err(SgfxBufferError::InvalidBuffer);
        }
        let texture = self
            .shared_textures
            .iter()
            .find(|entry| entry.identity == identity)
            .ok_or(SgfxBufferError::InvalidBuffer)?;
        let state = self
            .shared_windows
            .iter_mut()
            .find(|state| state.window_id == identity.window_id)
            .ok_or(SgfxBufferError::InvalidBuffer)?;
        if identity.compositor_epoch != state.compositor_epoch
            || identity.generation != state.latest_generation
        {
            return Err(SgfxBufferError::StaleGeneration);
        }
        if state
            .presented
            .is_some_and(|commit| commit.identity == identity)
            || state
                .pending
                .is_some_and(|commit| commit.identity == identity)
            || state
                .retire_after_present
                .is_some_and(|commit| commit.identity == identity)
        {
            return Err(SgfxBufferError::BufferBusy);
        }

        let mut clipped_damage = Vec::new();
        for rect in damage_rects {
            let left = i64::from(rect.x).max(0).min(i64::from(texture.width));
            let top = i64::from(rect.y).max(0).min(i64::from(texture.height));
            let right = i64::from(rect.x)
                .saturating_add(i64::from(rect.width))
                .max(0)
                .min(i64::from(texture.width));
            let bottom = i64::from(rect.y)
                .saturating_add(i64::from(rect.height))
                .max(0)
                .min(i64::from(texture.height));
            if right > left && bottom > top {
                clipped_damage.push((
                    left as u32,
                    top as u32,
                    (right - left) as u32,
                    (bottom - top) as u32,
                ));
            }
        }
        if clipped_damage.is_empty() {
            return Err(SgfxBufferError::InvalidBuffer);
        }
        let commit = SgfxCommitToken {
            identity,
            commit_serial,
        };
        if state.pending.is_some() {
            if state.retire_after_present.is_some() {
                return Err(SgfxBufferError::BufferBusy);
            }
            state.retire_after_present = state.pending.replace(commit);
        } else {
            state.pending = Some(commit);
        }
        Ok(clipped_damage)
    }

    /// Remove a registered shared buffer that is not retained by SWS.
    pub(super) fn destroy_shared_buffer(
        &mut self,
        identity: SgfxBufferIdentity,
    ) -> Result<(), SgfxBufferError> {
        if let Some(state) = self
            .shared_windows
            .iter()
            .find(|state| state.window_id == identity.window_id)
            && (state
                .presented
                .is_some_and(|commit| commit.identity == identity)
                || state
                    .pending
                    .is_some_and(|commit| commit.identity == identity)
                || state
                    .retire_after_present
                    .is_some_and(|commit| commit.identity == identity))
        {
            return Err(SgfxBufferError::BufferBusy);
        }
        let index = self
            .shared_textures
            .iter()
            .position(|entry| entry.identity == identity)
            .ok_or(SgfxBufferError::InvalidBuffer)?;
        self.target
            .release_imported_texture(self.shared_textures[index].texture)
            .map_err(|_| SgfxBufferError::Unavailable)?;
        self.shared_textures.remove(index);
        // MappedTarget retains the now-unmapped logical texture slot. A later
        // buffer with the same extent reimports into that slot without
        // rebuilding the output target, GPU context, or all live windows.
        Ok(())
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
        while let Some(index) = self
            .shared_textures
            .iter()
            .position(|entry| entry.identity.window_id == window_id)
        {
            let texture = self.shared_textures[index].texture;
            self.target
                .release_imported_texture(texture)
                .map_err(|_| "Failed to detach shared SGFX window texture")?;
            self.shared_textures.remove(index);
        }
        self.shared_windows
            .retain(|state| state.window_id != window_id);
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
            if window.visible && !self.has_committed_shared_buffer(window.id) && has_current_backing
            {
                self.sync_window_texture(window)?;
            }
        }

        super::trace::set_gpu_window(0);
        super::trace::set_compositor_stage(super::trace::STAGE_GPU_ENCODE);
        let clear_color = bgra_color(background);
        let requested_clip = damage
            .map(|(x, y, width, height)| PixelRect::new(x, y, width, height))
            .transpose()
            .map_err(|_| "Invalid GPU composition damage")?;
        let full_area = PixelRect::new(0, 0, self.target.width, self.target.height)
            .map_err(|_| "Invalid GPU composition target area")?;
        let render_area = self
            .target
            .prepare_render_area(requested_clip.unwrap_or(full_area))
            .map_err(|_| "Failed to prepare GPU swapchain damage")?;
        let damage_clip = (render_area != full_area).then_some(render_area);
        let mut operations = Vec::new();
        for window in windows {
            if !window.visible {
                continue;
            }

            let has_cached_texture = self
                .textures
                .iter()
                .any(|entry| entry.window_id == window.id);
            let has_current_backing = window.pixels().is_ok();
            if self.has_committed_shared_buffer(window.id)
                || has_cached_texture
                || has_current_backing
            {
                let (texture, texture_width, texture_height) =
                    match self.committed_shared_texture(window.id) {
                        Some(texture) => texture,
                        None => {
                            let texture = self
                                .textures
                                .iter()
                                .find(|entry| entry.window_id == window.id)
                                .ok_or("GPU window texture cache is missing")?;
                            (texture.texture, texture.width, texture.height)
                        }
                    };
                // A geometry change can temporarily precede its next CPU
                // upload or shared generation. Keep sampling the current cache
                // without extending beyond the registered image extent.
                let content_width = window.width.min(texture_width);
                let content_height = window.height.min(texture_height);
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
                if window.opacity == 1.0 && !window.has_alpha_content {
                    operations.push(Quad::Copy(CopiedRect {
                        texture,
                        destination,
                        source,
                        clip: damage_clip,
                    }));
                } else {
                    operations.push(Quad::Sampled(SampledRect {
                        texture,
                        texture_width,
                        texture_height,
                        destination,
                        source,
                        tint: Color::rgba(1.0, 1.0, 1.0, window.opacity)
                            .map_err(|_| "Invalid window opacity")?,
                        ignore_source_alpha: !window.has_alpha_content,
                        clip: damage_clip,
                    }));
                }
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
        let (cursor_texture, cursor_width, cursor_height) = {
            let cursor_image = self
                .cursor_images
                .get(cursor.active_image_index())
                .ok_or("GPU cursor image is missing")?;
            let texture = cursor_image
                .frames
                .get(cursor.active_frame_index())
                .copied()
                .ok_or("GPU cursor frame texture is missing")?;
            (texture, cursor_image.width, cursor_image.height)
        };
        let (cursor_x, cursor_y) = cursor.draw_position();
        if cursor_visible
            && let Some((destination, source)) = clipped_rect(
                cursor_x,
                cursor_y,
                cursor_width,
                cursor_height,
                self.target.width,
                self.target.height,
            )
            && damage_clip.is_none_or(|clip| pixel_rects_intersect(destination, clip))
        {
            operations.push(Quad::Sampled(SampledRect {
                texture: cursor_texture,
                texture_width: cursor_width,
                texture_height: cursor_height,
                destination,
                source,
                tint: Color::rgba(1.0, 1.0, 1.0, 1.0).map_err(|_| "Invalid cursor tint")?,
                ignore_source_alpha: false,
                clip: damage_clip,
            }));
        }

        super::trace::set_compositor_stage(super::trace::STAGE_GPU_SUBMIT);
        self.quad_renderer.submit_region(
            &mut self.target,
            render_area,
            LoadOp::Clear(clear_color),
            &operations,
        )?;
        super::trace::set_compositor_stage(super::trace::STAGE_GPU_PRESENT);
        let region = (render_area != full_area).then_some(DisplayPresentRegion {
            x: render_area.x(),
            y: render_area.y(),
            width: render_area.width(),
            height: render_area.height(),
        });
        self.target.present(display, region)?;
        self.force_full_repaint = false;
        super::trace::set_compositor_stage(super::trace::STAGE_GPU_COLLECT_RELEASES);
        Ok(self.take_presented_releases())
    }

    pub(super) fn has_committed_shared_buffer(&self, window_id: WindowId) -> bool {
        self.shared_windows.iter().any(|state| {
            state.window_id == window_id && (state.pending.is_some() || state.presented.is_some())
        })
    }

    fn committed_shared_texture(&self, window_id: WindowId) -> Option<(TextureId, u32, u32)> {
        let state = self
            .shared_windows
            .iter()
            .find(|state| state.window_id == window_id)?;
        let identity = state.pending.or(state.presented)?.identity;
        self.shared_textures
            .iter()
            .find(|entry| entry.identity == identity)
            .map(|entry| (entry.texture, entry.width, entry.height))
    }

    fn take_presented_releases(&mut self) -> Vec<SgfxCommitToken> {
        let mut releases = Vec::new();
        for state in &mut self.shared_windows {
            let Some(pending) = state.pending.take() else {
                continue;
            };
            if let Some(previous) = state.presented.replace(pending) {
                releases.push(previous);
            }
            if let Some(superseded) = state.retire_after_present.take() {
                releases.push(superseded);
            }
        }
        releases
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
        let cursor_layout_changed = self.cursor_images.len() != cursor.image_count()
            || self.cursor_images.iter().enumerate().any(|(index, image)| {
                cursor.image_extent(index) != Some((image.width, image.height))
                    || cursor.image_frame_count(index) != Some(image.frames.len())
            });
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
        if !self.rebuild_pending && !cursor_layout_changed && !texture_extent_changed {
            return Ok(());
        }

        let (width, height) = self
            .rebuild_extent
            .unwrap_or((self.target.width, self.target.height));
        let mut target = MappedTarget::open_swapchain(width, height)
            .map_err(|_| "Failed to rebuild mapped GPU swapchain")?;
        let quad_renderer = QuadRenderer::define(target.resources.as_ref(), 96)
            .map_err(|_| "Failed to redefine GPU composition resources")?;
        let cursor_images = create_cursor_images(&mut target, cursor)?;
        let mut shared_texture_ids = Vec::new();
        shared_texture_ids
            .try_reserve_exact(self.shared_textures.len())
            .map_err(|_| "Failed to reserve rebuilt shared texture mappings")?;
        for shared in &self.shared_textures {
            let handle = shared
                .handle
                .duplicate()
                .map_err(|_| "Failed to duplicate shared SGFX image")?;
            let texture = target
                .import_shared_bgra_texture(shared.width, shared.height, handle)
                .map_err(|_| "Failed to reimport shared SGFX image")?;
            shared_texture_ids.push(texture);
        }

        // A replacement session owns a fresh table, so no retired cursor or
        // window texture can consume another logical resource slot. The same
        // frame re-uploads every active CPU-backed window below and clears the
        // entire target before presenting it.
        self.target = target;
        self.quad_renderer = quad_renderer;
        self.cursor_images = cursor_images;
        self.textures.clear();
        for (shared, texture) in self.shared_textures.iter_mut().zip(shared_texture_ids) {
            shared.texture = texture;
        }
        self.rebuild_pending = false;
        self.rebuild_extent = None;
        self.force_full_repaint = true;
        Ok(())
    }
}

fn create_cursor_images(
    target: &mut MappedTarget,
    cursor: &Cursor,
) -> Result<Vec<CursorTextureSet>, &'static str> {
    let image_count = cursor.image_count();
    if image_count == 0 {
        return Err("Cursor theme has no GPU-uploadable images");
    }

    let mut images = Vec::new();
    images
        .try_reserve_exact(image_count)
        .map_err(|_| "Failed to reserve GPU cursor images")?;
    for image_index in 0..image_count {
        let (width, height) = cursor
            .image_extent(image_index)
            .ok_or("Cursor image extent is missing")?;
        let frame_count = cursor
            .image_frame_count(image_index)
            .ok_or("Cursor image frame metadata is missing")?;
        if frame_count == 0 {
            return Err("Cursor image has no GPU-uploadable frames");
        }

        let mut frames = Vec::new();
        frames
            .try_reserve_exact(frame_count)
            .map_err(|_| "Failed to reserve GPU cursor frame textures")?;
        for frame_index in 0..frame_count {
            let pixels = cursor
                .image_frame_bgra_pixels(image_index, frame_index)
                .ok_or("Cursor frame pixels are missing")?;
            let texture = define_bgra_texture(target.resources.as_ref(), width, height)
                .map_err(|_| "Failed to define GPU cursor frame texture")?;
            let area = PixelRect::new(0, 0, width, height)
                .map_err(|_| "Invalid GPU cursor frame extent")?;
            upload_bgra(target, texture, area, width.saturating_mul(4), pixels)?;
            frames.push(texture);
        }
        images.push(CursorTextureSet {
            width,
            height,
            frames,
        });
    }
    Ok(images)
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
