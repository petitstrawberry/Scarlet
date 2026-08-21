//! Optional GPU-backed scene composition for SWS.

use core::mem;

use super::cursor::Cursor;
use super::window::{Window, WindowId};
use framebuffer::{DisplayPresentRegion, DisplaySurface};
use scarlet_os::handle::Handle;
use sgfx::{
    Color, CompositionPass, Context, Device, Image, PixelRect, Queue, SgfxImagePresentExt,
    SourceAlpha, Texture,
};
use std::vec::Vec;

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

struct SharedWindowTexture {
    identity: SgfxBufferIdentity,
    width: u32,
    height: u32,
    texture: Texture,
}

struct SharedWindowState {
    window_id: WindowId,
    latest_generation: u32,
    compositor_epoch: u32,
    presented: Option<SgfxCommitToken>,
    pending: Option<SgfxCommitToken>,
    retire_after_present: Option<SgfxCommitToken>,
}

/// GPU resources and texture cache used by the internal SWS compositor.
pub(super) struct GpuCompositor {
    _context: Context,
    queue: Queue,
    target: Image,
    cursor_textures: Vec<Texture>,
    cursor_width: u32,
    cursor_height: u32,
    cursor_texture_generation: u64,
    textures: Vec<CachedWindowTexture>,
    shared_textures: Vec<SharedWindowTexture>,
    shared_windows: Vec<SharedWindowState>,
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
        let cursor_textures = create_cursor_textures(&context, cursor)?;

        Ok(Self {
            _context: context,
            queue,
            target,
            cursor_textures,
            cursor_width: cursor.width,
            cursor_height: cursor.height,
            cursor_texture_generation: cursor.texture_generation(),
            textures: Vec::new(),
            shared_textures: Vec::new(),
            shared_windows: Vec::new(),
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

        // Do not advance the accepted generation until import and metadata
        // validation succeed. A rejected capability must leave existing buffers
        // usable so a client can retry the registration.
        let texture = self
            ._context
            .import_shared_bgra_texture(handle)
            .map_err(|_| SgfxBufferError::ImportFailed)?;
        if texture.width() != width || texture.height() != height {
            self._context
                .release_texture(texture)
                .map_err(|_| SgfxBufferError::Unavailable)?;
            return Err(SgfxBufferError::InvalidBuffer);
        }
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
            // Keep a superseded, never-sampled frame retained until the same
            // presentation boundary as its replacement. Releasing it here
            // would let a two-slot client run without display backpressure and
            // overwrite every visible animation frame before SWS presents.
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
        let entry = self.shared_textures.remove(index);
        self._context
            .release_texture(entry.texture)
            .map_err(|_| SgfxBufferError::Unavailable)
    }

    /// Recreate the screen-sized target after an output resize.
    pub(super) fn resize_target(&mut self, width: u32, height: u32) -> Result<(), &'static str> {
        let target = self
            ._context
            .create_image(width, height)
            .map_err(|_| "Failed to resize GPU render target")?;
        let old_target = mem::replace(&mut self.target, target);
        self._context
            .release_image(old_target)
            .map_err(|_| "Failed to detach GPU render target")?;
        Ok(())
    }

    fn sync_cursor_textures(&mut self, cursor: &Cursor) -> Result<(), &'static str> {
        // Animation advances only the selected frame index. Rebuild this cache
        // when the semantic cursor image changes, never on each animation tick.
        if self.cursor_texture_generation == cursor.texture_generation()
            && self.cursor_width == cursor.width
            && self.cursor_height == cursor.height
            && self.cursor_textures.len() == cursor.frame_count()
        {
            return Ok(());
        }

        let textures = create_cursor_textures(&self._context, cursor)?;
        let previous = mem::replace(&mut self.cursor_textures, textures);
        self.cursor_width = cursor.width;
        self.cursor_height = cursor.height;
        self.cursor_texture_generation = cursor.texture_generation();
        release_cursor_textures(&self._context, previous)
    }

    /// Force the next composition to rebuild the active cursor frame textures.
    ///
    /// This is required when a complete theme replacement starts its cursor
    /// generation counter from the same value as the preceding theme.
    pub(super) fn invalidate_cursor_texture(&mut self) {
        self.cursor_texture_generation = 0;
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

    /// Detach the compositor-owned upload texture before CPU backing changes.
    ///
    /// Client-owned shared SGFX registrations deliberately survive a window
    /// resize. Their old generation remains committed until a new generation
    /// is presented, at which point the normal release protocol lets the
    /// client destroy it without losing a wakeup.
    pub(super) fn remove_window_texture(
        &mut self,
        window_id: WindowId,
    ) -> Result<(), &'static str> {
        if let Some(index) = self
            .textures
            .iter()
            .position(|entry| entry.window_id == window_id)
        {
            self.release_window_texture(index)?;
        }
        Ok(())
    }

    /// Detach all GPU resources after the window itself is closed.
    pub(super) fn remove_window(&mut self, window_id: WindowId) -> Result<(), &'static str> {
        self.remove_window_texture(window_id)?;
        while let Some(index) = self
            .shared_textures
            .iter()
            .position(|entry| entry.identity.window_id == window_id)
        {
            let entry = self.shared_textures.remove(index);
            self._context
                .release_texture(entry.texture)
                .map_err(|_| "Failed to detach shared SGFX window texture")?;
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
        self.sync_cursor_textures(cursor)?;
        for window in windows {
            super::trace::set_gpu_window(window.id);
            // Window geometry is updated before a resize client installs its
            // replacement backing.  During that transition the old SHM still
            // exists, but it no longer validates against the new dimensions.
            // Keep sampling the cached texture until a complete new backing is
            // available instead of treating the expected transition as a GPU
            // compositor failure.
            let has_current_backing = window.shm_layout().is_ok() || window.pixels().is_ok();
            if window.visible && !self.has_committed_shared_buffer(window.id) && has_current_backing
            {
                self.sync_window_texture(window)?;
            }
        }

        super::trace::set_gpu_window(0);
        super::trace::set_compositor_stage(super::trace::STAGE_GPU_ENCODE);
        let clear_color = bgra_color(background);
        let damage_clip = damage.map(|(x, y, width, height)| PixelRect::new(x, y, width, height));
        let mut composition = match damage_clip {
            Some(clip) => {
                let mut composition = CompositionPass::preserving(&self.target);
                composition
                    .draw_solid_rect(clip, clear_color, None)
                    .map_err(|_| "Failed to clear GPU composition damage")?;
                composition
            }
            None => CompositionPass::new(&self.target, clear_color)
                .map_err(|_| "Failed to begin GPU composition")?,
        };
        for window in windows {
            if !window.visible {
                continue;
            }

            let has_cached_texture = self
                .textures
                .iter()
                .any(|entry| entry.window_id == window.id);
            let has_current_backing = window.shm_layout().is_ok() || window.pixels().is_ok();
            if self.has_committed_shared_buffer(window.id)
                || has_cached_texture
                || has_current_backing
            {
                let texture = match self.committed_shared_texture(window.id) {
                    Some(texture) => texture,
                    None => self
                        .textures
                        .iter()
                        .find(|entry| entry.window_id == window.id)
                        .map(|entry| &entry.texture)
                        .ok_or("GPU window texture cache is missing")?,
                };
                // A resize event can be presented just before the client's
                // replacement generation arrives. Keep the old committed
                // shared image valid without sampling beyond its extent.
                let content_width = window.width.min(texture.width());
                let content_height = window.height.min(texture.height());
                let Some((destination, source)) = clipped_rect(
                    window.x,
                    window.y,
                    content_width,
                    content_height,
                    self.target.width(),
                    self.target.height(),
                ) else {
                    continue;
                };
                if damage_clip.is_some_and(|clip| !pixel_rects_intersect(destination, clip)) {
                    continue;
                }
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
                        damage_clip,
                    )
                    .map_err(|_| "Failed to compose GPU window texture")?;
            } else {
                let Some((destination, _)) = clipped_rect(
                    window.x,
                    window.y,
                    window.width,
                    window.height,
                    self.target.width(),
                    self.target.height(),
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
                composition
                    .draw_solid_rect(destination, color, damage_clip)
                    .map_err(|_| "Failed to compose GPU window placeholder")?;
            }
        }

        if let Some(outline) = resize_outline {
            append_outline(
                &mut composition,
                outline,
                self.target.width(),
                self.target.height(),
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
                self.target.width(),
                self.target.height(),
            )
            && damage_clip.is_none_or(|clip| pixel_rects_intersect(destination, clip))
        {
            let cursor_texture = self
                .cursor_textures
                .get(cursor.active_frame_index())
                .ok_or("GPU cursor frame texture is missing")?;
            composition
                .draw_textured_rect(
                    cursor_texture,
                    destination,
                    source,
                    1.0,
                    SourceAlpha::Respect,
                    damage_clip,
                )
                .map_err(|_| "Failed to compose GPU cursor")?;
        }

        super::trace::set_compositor_stage(super::trace::STAGE_GPU_SUBMIT);
        self.queue
            .submit_composition(&composition)
            .map_err(|_| "Failed to submit GPU composition")?;
        super::trace::set_compositor_stage(super::trace::STAGE_GPU_PRESENT);
        match damage {
            Some((x, y, width, height)) => self
                .target
                .present_region(
                    display,
                    DisplayPresentRegion {
                        x,
                        y,
                        width,
                        height,
                    },
                )
                .map_err(|_| "Failed to present GPU composition damage")?,
            None => self
                .target
                .present(display)
                .map_err(|_| "Failed to present GPU composition")?,
        }
        super::trace::set_compositor_stage(super::trace::STAGE_GPU_COLLECT_RELEASES);
        Ok(self.take_presented_releases())
    }

    fn has_committed_shared_buffer(&self, window_id: WindowId) -> bool {
        self.shared_windows.iter().any(|state| {
            state.window_id == window_id && (state.pending.is_some() || state.presented.is_some())
        })
    }

    fn committed_shared_texture(&self, window_id: WindowId) -> Option<&Texture> {
        let state = self
            .shared_windows
            .iter()
            .find(|state| state.window_id == window_id)?;
        let identity = state.pending.or(state.presented)?.identity;
        self.shared_textures
            .iter()
            .find(|entry| entry.identity == identity)
            .map(|entry| &entry.texture)
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

fn create_cursor_textures(
    context: &Context,
    cursor: &Cursor,
) -> Result<Vec<Texture>, &'static str> {
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
            let _ = release_cursor_textures(context, textures);
            return Err("Cursor frame pixels are missing");
        };
        let texture = match context.create_sampled_bgra_texture(cursor.width, cursor.height) {
            Ok(texture) => texture,
            Err(_) => {
                let _ = release_cursor_textures(context, textures);
                return Err("Failed to create GPU cursor frame texture");
            }
        };
        if context
            .upload_texture_bgra(
                &texture,
                pixels,
                cursor.width.saturating_mul(4),
                PixelRect::new(0, 0, cursor.width, cursor.height),
            )
            .is_err()
        {
            let _ = context.release_texture(texture);
            let _ = release_cursor_textures(context, textures);
            return Err("Failed to upload GPU cursor frame texture");
        }
        textures.push(texture);
    }
    Ok(textures)
}

fn release_cursor_textures(context: &Context, textures: Vec<Texture>) -> Result<(), &'static str> {
    let mut release_failed = false;
    for texture in textures {
        release_failed |= context.release_texture(texture).is_err();
    }
    if release_failed {
        Err("Failed to release previous GPU cursor frame textures")
    } else {
        Ok(())
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

fn pixel_rects_intersect(a: PixelRect, b: PixelRect) -> bool {
    let a_right = a.x().saturating_add(a.width());
    let a_bottom = a.y().saturating_add(a.height());
    let b_right = b.x().saturating_add(b.width());
    let b_bottom = b.y().saturating_add(b.height());
    a.x() < b_right && b.x() < a_right && a.y() < b_bottom && b.y() < a_bottom
}

fn append_outline(
    composition: &mut CompositionPass<'_>,
    rect: (i32, i32, u32, u32),
    target_width: u32,
    target_height: u32,
    clip: Option<PixelRect>,
) -> Result<(), &'static str> {
    append_outline_ring(
        composition,
        rect,
        bgra_color([0, 0, 0, 255]),
        target_width,
        target_height,
        clip,
    )?;
    if rect.2 > 2 && rect.3 > 2 {
        append_outline_ring(
            composition,
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
    composition: &mut CompositionPass<'_>,
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
            composition
                .draw_solid_rect(destination, color, clip)
                .map_err(|_| "Failed to compose GPU resize outline")?;
        }
    }
    Ok(())
}
