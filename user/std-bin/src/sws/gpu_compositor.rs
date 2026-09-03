//! Optional GPU-backed scene composition for SWS.

use super::cursor::Cursor;
use super::window::{PresentationInstance, Window, WindowId, WindowType, rounded_rect_row_span};
use framebuffer::{DisplayPresentRegion, DisplaySurface};
use scarlet_os::handle::Handle;
use sgfx::ir::{Color, LoadOp, PixelRect, TextureId};
use std::fmt;
use std::vec::Vec;

use crate::sgfx_ir_support::{
    CopiedRect, MappedTarget, Quad, QuadRenderer, QuadSubmitError, SampledRect,
    define_bgra_texture, upload_bgra,
};

type DamageRect = (u32, u32, u32, u32);

/// Scope of one failed GPU-composition frame.
#[derive(Debug)]
pub(super) enum GpuCompositionError {
    /// Frame construction failed before valid work reached the backend.
    Frame(&'static str),
    /// The mapped target or selected backend can no longer be trusted.
    Backend(&'static str),
    /// The SGFX backend rejected execution of a valid command buffer.
    Execution(sgfx::Error),
}

impl GpuCompositionError {
    /// Return whether shared client images must be invalidated.
    pub(super) const fn invalidates_shared_images(&self) -> bool {
        // SGFX execution errors currently combine validation, unsupported IR,
        // allocation, and transport failures. None is an explicit Vulkan-like
        // DEVICE_LOST signal, so execution rejection alone must remain local
        // to the frame. A mapped-target failure is the narrower point at which
        // the compositor can no longer promise that imported images survive.
        matches!(self, Self::Backend(_))
    }
}

impl From<&'static str> for GpuCompositionError {
    fn from(error: &'static str) -> Self {
        Self::Frame(error)
    }
}

impl From<QuadSubmitError> for GpuCompositionError {
    fn from(error: QuadSubmitError) -> Self {
        match error {
            QuadSubmitError::Recording(error) => Self::Frame(error),
            QuadSubmitError::Execution(error) => Self::Execution(error),
        }
    }
}

impl fmt::Display for GpuCompositionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Frame(error) | Self::Backend(error) => formatter.write_str(error),
            Self::Execution(error) => write!(formatter, "SGFX execution failed: {error}"),
        }
    }
}

const MAX_RETIRED_WINDOW_TEXTURES: usize = 8;
const MAX_RETIRED_WINDOW_TEXTURE_BYTES: u64 = 24 * 1024 * 1024;
const COMPOSITION_QUAD_CAPACITY: usize = 2048;

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
    window_id: Option<WindowId>,
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
        let quad_renderer =
            QuadRenderer::define(target.resources.as_ref(), COMPOSITION_QUAD_CAPACITY)
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

    /// Read damaged regions from the most recently presented SGFX target.
    ///
    /// # Arguments
    ///
    /// * `destination` - Complete writable BGRA capture buffer.
    /// * `destination_stride` - Bytes between destination rows.
    /// * `damage` - Output-space regions to transfer.
    ///
    /// # Returns
    ///
    /// Success after synchronous GPU readback of every region.
    pub(super) fn capture_bgra(
        &self,
        destination: &mut [u8],
        destination_stride: u32,
        damage: &[sws_remote_protocol::Rect],
    ) -> Result<(), &'static str> {
        let mut regions = Vec::new();
        regions
            .try_reserve_exact(damage.len())
            .map_err(|_| "Failed to reserve SGFX capture damage")?;
        for rect in damage {
            regions.push(
                PixelRect::new(rect.x, rect.y, rect.width, rect.height)
                    .map_err(|_| "Invalid SGFX capture damage")?,
            );
        }
        self.target
            .readback_bgra(destination, destination_stride, &regions)
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
            .find(|entry| entry.window_id == Some(window_id))
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
    /// The logical resource table is append-only, so keep the already-mapped
    /// texture as an unassigned cache slot. A later window with the same extent
    /// can reuse it without rebuilding the full-screen swapchain or allocating
    /// another physical image while the system is under memory pressure.
    pub(super) fn remove_window_texture(
        &mut self,
        window_id: WindowId,
    ) -> Result<(), &'static str> {
        if let Some(texture) = self
            .textures
            .iter_mut()
            .find(|entry| entry.window_id == Some(window_id))
        {
            texture.window_id = None;
            texture.pending_damage = None;
        }
        self.schedule_retired_texture_rebuild();
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

    fn append_window_projection(
        &self,
        operations: &mut Vec<Quad>,
        window: &Window,
        instance: Option<PresentationInstance>,
        damage_clip: Option<PixelRect>,
    ) -> Result<(), &'static str> {
        let presentation_clip = instance
            .and_then(|instance| instance.clip)
            .or(window.presentation_clip);
        let clip_radius = instance.map_or(window.presentation_clip_radius, |instance| {
            instance.clip_radius
        });
        let rounded_clip = presentation_clip
            .filter(|_| clip_radius > 0)
            .map(|rect| (rect, clip_radius));
        let presentation_clip = if let Some((x, y, width, height)) = presentation_clip {
            let Some((clip, _)) =
                clipped_rect(x, y, width, height, self.target.width, self.target.height)
            else {
                return Ok(());
            };
            Some(clip)
        } else {
            None
        };
        let operation_clip = match (damage_clip, presentation_clip) {
            (Some(damage), Some(presentation)) => {
                let Some(intersection) = intersect_pixel_rects(damage, presentation) else {
                    return Ok(());
                };
                Some(intersection)
            }
            (Some(clip), None) | (None, Some(clip)) => Some(clip),
            (None, None) => None,
        };

        let has_cached_texture = self
            .textures
            .iter()
            .any(|entry| entry.window_id == Some(window.id));
        let has_current_backing = window.pixels().is_ok();
        let visual_geometry = instance.map_or_else(
            || window.presentation_geometry(),
            |instance| {
                let transform = instance.transform;
                (transform.x, transform.y, transform.width, transform.height)
            },
        );
        let opacity = instance.map_or_else(
            || window.presentation_opacity(),
            |instance| (window.opacity * instance.transform.opacity).clamp(0.0, 1.0),
        );
        let transformed = instance.is_some() || window.presentation_transform.is_some();
        if self.has_committed_shared_buffer(window.id) || has_cached_texture || has_current_backing
        {
            let (texture, texture_width, texture_height) =
                match self.committed_shared_texture(window.id) {
                    Some(texture) => texture,
                    None => {
                        let texture = self
                            .textures
                            .iter()
                            .find(|entry| entry.window_id == Some(window.id))
                            .ok_or("GPU window texture cache is missing")?;
                        (texture.texture, texture.width, texture.height)
                    }
                };
            let content_width = window.width.min(texture_width);
            let content_height = window.height.min(texture_height);
            let (visual_x, visual_y, visual_width, visual_height) = visual_geometry;
            let rects = if transformed {
                clipped_scaled_rect(
                    visual_x,
                    visual_y,
                    visual_width,
                    visual_height,
                    content_width,
                    content_height,
                    self.target.width,
                    self.target.height,
                )
            } else {
                clipped_rect(
                    visual_x,
                    visual_y,
                    content_width,
                    content_height,
                    self.target.width,
                    self.target.height,
                )
            };
            let Some((destination, source)) = rects else {
                return Ok(());
            };
            if operation_clip.is_some_and(|clip| !pixel_rects_intersect(destination, clip)) {
                return Ok(());
            }
            if !transformed && opacity == 1.0 && !window.has_alpha_content {
                append_rounded_quad(
                    operations,
                    Quad::Copy(CopiedRect {
                        texture,
                        destination,
                        source,
                        clip: None,
                    }),
                    rounded_clip,
                    operation_clip,
                    self.target.width,
                    self.target.height,
                )?;
            } else {
                append_rounded_quad(
                    operations,
                    Quad::Sampled(SampledRect {
                        texture,
                        texture_width,
                        texture_height,
                        destination,
                        source,
                        tint: Color::rgba(1.0, 1.0, 1.0, opacity)
                            .map_err(|_| "Invalid window opacity")?,
                        ignore_source_alpha: !window.has_alpha_content,
                        clip: None,
                    }),
                    rounded_clip,
                    operation_clip,
                    self.target.width,
                    self.target.height,
                )?;
            }
        } else {
            let (x, y, width, height) = visual_geometry;
            let Some((destination, _)) =
                clipped_rect(x, y, width, height, self.target.width, self.target.height)
            else {
                return Ok(());
            };
            if operation_clip.is_some_and(|clip| !pixel_rects_intersect(destination, clip)) {
                return Ok(());
            }
            let color = if window.focused {
                bgra_color([150, 150, 200, 255])
            } else {
                bgra_color([180, 180, 180, 255])
            };
            append_rounded_quad(
                operations,
                Quad::Solid {
                    destination,
                    color,
                    clip: None,
                },
                rounded_clip,
                operation_clip,
                self.target.width,
                self.target.height,
            )?;
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
        overview_shadows: &[((i32, i32, u32, u32), u32, [u8; 4])],
        overview_cards: &[((i32, i32, u32, u32), bool, bool)],
        overview_remove_buttons: &[(u32, (i32, i32, u32, u32), bool)],
        resize_outline: Option<(i32, i32, u32, u32)>,
        cursor_visible: bool,
        damage: Option<DamageRect>,
    ) -> Result<Vec<SgfxCommitToken>, GpuCompositionError> {
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
            if window.is_presented()
                && !self.has_committed_shared_buffer(window.id)
                && has_current_backing
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
        let mut overview_backplates_drawn = false;
        for window in windows {
            if !window.is_presented() {
                continue;
            }
            if !overview_backplates_drawn
                && !matches!(
                    window.window_type,
                    WindowType::Desktop | WindowType::ShellBackground
                )
            {
                append_overview_shadows(
                    &mut operations,
                    overview_shadows,
                    self.target.width,
                    self.target.height,
                    damage_clip,
                )?;
                append_overview_backplates(
                    &mut operations,
                    overview_cards,
                    self.target.width,
                    self.target.height,
                    damage_clip,
                )?;
                overview_backplates_drawn = true;
            }
            self.append_window_projection(&mut operations, window, None, damage_clip)?;
            for instance in &window.presentation_instances {
                self.append_window_projection(
                    &mut operations,
                    window,
                    Some(*instance),
                    damage_clip,
                )?;
            }
        }
        if !overview_backplates_drawn {
            append_overview_shadows(
                &mut operations,
                overview_shadows,
                self.target.width,
                self.target.height,
                damage_clip,
            )?;
            append_overview_backplates(
                &mut operations,
                overview_cards,
                self.target.width,
                self.target.height,
                damage_clip,
            )?;
        }
        append_overview_remove_buttons(
            &mut operations,
            overview_remove_buttons,
            self.target.width,
            self.target.height,
            damage_clip,
        )?;

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
        self.target
            .present(display, region)
            .map_err(GpuCompositionError::Backend)?;
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
            releases.extend(promote_pending_shared_frame(state).into_iter().flatten());
        }
        // Once a shared image has reached the display, the CPU-upload cache
        // for that window is redundant. Keep its physical image as a reusable
        // same-size slot instead of retaining one private copy per SGFX
        // window. If the shared path is later removed, `sync_window_texture`
        // transparently claims a compatible slot and uploads the SHM backing.
        for texture in &mut self.textures {
            let Some(window_id) = texture.window_id else {
                continue;
            };
            if self
                .shared_windows
                .iter()
                .any(|state| state.window_id == window_id && state.presented.is_some())
            {
                texture.window_id = None;
                texture.pending_damage = None;
            }
        }
        self.schedule_retired_texture_rebuild();
        releases
    }

    fn schedule_retired_texture_rebuild(&mut self) {
        let mut count = 0usize;
        let mut bytes = 0u64;
        for texture in &self.textures {
            if texture.window_id.is_some() {
                continue;
            }
            count += 1;
            bytes = bytes.saturating_add(
                u64::from(texture.width)
                    .saturating_mul(u64::from(texture.height))
                    .saturating_mul(4),
            );
        }
        if count > MAX_RETIRED_WINDOW_TEXTURES || bytes > MAX_RETIRED_WINDOW_TEXTURE_BYTES {
            // MappedTarget's resource table is append-only. Rebuilding is the
            // only way to release unmatched extents; schedule it while the
            // cache is still small enough to allocate the replacement target.
            self.rebuild_pending = true;
        }
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
            .position(|entry| entry.window_id == Some(window.id));
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
        window: &Window,
        width: u32,
        height: u32,
    ) -> Result<usize, &'static str> {
        if let Some(index) = self.textures.iter().position(|entry| {
            entry.window_id.is_none() && entry.width == width && entry.height == height
        }) {
            self.textures[index].window_id = Some(window.id);
            self.textures[index].pending_damage = Some((0, 0, width, height));
            return Ok(index);
        }
        let texture = define_bgra_texture(self.target.resources.as_ref(), width, height)
            .map_err(|_| "Failed to define private GPU window texture")?;
        self.textures.push(CachedWindowTexture {
            window_id: Some(window.id),
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
            let Some(window_id) = texture.window_id else {
                return false;
            };
            windows
                .iter()
                .find(|window| window.id == window_id)
                .is_some_and(|window| {
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
        let quad_renderer =
            QuadRenderer::define(target.resources.as_ref(), COMPOSITION_QUAD_CAPACITY)
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

/// Promote the frame used for the completed display present.
///
/// The previously presented commit remains retained until this transition.
/// This is also the generation boundary used during a window resize: SWS may
/// sample the old extent until the replacement extent has actually presented.
fn promote_pending_shared_frame(state: &mut SharedWindowState) -> [Option<SgfxCommitToken>; 2] {
    let Some(pending) = state.pending.take() else {
        return [None, None];
    };
    [
        state.presented.replace(pending),
        state.retire_after_present.take(),
    ]
}

#[cfg(test)]
mod shared_frame_promotion_tests {
    use super::*;

    #[test]
    fn frame_recording_failure_does_not_invalidate_client_epochs() {
        assert!(!GpuCompositionError::Frame("invalid frame").invalidates_shared_images());
        assert!(
            !GpuCompositionError::Execution(sgfx::Error::InvalidBackendPreference)
                .invalidates_shared_images()
        );
        assert!(GpuCompositionError::Backend("device unavailable").invalidates_shared_images());
    }

    fn token(buffer_id: u32, generation: u32, commit_serial: u64) -> SgfxCommitToken {
        SgfxCommitToken {
            identity: SgfxBufferIdentity {
                window_id: 41,
                buffer_id,
                generation,
                compositor_epoch: 3,
            },
            commit_serial,
        }
    }

    #[test]
    fn replacement_generation_releases_old_frame_only_after_promotion() {
        let old_presented = token(1, 7, 101);
        let replacement = token(1, 8, 102);
        let mut state = SharedWindowState {
            window_id: 41,
            latest_generation: 8,
            compositor_epoch: 3,
            presented: Some(old_presented),
            pending: Some(replacement),
            retire_after_present: None,
        };

        assert_eq!(state.presented, Some(old_presented));
        assert_eq!(state.pending, Some(replacement));

        let releases = promote_pending_shared_frame(&mut state);

        assert_eq!(releases, [Some(old_presented), None]);
        assert_eq!(state.presented, Some(replacement));
        assert!(state.pending.is_none());
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

fn clipped_scaled_rect(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
) -> Option<(PixelRect, PixelRect)> {
    if width == 0 || height == 0 || source_width == 0 || source_height == 0 {
        return None;
    }
    let right = x.checked_add(width as i32)?;
    let bottom = y.checked_add(height as i32)?;
    let left = x.max(0).min(target_width as i32);
    let top = y.max(0).min(target_height as i32);
    let right = right.max(0).min(target_width as i32);
    let bottom = bottom.max(0).min(target_height as i32);
    if right <= left || bottom <= top {
        return None;
    }

    let source_left = ((i64::from(left - x) * i64::from(source_width)) / i64::from(width)) as u32;
    let source_top = ((i64::from(top - y) * i64::from(source_height)) / i64::from(height)) as u32;
    let source_right = ((i64::from(right - x) * i64::from(source_width)
        + i64::from(width.saturating_sub(1)))
        / i64::from(width)) as u32;
    let source_bottom = ((i64::from(bottom - y) * i64::from(source_height)
        + i64::from(height.saturating_sub(1)))
        / i64::from(height)) as u32;
    Some((
        PixelRect::new(
            left as u32,
            top as u32,
            (right - left) as u32,
            (bottom - top) as u32,
        )
        .ok()?,
        PixelRect::new(
            source_left.min(source_width.saturating_sub(1)),
            source_top.min(source_height.saturating_sub(1)),
            source_right
                .min(source_width)
                .saturating_sub(source_left)
                .max(1),
            source_bottom
                .min(source_height)
                .saturating_sub(source_top)
                .max(1),
        )
        .ok()?,
    ))
}

fn pixel_rects_intersect(a: PixelRect, b: PixelRect) -> bool {
    let a_right = a.x().saturating_add(a.width());
    let a_bottom = a.y().saturating_add(a.height());
    let b_right = b.x().saturating_add(b.width());
    let b_bottom = b.y().saturating_add(b.height());
    a.x() < b_right && b.x() < a_right && a.y() < b_bottom && b.y() < a_bottom
}

fn intersect_pixel_rects(a: PixelRect, b: PixelRect) -> Option<PixelRect> {
    let left = a.x().max(b.x());
    let top = a.y().max(b.y());
    let right = a
        .x()
        .saturating_add(a.width())
        .min(b.x().saturating_add(b.width()));
    let bottom = a
        .y()
        .saturating_add(a.height())
        .min(b.y().saturating_add(b.height()));
    (right > left && bottom > top)
        .then(|| PixelRect::new(left, top, right - left, bottom - top).ok())
        .flatten()
}

fn quad_with_clip(operation: Quad, clip: Option<PixelRect>) -> Quad {
    match operation {
        Quad::Solid {
            destination, color, ..
        } => Quad::Solid {
            destination,
            color,
            clip,
        },
        Quad::Sampled(mut rect) => {
            rect.clip = clip;
            Quad::Sampled(rect)
        }
        Quad::Copy(mut rect) => {
            rect.clip = clip;
            Quad::Copy(rect)
        }
    }
}

fn append_rounded_quad(
    operations: &mut Vec<Quad>,
    operation: Quad,
    rounded_clip: Option<((i32, i32, u32, u32), u32)>,
    base_clip: Option<PixelRect>,
    target_width: u32,
    target_height: u32,
) -> Result<(), &'static str> {
    let Some((rect, radius)) = rounded_clip else {
        operations.push(quad_with_clip(operation, base_clip));
        return Ok(());
    };

    let top = rect.1.max(0);
    let bottom = rect
        .1
        .saturating_add(rect.3 as i32)
        .min(target_height as i32);
    let mut bands: Vec<(i32, i32, u32, u32)> = Vec::new();
    for row_y in top..bottom {
        let Some((left, right)) = rounded_rect_row_span(rect, radius, row_y) else {
            continue;
        };
        let left = left.max(0);
        let right = right.min(target_width as i32);
        if right <= left {
            continue;
        }
        let width = (right - left) as u32;
        if let Some(last) = bands.last_mut()
            && last.0 == left
            && last.2 == width
            && last.1.saturating_add(last.3 as i32) == row_y
        {
            last.3 = last.3.saturating_add(1);
        } else {
            bands.push((left, row_y, width, 1));
        }
    }

    for (x, y, width, height) in bands {
        let mut clip = PixelRect::new(x as u32, y as u32, width, height)
            .map_err(|_| "Invalid rounded Overview clip")?;
        if let Some(base) = base_clip {
            let Some(intersection) = intersect_pixel_rects(clip, base) else {
                continue;
            };
            clip = intersection;
        }
        operations.push(quad_with_clip(operation, Some(clip)));
    }
    Ok(())
}

fn append_overview_backplates(
    operations: &mut Vec<Quad>,
    overview_cards: &[((i32, i32, u32, u32), bool, bool)],
    target_width: u32,
    target_height: u32,
    damage_clip: Option<PixelRect>,
) -> Result<(), &'static str> {
    for (rect, selected_or_hovered, add_workspace) in overview_cards {
        let Some((destination, _)) =
            clipped_rect(rect.0, rect.1, rect.2, rect.3, target_width, target_height)
        else {
            continue;
        };
        let color = if *add_workspace {
            if *selected_or_hovered {
                bgra_color([255, 255, 255, 34])
            } else {
                bgra_color([255, 255, 255, 18])
            }
        } else if *selected_or_hovered {
            bgra_color(sws_protocol::workspace::OVERVIEW_CARD_SELECTED_OVERLAY_BGRA)
        } else {
            bgra_color(sws_protocol::workspace::OVERVIEW_CARD_INACTIVE_OVERLAY_BGRA)
        };
        append_rounded_quad(
            operations,
            Quad::Solid {
                destination,
                color,
                clip: None,
            },
            Some((*rect, sws_protocol::workspace::OVERVIEW_CARD_CORNER_RADIUS)),
            damage_clip,
            target_width,
            target_height,
        )?;
        if *add_workspace {
            let short_side = rect.2.min(rect.3);
            let arm = (short_side / 5).clamp(14, 52);
            let thickness = (short_side / 48).clamp(2, 6);
            let center_x = rect.0.saturating_add((rect.2 / 2) as i32);
            let center_y = rect.1.saturating_add((rect.3 / 2) as i32);
            let plus_color =
                bgra_color([255, 255, 255, if *selected_or_hovered { 220 } else { 170 }]);
            for plus_rect in [
                (
                    center_x.saturating_sub((arm / 2) as i32),
                    center_y.saturating_sub((thickness / 2) as i32),
                    arm,
                    thickness,
                ),
                (
                    center_x.saturating_sub((thickness / 2) as i32),
                    center_y.saturating_sub((arm / 2) as i32),
                    thickness,
                    arm,
                ),
            ] {
                let Some((plus_destination, _)) = clipped_rect(
                    plus_rect.0,
                    plus_rect.1,
                    plus_rect.2,
                    plus_rect.3,
                    target_width,
                    target_height,
                ) else {
                    continue;
                };
                operations.push(Quad::Solid {
                    destination: plus_destination,
                    color: plus_color,
                    clip: damage_clip,
                });
            }
        }
    }
    Ok(())
}

fn append_overview_shadows(
    operations: &mut Vec<Quad>,
    shadows: &[((i32, i32, u32, u32), u32, [u8; 4])],
    target_width: u32,
    target_height: u32,
    damage_clip: Option<PixelRect>,
) -> Result<(), &'static str> {
    for (rect, radius, color) in shadows {
        let Some((destination, _)) =
            clipped_rect(rect.0, rect.1, rect.2, rect.3, target_width, target_height)
        else {
            continue;
        };
        append_rounded_quad(
            operations,
            Quad::Solid {
                destination,
                color: bgra_color(*color),
                clip: None,
            },
            Some((*rect, *radius)),
            damage_clip,
            target_width,
            target_height,
        )?;
    }
    Ok(())
}

fn append_overview_remove_buttons(
    operations: &mut Vec<Quad>,
    buttons: &[(u32, (i32, i32, u32, u32), bool)],
    target_width: u32,
    target_height: u32,
    damage_clip: Option<PixelRect>,
) -> Result<(), &'static str> {
    for (_, rect, hovered) in buttons {
        let Some((destination, _)) =
            clipped_rect(rect.0, rect.1, rect.2, rect.3, target_width, target_height)
        else {
            continue;
        };
        append_rounded_quad(
            operations,
            Quad::Solid {
                destination,
                color: bgra_color([0, 0, 0, if *hovered { 118 } else { 82 }]),
                clip: None,
            },
            Some((*rect, rect.2 / 2)),
            damage_clip,
            target_width,
            target_height,
        )?;

        let center_x = rect.0.saturating_add((rect.2 / 2) as i32);
        let center_y = rect.1.saturating_add((rect.3 / 2) as i32);
        let half = (rect.2.min(rect.3) / 5).max(5) as i32;
        let dot = (rect.2.min(rect.3) / 16).clamp(2, 3);
        let color = bgra_color([255, 255, 255, if *hovered { 235 } else { 205 }]);
        for delta in -half..=half {
            for (x, y) in [
                (
                    center_x.saturating_add(delta),
                    center_y.saturating_add(delta),
                ),
                (
                    center_x.saturating_add(delta),
                    center_y.saturating_sub(delta),
                ),
            ] {
                let Some((dot_rect, _)) = clipped_rect(
                    x.saturating_sub((dot / 2) as i32),
                    y.saturating_sub((dot / 2) as i32),
                    dot,
                    dot,
                    target_width,
                    target_height,
                ) else {
                    continue;
                };
                if damage_clip.is_some_and(|clip| !pixel_rects_intersect(dot_rect, clip)) {
                    continue;
                }
                operations.push(Quad::Solid {
                    destination: dot_rect,
                    color,
                    clip: damage_clip,
                });
            }
        }
    }
    Ok(())
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
