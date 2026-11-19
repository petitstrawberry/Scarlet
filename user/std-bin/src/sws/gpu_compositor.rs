//! Optional GPU-backed scene composition for SWS.

#[path = "output_admission.rs"]
mod output_admission;

use super::output::OutputProjection;
use super::cursor::Cursor;
use super::status_backdrop::{BackdropDamage, BackdropGeometry, backdrop_refreshes};
use super::window::{PresentationInstance, Window, WindowId, WindowType, rounded_rect_row_span};
use framebuffer::{DisplayPresentRegion, DisplaySurface};
use gpu_raw::{Gpu, GpuContext, GpuImage, GpuImageBgraRect};
use scarlet_os::handle::Handle;
use scarlet_os::ipc::SharedMemory;
use scarlet_ui_renderer_sgfx::FrameSubmissionError;
use sgfx::ir::{Color, LoadOp, PixelRect, TextureId};
use std::cell::RefCell;
use std::env;
use std::fmt;
use std::time::Instant;
use std::vec::Vec;

use crate::sgfx_ir_support::{
    BackdropPass, BackdropTextures, CopiedRect, MappedTarget, MaterialDamage, Quad, QuadOutput,
    QuadRenderer, QuadSubmitError, SampledRect, TextureUpload, define_bgra_texture,
    define_scene_texture, retarget_backdrops, upload_bgra,
};

use crate::sgfx_ir_support::clipped_scaled_image as clipped_scaled_rect;

use super::surface_scene::{SceneView, source_uv};

type DamageRect = (u32, u32, u32, u32);

/// Every composition target, including resource-table/extent replacements,
/// must own its receipt worker BEFORE any final asynchronous frame is admitted.
/// Target drop still joins that worker before releasing images/session state.
fn open_composition_target(extents: &[(u32, u32)]) -> Result<MappedTarget, &'static str> {
    let mut target = MappedTarget::open_swapchains(extents)
        .map_err(|_| "Failed to create mapped GPU swapchain")?;
    // Synchronous GPU executors still hand flips to the display worker.
    target.prepare_display_completion(super::ipc::wake_compositor);
    if target.supports_tracked_submission() {
        target.prepare_gpu_completion(super::ipc::wake_compositor)?;
    }
    Ok(target)
}

/// Damage is expressed in pixels of the registered image, not the window.
pub(super) struct SharedFrameDamage {
    pub(super) extent: (u32, u32),
    pub(super) rects: Vec<DamageRect>,
}

/// Scope of one failed GPU-composition frame.
#[derive(Debug)]
pub(super) enum GpuCompositionError {
    /// Display capacity unavailable before any frame construction or GPU work.
    DisplayBusy,
    /// One fully admitted frame is waiting for proven GPU retirement.
    Pending,
    /// Temporary GPU admission pressure, with the discarded prefix retired.
    Busy,
    /// Frame construction failed before valid work reached the backend.
    Frame(&'static str),
    /// The mapped target or selected backend can no longer be trusted.
    Backend(&'static str),
    /// The SGFX backend rejected execution of a valid command buffer.
    Execution(sgfx::Error),
    /// A tracked frame failed admission or completion; do not reuse its cache
    /// or release imported client buffers as if the frame had completed.
    TrackedExecution(FrameSubmissionError<sgfx::Error, sgfx::Submission>),
}

impl GpuCompositionError {
    /// Return whether shared client images must be invalidated.
    pub(super) const fn invalidates_shared_images(&self) -> bool {
        // Legacy execution rejection does not imply backend loss. A tracked
        // frame, however, may have accepted earlier streams before failing;
        // without successful completion its images cannot be reused/released.
        // Backend failures also invalidate imported-image lifetime guarantees.
        matches!(self, Self::Backend(_) | Self::TrackedExecution(_))
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
            QuadSubmitError::Busy => Self::Busy,
            QuadSubmitError::Recording(error) => Self::Frame(error),
            QuadSubmitError::Execution(error) => Self::Execution(error),
        }
    }
}

impl fmt::Display for GpuCompositionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => formatter.write_str("GPU composition pending"),
            Self::DisplayBusy => formatter.write_str("Display admission is temporarily busy"),
            Self::Busy => formatter.write_str("GPU admission is temporarily busy"),
            Self::Frame(error) | Self::Backend(error) => formatter.write_str(error),
            Self::Execution(error) => write!(formatter, "SGFX execution failed: {error}"),
            Self::TrackedExecution(error) => {
                write!(formatter, "SGFX tracked frame failed: {error}")
            }
        }
    }
}

const MAX_RETIRED_WINDOW_TEXTURES: usize = 8;
const MAX_RETIRED_WINDOW_TEXTURE_BYTES: u64 = 24 * 1024 * 1024;
const MAX_INACTIVE_BACKDROPS: usize = 8;
const MAX_INACTIVE_BACKDROP_BYTES: u64 = 24 * 1024 * 1024;
const COMPOSITION_QUAD_CAPACITY: usize = 2048;

fn direct_extension_shm_import_enabled() -> bool {
    env::var("SWS_EXTENSION_SHM_DIRECT_IMPORT")
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
}

fn clipped_damage(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    target_width: u32,
    target_height: u32,
) -> Option<DamageRect> {
    if width == 0 || height == 0 {
        return None;
    }
    let left = i64::from(x).max(0).min(i64::from(target_width));
    let top = i64::from(y).max(0).min(i64::from(target_height));
    let right = i64::from(x)
        .saturating_add(i64::from(width))
        .max(0)
        .min(i64::from(target_width));
    let bottom = i64::from(y)
        .saturating_add(i64::from(height))
        .max(0)
        .min(i64::from(target_height));
    (right > left && bottom > top).then_some((
        left as u32,
        top as u32,
        (right - left) as u32,
        (bottom - top) as u32,
    ))
}

fn union_damage(left: DamageRect, right: DamageRect) -> DamageRect {
    let x = left.0.min(right.0);
    let y = left.1.min(right.1);
    let right_edge = left
        .0
        .saturating_add(left.2)
        .max(right.0.saturating_add(right.2));
    let bottom_edge = left
        .1
        .saturating_add(left.3)
        .max(right.1.saturating_add(right.3));
    (x, y, right_edge - x, bottom_edge - y)
}

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

/// Context used only to make client-owned SHM writes visible to imported images.
///
/// The SGFX mapped target owns the sampling context. Keeping this small raw
/// context separate lets SWS invoke the generic imported-image cache transfer
/// without teaching the compositor about the selected SGFX backend.
struct ImportedShmContext {
    gpu: Gpu,
    context: GpuContext,
}

impl ImportedShmContext {
    fn open() -> Result<Self, &'static str> {
        let gpu = Gpu::open("/dev/gpu0").map_err(|_| "Failed to open GPU for SHM imports")?;
        let dialect = gpu
            .query_dialect(0)
            .map_err(|_| "Failed to query GPU dialect for SHM imports")?;
        let context = gpu
            .create_context(&dialect)
            .map_err(|_| "Failed to create GPU context for SHM imports")?;
        Ok(Self { gpu, context })
    }
}

/// One Wayland-style SHM buffer sampled directly by the GPU compositor.
struct ImportedShmTexture {
    client_id: usize,
    buffer_id: u32,
    width: u32,
    height: u32,
    texture: TextureId,
    image: GpuImage,
    pending_damage: Option<DamageRect>,
}

/// GPU-only retained scene plus reusable upload textures for its layers.
/// Buffers remain leased by the compositor until replacement; these resources
/// never own unbounded copies of the SHM data in the CPU heap.
struct SceneTexture {
    window_id: WindowId,
    width: u32,
    height: u32,
    serial: Option<u64>,
    texture: TextureId,
    // A full GPU layer can be sampled by the final composition pass.
    // The retained scene keeps its client buffer lease until replacement.
    direct: bool,
    direct_layers: Vec<Quad>,
    uploaded_source_buffers: Vec<Option<u32>>,
    sources: Vec<(u32, u32, Option<TextureId>)>,
}

struct ExtensionGpuTexture {
    client_id: usize,
    buffer_id: u32,
    width: u32,
    height: u32,
    texture: TextureId,
    handle: Handle,
}

impl SceneTexture {
    fn matches(&self, view: &SceneView<'_>) -> bool {
        self.window_id == view.scene.window_id
            && self.width == view.scene.width
            && self.height == view.scene.height
            && self.sources.len() == view.sources.len()
            && self
                .sources
                .iter()
                .zip(&view.sources)
                .all(|(&(w, h, texture), source)| {
                    w == source.width
                        && h == source.height
                        && texture.is_none() == source.gpu_buffer.is_some()
                })
    }
}

struct SharedWindowTexture {
    identity: SgfxBufferIdentity,
    width: u32,
    height: u32,
    texture: TextureId,
    handle: Handle,
}

#[derive(Clone)]
struct SharedWindowState {
    window_id: WindowId,
    latest_generation: u32,
    compositor_epoch: u32,
    presented: Option<SgfxCommitToken>,
    pending: Option<SgfxCommitToken>,
    retire_after_present: [Option<SgfxCommitToken>; 3],
}

struct CursorTextureSet {
    width: u32,
    height: u32,
    frames: Vec<TextureId>,
}

#[derive(Clone)]
struct ScenePublication {
    window_id: WindowId,
    texture: TextureId,
    serial: u64,
    buffers: Vec<Option<u32>>,
}

struct PendingComposition {
    outputs: Vec<(usize, Option<DisplayPresentRegion>)>,
    sampled_windows: Vec<WindowId>,
    timings: Option<crate::sgfx_ir_support::FrameTimings>,
    other_outputs_pending_at_admission: usize,
    // Captures address the primary output only. An external-only submission
    // must not publish stale primary pixels as a newly rendered frame.
    logical_damage: Option<DamageRect>,
    damage_profile: Option<super::trace::DamageProfile>,
    shared: Vec<SharedWindowState>,
    backdrops: Vec<(usize, BackdropGeometry)>,
    scenes: Vec<ScenePublication>,
    profile: [Option<Instant>; 4],
    quads: usize,
    damage_pixels: u64,
}

/// What one output needs redrawn this frame, in output-local pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OutputDamage {
    /// Nothing changed on this output.
    Skip,
    /// Redraw the whole output.
    Full,
    /// Redraw this output-local rectangle.
    Region(DamageRect),
}

/// One display output to compose in a GPU frame.
pub(super) struct GpuOutputFrame<'a> {
    pub(super) id: u32,
    pub(super) overview_shadows: Vec<((i32, i32, u32, u32), u32, [u8; 4])>,
    pub(super) overview_cards: Vec<((i32, i32, u32, u32), bool, bool)>,
    pub(super) overview_remove_buttons: Vec<(u32, (i32, i32, u32, u32), bool)>,
    pub(super) display: &'a DisplaySurface,
    /// Top-left of the output in global compositor coordinates.
    pub(super) origin: (i32, i32),
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) scene_width: u32,
    pub(super) scene_height: u32,
    pub(super) scale_milli: u32,
    pub(super) damage: OutputDamage,
}

fn translate_rect(rect: (i32, i32, u32, u32), origin: (i32, i32)) -> (i32, i32, u32, u32) {
    (
        rect.0.saturating_sub(origin.0),
        rect.1.saturating_sub(origin.1),
        rect.2,
        rect.3,
    )
}

fn window_samples_output(window: &Window, output: &GpuOutputFrame<'_>) -> bool {
    if !window.is_presented() { return false; }
    if !window.presentation_cross_output && window.output_id != output.id && (window.presentation_transform.is_some()
        || !window.presentation_instances.is_empty()
        || matches!(window.window_type, WindowType::Desktop | WindowType::Taskbar
            | WindowType::ShellBackground | WindowType::ShellChrome | WindowType::ShellPanel)) { return false; }
    let intersects = |(x, y, width, height): (i32, i32, u32, u32)| {
        i64::from(x) < i64::from(output.origin.0) + i64::from(output.scene_width)
            && i64::from(output.origin.0) < i64::from(x) + i64::from(width)
            && i64::from(y) < i64::from(output.origin.1) + i64::from(output.scene_height)
            && i64::from(output.origin.1) < i64::from(y) + i64::from(height)
    };
    intersects(window.presentation_geometry()) || window.presentation_instances.iter().any(|instance| {
        let t = instance.transform;
        intersects((t.x, t.y, t.width, t.height))
    })
}

/// GPU resources and texture cache used by the internal SWS compositor.
pub(super) struct GpuCompositor {
    operations: RefCell<Vec<Quad>>,
    output_operations: Vec<Vec<Quad>>,
    frame_profile: Option<super::trace::FrameProfile>,
    pending_frames: Vec<PendingComposition>,
    output_dependencies: Vec<Vec<WindowId>>,
    full_repaint_outputs: Vec<bool>,
    drain_for_rebuild: bool,
    completed_primary_damage: Vec<DamageRect>,
    deferred_window_removals: Vec<WindowId>,
    deferred_texture_removals: Vec<WindowId>,
    target: MappedTarget,
    quad_renderer: QuadRenderer,
    cursor_images: Vec<CursorTextureSet>,
    textures: Vec<CachedWindowTexture>,
    scene_textures: Vec<SceneTexture>,
    extension_gpu_textures: Vec<ExtensionGpuTexture>,
    imported_shm_context: Option<ImportedShmContext>,
    imported_shm_textures: Vec<ImportedShmTexture>,
    shared_textures: Vec<SharedWindowTexture>,
    shared_windows: Vec<SharedWindowState>,
    rebuild_pending: bool,
    /// Output extents to recreate the target with on the next rebuild.
    rebuild_extents: Option<Vec<(u32, u32)>>,
    /// Global rectangles of the outputs composed by the latest frame.
    output_rects: Vec<(i32, i32, u32, u32)>,
    admitted_outputs: Vec<usize>,
    completed_outputs: Vec<usize>,
    force_full_repaint: bool,
    backdrops: Vec<(WindowId, BackdropGeometry, BackdropTextures)>,
    active_backdrops: Vec<(WindowId, BackdropGeometry)>,
}

impl GpuCompositor {
    pub(super) fn has_pending_frame(&self) -> bool {
        !self.pending_frames.is_empty()
    }

    pub(super) fn window_in_flight(&self, window: WindowId) -> bool {
        self.pending_frames.iter().any(|frame| frame.sampled_windows.contains(&window))
    }

    pub(super) fn needs_redraw(&self) -> bool {
        self.force_full_repaint || self.full_repaint_outputs.iter().any(|full| *full)
    }

    pub(super) fn take_completed_primary_damage(&mut self) -> Vec<DamageRect> {
        core::mem::take(&mut self.completed_primary_damage)
    }

    fn output_can_admit(&self, index: usize) -> bool {
        output_admission::can_admit(index,
            self.output_dependencies.get(index).map_or(&[], Vec::as_slice),
            self.pending_frames.iter().map(|frame| (frame.outputs[0].0, frame.sampled_windows.as_slice())))
    }

    pub(super) fn retire_pending_frame(
        &mut self,
        displays: &[&DisplaySurface],
    ) -> Result<Option<Vec<SgfxCommitToken>>, GpuCompositionError> {
        let mut completed = Vec::new();
        // Keep non-complete read sets registered while publishing completed
        // outputs. A shared client image is released only after its last reader.
        for mut frame in core::mem::take(&mut self.pending_frames) {
            let index = frame.outputs[0].0;
            if let Some(elapsed) = self.target.poll_output_gpu_completion(index).map_err(GpuCompositionError::Backend)? {
                if let Some(timings) = &mut frame.timings { timings.gpu_wait_ns += elapsed.as_nanos(); }
                completed.push(frame);
            } else {
                self.pending_frames.push(frame);
            }
        }
        if completed.is_empty() { return Ok(None); }
        let mut releases = Vec::new();
        let mut promoted = Vec::new();
        for mut frame in completed {
            frame.shared.retain(|snapshot| {
                if self.window_in_flight(snapshot.window_id) || promoted.contains(&snapshot.window_id) { return false; }
                promoted.push(snapshot.window_id);
                true
            });
            if let Some(damage) = frame.logical_damage { self.completed_primary_damage.push(damage); }
            releases.extend(self.publish_completed_frame(displays, frame)?);
        }
        Ok(Some(releases))
    }

    pub(super) fn is_accelerated(&self) -> bool {
        // A requested SWS/SGFX backend is not evidence of acceleration. Read
        // the selected target, including after a session rebuild. Unknown
        // dynamically installed backends must not opt into timed animation.
        super::animation_policy::backend_is_accelerated(self.target.backend.as_str())
    }

    /// Create the optional GPU compositor after confirming required capabilities.
    pub(super) fn new(width: u32, height: u32, cursor: &Cursor) -> Result<Self, &'static str> {
        let mut target = open_composition_target(&[(width, height)])?;
        let quad_renderer =
            QuadRenderer::define(target.resources.as_ref(), COMPOSITION_QUAD_CAPACITY)
                .map_err(|_| "Failed to define GPU composition resources")?;
        // Upload the complete theme once. Pointer-shape changes are common
        // during hover and resize, and must only select another texture rather
        // than rebuilding the GPU context and every live window resource.
        let cursor_images = create_cursor_images(&mut target, cursor)?;
        let imported_shm_context = if direct_extension_shm_import_enabled() {
            match ImportedShmContext::open() {
                Ok(context) => {
                    println!("[Compositor] Experimental extension SHM GPU import enabled");
                    Some(context)
                }
                Err(error) => {
                    println!(
                        "[Compositor] Direct SHM texture import unavailable: {}; using copied uploads",
                        error
                    );
                    None
                }
            }
        } else {
            println!(
                "[Compositor] Extension SHM uses copied GPU uploads (set SWS_EXTENSION_SHM_DIRECT_IMPORT=1 to experiment with direct import)"
            );
            None
        };

        Ok(Self {
            operations: RefCell::new(Vec::with_capacity(COMPOSITION_QUAD_CAPACITY)),
            output_operations: Vec::new(),
            target,
            pending_frames: Vec::new(),
            output_dependencies: Vec::new(),
            full_repaint_outputs: Vec::new(),
            drain_for_rebuild: false,
            completed_primary_damage: Vec::new(),
            deferred_window_removals: Vec::new(),
            deferred_texture_removals: Vec::new(),
            frame_profile: super::trace::FrameProfile::open(),
            quad_renderer,
            cursor_images,
            textures: Vec::new(),
            scene_textures: Vec::new(),
            extension_gpu_textures: Vec::new(),
            imported_shm_context,
            imported_shm_textures: Vec::new(),
            shared_textures: Vec::new(),
            shared_windows: Vec::new(),
            rebuild_pending: false,
            rebuild_extents: None,
            output_rects: vec![(0, 0, width, height)],
            admitted_outputs: Vec::new(),
            completed_outputs: Vec::new(),
            force_full_repaint: false,
            backdrops: Vec::new(),
            active_backdrops: Vec::new(),
        })
    }

    /// Retain an image capability and import it once, including scene/subsurface use.
    pub(super) fn register_extension_gpu_buffer(
        &mut self,
        client_id: usize,
        buffer_id: u32,
        width: u32,
        height: u32,
        handle: Handle,
    ) -> Result<(), &'static str> {
        if width == 0
            || height == 0
            || self
                .extension_gpu_textures
                .iter()
                .any(|entry| (entry.client_id, entry.buffer_id) == (client_id, buffer_id))
        {
            return Err("Invalid or duplicate extension GPU image");
        }
        let texture = self
            .target
            .import_shared_bgra_texture(
                width,
                height,
                handle
                    .duplicate()
                    .map_err(|_| "GPU image duplication failed")?,
            )
            .map_err(|_| "GPU image import failed")?;
        self.extension_gpu_textures.push(ExtensionGpuTexture {
            client_id,
            buffer_id,
            width,
            height,
            texture,
            handle,
        });
        Ok(())
    }

    pub(super) fn unregister_extension_gpu_buffer(
        &mut self,
        client_id: usize,
        buffer_id: u32,
    ) -> Result<(), &'static str> {
        if let Some(index) = self
            .extension_gpu_textures
            .iter()
            .position(|entry| (entry.client_id, entry.buffer_id) == (client_id, buffer_id))
        {
            let entry = self.extension_gpu_textures.swap_remove(index);
            self.target
                .release_imported_texture(entry.texture)
                .map_err(|_| "GPU scene image retirement failed")?;
        }
        Ok(())
    }

    /// Import one extension-owned SHM buffer as a directly sampled GPU image.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn register_extension_shm_buffer(
        &mut self,
        client_id: usize,
        buffer_id: u32,
        width: u32,
        height: u32,
        offset: usize,
        stride: u32,
        handle: Handle,
    ) -> Result<(), &'static str> {
        if self
            .imported_shm_textures
            .iter()
            .any(|entry| entry.client_id == client_id && entry.buffer_id == buffer_id)
        {
            return Err("Extension SHM buffer is already imported");
        }
        let imported = self
            .imported_shm_context
            .as_ref()
            .ok_or("Direct extension SHM import is unavailable")?;
        let shared_memory =
            SharedMemory::from_handle(handle).map_err(|_| "Invalid extension SHM handle")?;
        let offset = u64::try_from(offset).map_err(|_| "Extension SHM offset is too large")?;
        let image = imported
            .gpu
            .create_imported_bgra_image(&shared_memory, width, height, offset, stride)
            .map_err(|_| "Failed to create imported extension SHM image")?;
        let sampled_handle = image
            .as_handle()
            .duplicate()
            .map_err(|_| "Failed to duplicate imported extension SHM image")?;
        imported
            .context
            .attach_image(&image)
            .map_err(|_| "Failed to attach imported extension SHM image")?;
        if imported
            .context
            .transfer_imported_image_bgra(&image, GpuImageBgraRect::new(0, 0, width, height))
            .is_err()
        {
            let _ = imported.context.detach_image(&image);
            return Err("GPU backend does not support imported SHM transfers");
        }
        let texture = match self
            .target
            .import_shared_bgra_texture(width, height, sampled_handle)
        {
            Ok(texture) => texture,
            Err(_) => {
                let _ = imported.context.detach_image(&image);
                return Err("Failed to map imported extension SHM image into SGFX");
            }
        };
        self.imported_shm_textures.push(ImportedShmTexture {
            client_id,
            buffer_id,
            width,
            height,
            texture,
            image,
            // The capability probe above performed the initial full-range
            // transfer. A later client commit supplies the damage written
            // after definition and makes those bytes visible before sampling.
            pending_damage: Some((0, 0, width, height)),
        });
        Ok(())
    }

    /// Stop retaining a directly imported extension SHM buffer.
    pub(super) fn unregister_extension_shm_buffer(
        &mut self,
        client_id: usize,
        buffer_id: u32,
    ) -> Result<(), &'static str> {
        let Some(index) = self
            .imported_shm_textures
            .iter()
            .position(|entry| entry.client_id == client_id && entry.buffer_id == buffer_id)
        else {
            return Ok(());
        };
        let entry = self.imported_shm_textures.swap_remove(index);
        let release_failed = self.target.release_imported_texture(entry.texture).is_err();
        let detach_failed = self
            .imported_shm_context
            .as_ref()
            .is_some_and(|imported| imported.context.detach_image(&entry.image).is_err());
        if release_failed || detach_failed {
            // A fresh mapped target drops any stale sampling attachment. The
            // raw transfer context is independently retired with `entry`.
            self.rebuild_pending = true;
            return Err("Failed to release imported extension SHM image cleanly");
        }
        Ok(())
    }

    /// Accumulate damage for a directly imported extension SHM buffer.
    pub(super) fn mark_extension_shm_damage(
        &mut self,
        client_id: usize,
        buffer_id: u32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) {
        let Some(entry) = self
            .imported_shm_textures
            .iter_mut()
            .find(|entry| entry.client_id == client_id && entry.buffer_id == buffer_id)
        else {
            return;
        };
        let Some(damage) = clipped_damage(x, y, width, height, entry.width, entry.height) else {
            return;
        };
        entry.pending_damage = Some(match entry.pending_damage {
            Some(previous) => union_damage(previous, damage),
            None => damage,
        });
    }

    fn imported_shm_texture(&self, window: &Window) -> Option<(TextureId, u32, u32)> {
        let client_id = window.owner_client_id?;
        let buffer_id = window.external_buffer_id?;
        self.imported_shm_textures
            .iter()
            .find(|entry| entry.client_id == client_id && entry.buffer_id == buffer_id)
            .map(|entry| (entry.texture, entry.width, entry.height))
    }

    fn transfer_visible_imported_shm(&mut self, windows: &[Window], sampled: &[WindowId]) -> Result<(), &'static str> {
        let Some(imported) = self.imported_shm_context.as_ref() else {
            return Ok(());
        };
        for window in windows.iter().filter(|window| window.is_presented() && sampled.contains(&window.id)) {
            let (Some(client_id), Some(buffer_id)) =
                (window.owner_client_id, window.external_buffer_id)
            else {
                continue;
            };
            let Some(entry) = self
                .imported_shm_textures
                .iter_mut()
                .find(|entry| entry.client_id == client_id && entry.buffer_id == buffer_id)
            else {
                continue;
            };
            let Some((x, y, width, height)) = entry.pending_damage else {
                continue;
            };
            imported
                .context
                .transfer_imported_image_bgra(
                    &entry.image,
                    GpuImageBgraRect::new(x, y, width, height),
                )
                .map_err(|_| "Failed to transfer imported extension SHM damage")?;
            entry.pending_damage = None;
        }
        Ok(())
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
                retire_after_present: [None; 3],
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

    /// Poll proven display progress without constructing or admitting GPU work.
    pub(super) fn admitted_outputs(&self) -> &[usize] { &self.admitted_outputs }
    pub(super) fn take_completed_outputs(&mut self) -> Vec<usize> { core::mem::take(&mut self.completed_outputs) }
    pub(super) fn render_target_ready(&mut self, wanted: &[bool]) -> Result<bool, &'static str> {
        if self.drain_for_rebuild && self.has_pending_frame() { return Ok(false); }
        let mut ready = false;
        for (index, wanted) in wanted.iter().enumerate() {
            if !wanted && !self.force_full_repaint && !self.full_repaint_outputs.get(index).copied().unwrap_or(false) { continue; }
            if !self.output_can_admit(index) { continue; }
            self.target.select_surface(index).map_err(|_| "GPU output is missing")?;
            ready |= self.target.try_acquire_render_target()?;
        }
        self.target.select_surface(0).map_err(|_| "GPU primary output is missing")?;
        Ok(ready)
    }

    /// Atomically publish a registered buffer and bounded damage list.
    pub(super) fn commit_shared_buffer(
        &mut self,
        identity: SgfxBufferIdentity,
        commit_serial: u64,
        damage_rects: &[sws_protocol::SgfxDamageRect],
    ) -> Result<SharedFrameDamage, SgfxBufferError> {
        if commit_serial == 0 {
            return Err(SgfxBufferError::InvalidBuffer);
        }
        let texture_index = self
            .shared_textures
            .iter()
            .position(|entry| entry.identity == identity)
            .ok_or(SgfxBufferError::InvalidBuffer)?;
        let texture = &self.shared_textures[texture_index];
        let state = self
            .shared_windows
            .iter()
            .find(|state| state.window_id == identity.window_id)
            .ok_or(SgfxBufferError::InvalidBuffer)?;
        if identity.compositor_epoch != state.compositor_epoch
            || identity.generation != state.latest_generation
        {
            return Err(SgfxBufferError::StaleGeneration);
        }
        let commit = SgfxCommitToken {
            identity,
            commit_serial,
        };
        let leases = crate::sgfx_ir_support::queue_shared_commit(
            shared_lease_snapshot(state),
            commit,
            |a, b| a.identity == b.identity,
        )
        .map_err(|_| SgfxBufferError::BufferBusy)?;

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
        let extent = (texture.width, texture.height);
        // The producer completes its GPU work before COMMIT. Keep the
        // registered image and sampled view until DESTROY or a generation
        // change: reimporting every frame drains the compositor queue and
        // rebuilds sampler objects without changing the shared storage.
        let state = self
            .shared_windows
            .iter_mut()
            .find(|state| state.window_id == identity.window_id)
            .ok_or(SgfxBufferError::InvalidBuffer)?;
        install_shared_lease_snapshot(state, leases);
        Ok(SharedFrameDamage {
            extent,
            rects: clipped_damage,
        })
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
                    .iter()
                    .flatten()
                    .any(|commit| commit.identity == identity))
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
        let mut extents = self
            .rebuild_extents
            .clone()
            .unwrap_or_else(|| self.target.surface_extents());
        extents[0] = (width, height);
        self.rebuild_pending = true;
        self.rebuild_extents = Some(extents);
        Ok(())
    }

    /// Recreate the presentation targets on the next frame because an output
    /// was attached, detached or replaced, even if the extents still match.
    pub(super) fn outputs_changed(&mut self) {
        self.rebuild_pending = true;
    }

    /// Match the presentation targets to the current outputs.
    ///
    /// A changed output count or size recreates the session on the next
    /// frame, like a resize of the primary output does.
    fn set_output_extents(&mut self, extents: Vec<(u32, u32)>) {
        let current = self
            .rebuild_extents
            .clone()
            .unwrap_or_else(|| self.target.surface_extents());
        if current != extents {
            self.rebuild_pending = true;
            self.rebuild_extents = Some(extents);
        }
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
        let Some(entry) = self
            .textures
            .iter_mut()
            .find(|entry| entry.window_id == Some(window_id))
        else {
            return;
        };
        let Some(damage) = clipped_damage(x, y, width, height, entry.width, entry.height) else {
            return;
        };
        entry.pending_damage = Some(match entry.pending_damage {
            Some(previous) => union_damage(previous, damage),
            None => damage,
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
        if self.has_pending_frame() {
            if !self.deferred_texture_removals.contains(&window_id) {
                self.deferred_texture_removals.push(window_id);
            }
            return Ok(());
        }
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
        if self.has_pending_frame() {
            if !self.deferred_window_removals.contains(&window_id) {
                self.deferred_window_removals.push(window_id);
            }
            return Ok(());
        }
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

    /// Append the quads that draw one window (or presentation instance) on
    /// the selected output, whose top-left is `origin` in global coordinates.
    fn append_window_projection(
        &self,
        operations: &mut Vec<Quad>,
        window: &Window,
        instance: Option<PresentationInstance>,
        damage_clip: Option<PixelRect>,
        projection: OutputProjection,
    ) -> Result<(), &'static str> {
        let trace_home = window.title.as_deref() == Some(b"Home");
        if trace_home {
            super::home_trace::record(
                8,
                format_args!(
                    "sws-project window={} presented={} flags={} geometry={:?} transform={:?} clip={:?} opacity={} instance={:?} damage_clip={:?} retained={}",
                    window.id,
                    window.is_presented(),
                    window.state_flags(),
                    window.presentation_geometry(),
                    window.presentation_transform,
                    window.presentation_clip,
                    window.presentation_opacity(),
                    instance,
                    damage_clip,
                    window.retains_native_transition_frame(),
                ),
            );
        }
        let presentation_clip = instance
            .and_then(|instance| instance.clip)
            .or(window.presentation_clip)
            .map(|rect| projection.rect(rect));
        let clip_radius = projection.radius(instance.map_or(window.presentation_clip_radius, |instance| {
            instance.clip_radius
        }));
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
        let has_current_backing = window.presentation_pixels().is_ok();
        let scene_entry = self
            .scene_textures
            .iter()
            .find(|entry| entry.window_id == window.id);
        let scene_texture = scene_entry.map(|entry| (entry.texture, entry.width, entry.height));
        let shared_texture = self.committed_shared_texture(window);
        let imported_shm_texture = self.imported_shm_texture(window);
        if trace_home {
            let state = self
                .shared_windows
                .iter()
                .find(|state| state.window_id == window.id);
            super::home_trace::record(
                5,
                format_args!(
                    "sws-source window={} scene={:?} shared={:?} shm={:?} pending={:?} presented={:?}",
                    window.id,
                    scene_texture,
                    shared_texture,
                    imported_shm_texture,
                    state.and_then(|s| s.pending),
                    state.and_then(|s| s.presented),
                ),
            );
        }
        let directly_imported_shm = imported_shm_texture.is_some();
        let visual_geometry = projection.rect(
            instance.map_or_else(
                || window.presentation_geometry(),
                |instance| {
                    let transform = instance.transform;
                    (transform.x, transform.y, transform.width, transform.height)
                },
            ),
        );
        // Nothing of this window is on the selected output.
        if clipped_rect(
            visual_geometry.0,
            visual_geometry.1,
            visual_geometry.2,
            visual_geometry.3,
            self.target.width,
            self.target.height,
        )
        .is_none()
        {
            return Ok(());
        }
        let opacity = instance.map_or_else(
            || window.presentation_opacity(),
            |instance| (window.opacity * instance.transform.opacity).clamp(0.0, 1.0),
        );
        let transformed = instance.is_some() || window.presentation_transform.is_some();
        if let Some(entry) = scene_entry.filter(|entry| entry.direct) {
            // Direct scenes are selected only for an untransformed, opaque
            // group entirely inside the target. Layer UVs and premultiplied
            // alpha remain unchanged, including client-side decorations.
            for &quad in &entry.direct_layers {
                let Quad::SampledUv {
                    mut rect,
                    uv,
                    premultiplied,
                } = quad
                else {
                    unreachable!()
                };
                rect.destination = PixelRect::new(
                    visual_geometry.0 as u32 + rect.destination.x(),
                    visual_geometry.1 as u32 + rect.destination.y(),
                    rect.destination.width(),
                    rect.destination.height(),
                )
                .map_err(|_| "Invalid direct scene projection")?;
                append_rounded_quad(
                    operations,
                    Quad::SampledUv {
                        rect,
                        uv,
                        premultiplied,
                    },
                    rounded_clip,
                    operation_clip,
                    self.target.width,
                    self.target.height,
                )?;
            }
            return Ok(());
        }

        if scene_texture.is_some()
            || shared_texture.is_some()
            || imported_shm_texture.is_some()
            || has_cached_texture
            || has_current_backing
        {
            let (texture, texture_width, texture_height) =
                match scene_texture.or(shared_texture).or(imported_shm_texture) {
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
            let content_width = texture_width;
            let content_height = texture_height;
            let (visual_x, visual_y, visual_width, visual_height) = visual_geometry;
            let scaled =
                transformed || content_width != visual_width || content_height != visual_height;
            let rects = if scaled {
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
            if scene_texture.is_none()
                && !directly_imported_shm
                && !scaled
                && opacity == 1.0
                && !window.has_alpha_content
            {
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
                let mut rect = SampledRect {
                    texture,
                    texture_width,
                    texture_height,
                    destination,
                    source,
                    tint: Color::rgba(1.0, 1.0, 1.0, opacity)
                        .map_err(|_| "Invalid window opacity")?,
                    ignore_source_alpha: !window.has_alpha_content,
                    clip: None,
                };
                let operation = if scene_texture.is_some() || shared_texture.is_some() {
                    // Extension GPU buffers, like composed scenes, carry
                    // premultiplied RGB. Scale RGB and alpha together for
                    // window opacity; straight-alpha blending darkens glass
                    // and antialiased edges by applying source alpha twice.
                    // Opaque GPU registrations still ignore their alpha.
                    if scene_texture.is_some() {
                        rect.ignore_source_alpha = false;
                    }
                    rect.tint = Color::rgba(opacity, opacity, opacity, opacity)
                        .map_err(|_| "Invalid scene opacity")?;
                    let l = source.x() as f32 / texture_width as f32;
                    let r = (source.x() + source.width()) as f32 / texture_width as f32;
                    let t = source.y() as f32 / texture_height as f32;
                    let b = (source.y() + source.height()) as f32 / texture_height as f32;
                    Quad::SampledUv {
                        rect,
                        uv: [[l, t], [l, b], [r, b], [r, t]],
                        premultiplied: true,
                    }
                } else {
                    Quad::Sampled(rect)
                };
                append_rounded_quad(
                    operations,
                    operation,
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
    /// Compose the scene on every output and present each damaged output.
    ///
    /// `outputs[0]` must be the primary output. Window textures and scene
    /// caches are prepared once and shared; each output is then rendered with
    /// its own origin and presented on its own display. Backdrop caches are
    /// clipped to each output and addressed in that output's local pixel grid.
    pub(super) fn compose_and_present(
        &mut self,
        outputs: &[GpuOutputFrame<'_>],
        windows: &[Window],
        scenes: &[SceneView<'_>],
        cursor: &Cursor,
        background: [u8; 4],
        resize_outline: Option<(i32, i32, u32, u32)>,
        navigation_indicator: Option<(i32, i32, u32, u32)>,
        cursor_visible: bool,
        scene_damage: Option<&[BackdropDamage]>,
        backdrops: &[(WindowId, BackdropGeometry)],
        mut damage_profile: Option<super::trace::DamageProfile>,
    ) -> Result<(Vec<SgfxCommitToken>, Option<DamageRect>), GpuCompositionError> {
        self.admitted_outputs.clear();
        if let Some(profile) = &mut damage_profile {
            profile.gpu_forced = if self.force_full_repaint { 4 } else { 0 };
        }
        if outputs.is_empty() {
            return Err("GPU composition has no outputs".into());
        }
        self.set_output_extents(outputs.iter().map(|f| (f.width, f.height)).collect());
        self.output_rects = outputs
            .iter()
            .map(|f| (f.origin.0, f.origin.1, f.scene_width, f.scene_height))
            .collect();
        let damage = match outputs[0].damage {
            OutputDamage::Region(rect) => Some(rect),
            _ => None,
        };
        let profiling = super::trace::profile_enabled() || self.frame_profile.is_some();
        let profile_start = profiling.then(Instant::now);
        super::trace::set_compositor_stage(super::trace::STAGE_GPU_SYNC_WINDOWS);
        // Match individual material allocations, not absolute screen geometry.
        // A window may own several regions with identical extents: consume each
        // cached allocation once. Moving a region only invalidates its content;
        // it must not drain presentation, rebuild the context/cursor atlas and
        // reimport every client image on every animation frame.
        if self.active_backdrops.iter().any(|old| !backdrops.contains(old)) {
            // Keep the old rebuild's conservative invalidation for changed
            // placement/rounding and overlapping upper materials. Only the
            // expensive allocation/context reset is removed.
            self.force_full_repaint = true;
            if let Some(profile) = &mut damage_profile {
                profile.gpu_forced |= 2;
            }
        }
        // Keep a bounded set of inactive extents. Menus can return to a previous
        // size without draining presentation and reimporting all client images.
        if retarget_backdrops(&mut self.backdrops, backdrops, |geometry| {
            (geometry.source.2, geometry.source.3, geometry.radius)
        }) {
            let mut inactive = 0;
            let mut bytes = 0u64;
            for (owner, geometry, textures) in &mut self.backdrops {
                if !backdrops.contains(&(*owner, *geometry)) {
                    textures.valid = false;
                    inactive += 1;
                    bytes = bytes.saturating_add(textures.allocation_bytes());
                }
            }
            if inactive > MAX_INACTIVE_BACKDROPS || bytes > MAX_INACTIVE_BACKDROP_BYTES {
                self.rebuild_pending = true;
            }
        }
        self.active_backdrops.clear();
        self.active_backdrops.extend_from_slice(backdrops);
        // Retire changed layouts with the same bounded resource-table rebuild
        // policy used by window textures. A buffer swap at the same extent
        // reuses the source texture; it does not allocate a new GPU resource.
        if self
            .scene_textures
            .iter()
            .any(|cached| !scenes.iter().any(|view| cached.matches(view)))
        {
            self.rebuild_pending = true;
        }
        let rebuilt = self.rebuild_if_needed(cursor, windows)?;
        if self.drain_for_rebuild { return Err(GpuCompositionError::DisplayBusy); }
        self.full_repaint_outputs.resize(outputs.len(), false);
        if self.force_full_repaint {
            self.full_repaint_outputs.fill(true);
            self.force_full_repaint = false;
        }
        self.output_dependencies = outputs.iter().map(|output|
            windows.iter().filter(|window| window_samples_output(window, output)).map(|window| window.id).collect()
        ).collect();
        if let Some(profile) = &mut damage_profile {
            profile.gpu_forced |= u64::from(rebuilt);
        }
        // Rebuild changed output targets before acquiring their swapchain slots.
        // Reject before uploads or GPU admission. Do not
        // spend a vblank inside present() before returning completed client
        // leases; the normal retry path keeps input handling and credits live.
        self.admitted_outputs.clear();
        let mut ready_outputs = vec![false; outputs.len()];
        for (index, output) in outputs.iter().enumerate() {
            if output.damage == OutputDamage::Skip && !self.full_repaint_outputs[index] { continue; }
            if !self.output_can_admit(index) { continue; }
            self.target.select_surface(index).map_err(|_| "GPU output is missing")?;
            ready_outputs[index] = self.target.try_acquire_render_target().map_err(GpuCompositionError::Backend)?;
        }
        self.target.select_surface(0).map_err(|_| "GPU primary output is missing")?;
        if !ready_outputs.iter().any(|ready| *ready) { return Err(GpuCompositionError::DisplayBusy); }
        self.target.frame_timings = profiling.then(Default::default);
        let sampled_windows: Vec<_> = self.output_dependencies.iter().enumerate()
            .filter(|(index, _)| ready_outputs[*index]).flat_map(|(_, windows)| windows.iter().copied()).collect();
        let mut texture_uploads = Vec::new();
        self.prepare_scene_textures(scenes, windows, &sampled_windows, &mut texture_uploads)?;
        self.transfer_visible_imported_shm(windows, &sampled_windows)?;
        for &(id, geometry) in backdrops {
            if !self
                .backdrops
                .iter()
                .any(|(cached_id, cached, _)| *cached_id == id && *cached == geometry)
            {
                let textures = BackdropTextures::define(
                    self.target.resources.as_ref(),
                    geometry.source.2,
                    geometry.source.3,
                    geometry.radius,
                )
                .map_err(|_| "Failed to define backdrop textures")?;
                self.backdrops.push((id, geometry, textures));
            }
        }
        if let Some(profile) = &mut damage_profile {
            profile.invalid_caches = self
                .backdrops
                .iter()
                .filter(|(id, geometry, textures)| {
                    !textures.valid && backdrops.contains(&(*id, *geometry))
                })
                .count() as u64;
        }
        // A newly allocated/invalid lower cache can change its output even
        // without a new foreground commit. Include that dependency before
        // deciding whether an upper material's background remains reusable.
        let mut capture_damage = scene_damage.map(|damage| damage.to_vec());
        if let Some(damage) = &mut capture_damage {
            for (id, geometry, textures) in &self.backdrops {
                if !textures.valid && backdrops.contains(&(*id, *geometry)) {
                    damage.push(BackdropDamage {
                        rect: geometry.global_output(),
                        foreground: Some(*id),
                    });
                }
            }
        }
        // Preserve native content ownership when deciding whether background
        // pixels changed. Presentation repair/halo expansion below remains
        // conservative, but must not force a fresh capture of unchanged glass.
        let refresh = if backdrops.is_empty() {
            Vec::new()
        } else {
            let window_order: Vec<_> = windows.iter().map(|window| window.id).collect();
            backdrop_refreshes(
                backdrops,
                &window_order,
                if ready_outputs.iter().enumerate().any(|(index, ready)| *ready && self.full_repaint_outputs[index]) {
                    None
                } else {
                    capture_damage.as_deref()
                },
            )
        };
        for (id, geometry, textures) in &mut self.backdrops {
            if backdrops
                .iter()
                .zip(&refresh)
                .find(|((owner, current), _)| id == owner && geometry == current)
                .is_none_or(|(_, refresh)| *refresh)
            {
                textures.valid = false;
            }
        }
        if let Some(profile) = &mut damage_profile {
            profile.material_refreshes = self.backdrops.iter().filter(|(id, geometry, textures)|
                !textures.valid && backdrops.contains(&(*id, *geometry))).count() as u64;
            profile.requested_pixels = damage.map_or(
                u64::from(self.target.width) * u64::from(self.target.height),
                |rect| u64::from(rect.2) * u64::from(rect.3));
        }
        let mut uploaded_texture_indices = Vec::new();
        for window in windows.iter().filter(|window| sampled_windows.contains(&window.id)) {
            super::trace::set_gpu_window(window.id);
            // CPU-backed texture extent changes are detected before this loop
            // and cause a fresh private resource table. Each active window is
            // therefore re-uploaded into that replacement table here.
            let has_current_backing = window.presentation_pixels().is_ok();
            let has_direct_shm_texture = self.imported_shm_texture(window).is_some();
            if window.is_presented()
                && !scenes.iter().any(|view| view.scene.window_id == window.id)
                && !self.has_committed_shared_buffer(window.id)
                && !has_direct_shm_texture
                && has_current_backing
                && let Some((texture_index, upload)) = self.prepare_window_texture_upload(window)?
            {
                uploaded_texture_indices.push(texture_index);
                texture_uploads.push(upload);
            }
        }

        super::trace::set_gpu_window(0);
        let profile_synced = profiling.then(Instant::now);
        super::trace::set_compositor_stage(super::trace::STAGE_GPU_ENCODE);
        let clear_color = bgra_color(background);
        let mut recorded = Vec::with_capacity(outputs.len());
        let mut logical_damage = None;
        let mut published_backdrops = Vec::new();
        for (output_index, frame) in outputs.iter().enumerate() {
            if !ready_outputs[output_index] { continue; }
            let projection = OutputProjection { origin: frame.origin, scene: (frame.scene_width, frame.scene_height), native: (frame.width, frame.height) };
            let damage = match frame.damage {
                _ if self.full_repaint_outputs[output_index] => None,
                OutputDamage::Skip => continue,
                OutputDamage::Full => None,
                OutputDamage::Region(rect) => Some(rect),
            };
            let origin = frame.origin;
            let is_primary = output_index == 0;
            self.target
                .select_surface(output_index)
                .map_err(|_| "GPU output is missing")?;
            let local_materials = self
                    .backdrops
                    .iter()
                    .filter(|(id, geometry, _)| backdrops.contains(&(*id, *geometry)))
                    .filter_map(|(_, geometry, textures)| {
                        geometry
                            .relative_to(origin, frame.width, frame.height)
                            .map(|g| (g, textures))
                    })
                    .map(|(g, textures)| {
                        Ok(MaterialDamage {
                            source: PixelRect::new(
                                g.source.0 as u32,
                                g.source.1 as u32,
                                g.source.2,
                                g.source.3,
                            )?,
                            output: PixelRect::new(
                                g.output.0 as u32,
                                g.output.1 as u32,
                                g.output.2,
                                g.output.3,
                            )?,
                            refresh: !textures.valid,
                        })
                    })
                    .collect::<Result<Vec<_>, sgfx::ir::Error>>()
                    .map_err(|_| "Invalid output backdrop geometry")?;
            let output_materials = local_materials.as_slice();
            let output_shadows: Vec<_> = frame.overview_shadows
                .iter()
                .map(|&(r, rad, c)| (projection.rect(r), projection.radius(rad), c))
                .collect();
            let output_cards: Vec<_> = frame.overview_cards
                .iter()
                .map(|&(r, a, b)| (projection.rect(r), a, b))
                .collect();
            let output_remove_buttons: Vec<_> = frame.overview_remove_buttons
                .iter()
                .map(|&(id, r, h)| (id, projection.rect(r), h))
                .collect();
            let requested_clip = damage
                .map(|(x, y, width, height)| PixelRect::new(x, y, width, height))
                .transpose()
                .map_err(|_| "Invalid GPU composition damage")?;
            let full_area = PixelRect::new(0, 0, self.target.width, self.target.height)
                .map_err(|_| "Invalid GPU composition target area")?;
            // Only changed filter OUTPUT enters swapchain age history and capture
            // damage. A read halo is temporary coverage for rebuilding that cache.
            let logical_area = MaterialDamage::logical_region(
                requested_clip.unwrap_or(full_area),
                output_materials,
            )
            .map_err(|_| "Invalid backdrop logical damage")?;
            let aged_area = self
                .target
                .prepare_render_area(logical_area)
                .map_err(|_| "Failed to prepare GPU swapchain damage")?;
            let render_area = MaterialDamage::repair_region(aged_area, output_materials)
                .map_err(|_| "Invalid backdrop repair damage")?;
            if let Some(profile) = &mut damage_profile {
                profile.logical_pixels =
                    u64::from(logical_area.width()) * u64::from(logical_area.height());
                profile.aged_pixels = u64::from(aged_area.width()) * u64::from(aged_area.height());
            }
            let damage_clip = (render_area != full_area).then_some(render_area);
            let mut operations = if output_index == 0 {
                std::mem::take(&mut *self.operations.borrow_mut())
            } else {
                self.output_operations
                    .resize_with(outputs.len() - 1, Vec::new);
                std::mem::take(&mut self.output_operations[output_index - 1])
            };
            operations.clear();
            let mut backdrop_splits = Vec::new();
            let mut overview_backplates_drawn = false;
            for window in windows {
                if !window.is_presented() {
                    continue;
                }
                // Overview actors and shell surfaces belong to one output.
                // Only ordinary, untransformed windows may span its edge.
                if !window.presentation_cross_output && window.output_id != frame.id
                    && (window.presentation_transform.is_some()
                        || !window.presentation_instances.is_empty()
                        || matches!(window.window_type, WindowType::Desktop | WindowType::Taskbar
                            | WindowType::ShellBackground | WindowType::ShellChrome | WindowType::ShellPanel))
                {
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
                        &output_shadows,
                        self.target.width,
                        self.target.height,
                        damage_clip,
                    )?;
                    append_overview_backplates(
                        &mut operations,
                        &output_cards,
                        self.target.width,
                        self.target.height,
                        damage_clip,
                    )?;
                    overview_backplates_drawn = true;
                }
                for (index, (id, global_geometry, _)) in self.backdrops.iter().enumerate() {
                    let Some(geometry) =
                        global_geometry.relative_to(origin, frame.width, frame.height)
                    else {
                        continue;
                    };
                    if *id == window.id
                        && backdrops.contains(&(*id, *global_geometry))
                        && intersect_pixel_rects(
                            PixelRect::new(
                                geometry.output.0 as u32,
                                geometry.output.1 as u32,
                                geometry.output.2,
                                geometry.output.3,
                            )
                            .map_err(|_| "Invalid backdrop output")?,
                            render_area,
                        )
                        .is_some()
                    {
                        backdrop_splits.push((index, operations.len()));
                    }
                }
                self.append_window_projection(&mut operations, window, None, damage_clip, projection)?;
                for instance in &window.presentation_instances {
                    self.append_window_projection(
                        &mut operations,
                        window,
                        Some(*instance),
                        damage_clip,
                        projection,
                    )?;
                }
            }
            if !overview_backplates_drawn {
                append_overview_shadows(
                    &mut operations,
                    &output_shadows,
                    self.target.width,
                    self.target.height,
                    damage_clip,
                )?;
                append_overview_backplates(
                    &mut operations,
                    &output_cards,
                    self.target.width,
                    self.target.height,
                    damage_clip,
                )?;
            }
            append_overview_remove_buttons(
                &mut operations,
                &output_remove_buttons,
                self.target.width,
                self.target.height,
                damage_clip,
            )?;

            if let Some(outline) = resize_outline {
                append_outline(
                    &mut operations,
                    projection.rect(outline),
                    self.target.width,
                    self.target.height,
                    damage_clip,
                )?;
            }
            if let Some(rect) = navigation_indicator {
                append_overview_shadows(
                    &mut operations,
                    &[(projection.rect(rect), rect.3 / 2, [96, 96, 96, 210])],
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
            let (cursor_x, cursor_y, destination_width, destination_height) = projection.rect((cursor_x, cursor_y, cursor_width, cursor_height));
            if cursor_visible
                && let Some((destination, source)) = clipped_scaled_rect(
                    cursor_x, cursor_y, destination_width, destination_height, cursor_width, cursor_height,
                    self.target.width, self.target.height,
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
            let mut backdrop_passes = Vec::new();
            for &(index, split) in &backdrop_splits {
                let (_, global_geometry, textures) = &self.backdrops[index];
                let geometry = global_geometry
                    .relative_to(origin, frame.width, frame.height)
                    .ok_or("Backdrop belongs to a different output")?;
                let source = geometry.source;
                let output = geometry.output;
                let mut clips = Vec::new();
                let mut y = 0;
                while y < output.3 {
                    let (x, width) = geometry.row_span(y);
                    let mut end = y + 1;
                    while end < output.3 && geometry.row_span(end) == (x, width) {
                        end += 1;
                    }
                    if width > 0 {
                        clips.push(
                            PixelRect::new(x, output.1 as u32 + y, width, end - y)
                                .map_err(|_| "Invalid rounded backdrop mask")?,
                        );
                    }
                    y = end;
                }
                backdrop_passes.push(BackdropPass {
                    refresh: !textures.valid,
                    textures,
                    split,
                    clips,
                    radius: geometry.radius,
                    source: PixelRect::new(source.0 as u32, source.1 as u32, source.2, source.3)
                        .map_err(|_| "Invalid backdrop source")?,
                    output: PixelRect::new(output.0 as u32, output.1 as u32, output.2, output.3)
                        .map_err(|_| "Invalid backdrop output")?,
                });
            }

            if is_primary {
                logical_damage = Some((
                    logical_area.x(),
                    logical_area.y(),
                    logical_area.width(),
                    logical_area.height(),
                ));
            }
            published_backdrops.extend(
                backdrop_splits
                    .iter()
                    .map(|&(i, _)| (i, self.backdrops[i].1)),
            );
            recorded.push((
                output_index,
                self.target.texture,
                (self.target.width, self.target.height),
                render_area,
                (render_area != full_area).then_some(DisplayPresentRegion {
                    x: render_area.x(),
                    y: render_area.y(),
                    width: render_area.width(),
                    height: render_area.height(),
                }),
                operations,
                backdrop_passes,
            ));
        }
        let profile_encoded = profiling.then(Instant::now);
        let mut publication = PendingComposition {
            damage_profile,
            logical_damage,
            outputs: recorded.iter().map(|r| (r.0, r.4)).collect(),
            sampled_windows,
            timings: None,
            other_outputs_pending_at_admission: self.pending_frames.len(),
            shared: self
                .shared_windows
                .iter()
                .filter(|state| {
                    !windows.iter().any(|window| {
                        window.id == state.window_id && window.retains_native_transition_frame()
                    })
                })
                .cloned()
                .collect(),
            backdrops: published_backdrops,
            scenes: scenes
                .iter()
                .filter_map(|view| {
                    let entry = self
                        .scene_textures
                        .iter()
                        .find(|entry| entry.window_id == view.scene.window_id && entry.direct)?;
                    if !windows
                        .iter()
                        .any(|window| window.id == entry.window_id && window.is_presented())
                    {
                        return None;
                    }
                    Some(ScenePublication {
                        window_id: entry.window_id,
                        texture: entry.texture,
                        serial: view.scene.serial,
                        buffers: view
                            .scene
                            .layers
                            .iter()
                            .zip(&view.sources)
                            .map(|(layer, source)| {
                                source.gpu_buffer.is_none().then_some(layer.buffer_id)
                            })
                            .collect(),
                    })
                })
                .collect(),
            profile: [profile_start, profile_synced, profile_encoded, None],
            quads: recorded.iter().map(|r| r.5.len()).sum(),
            damage_pixels: recorded
                .iter()
                .map(|r| u64::from(r.3.width()) * u64::from(r.3.height()))
                .sum(),
        };
        let passes: Vec<_> = recorded
            .iter()
            .map(|r| QuadOutput {
                texture: r.1,
                extent: r.2,
                area: r.3,
                load: LoadOp::Clear(clear_color),
                operations: &r.5,
                backdrops: &r.6,
            })
            .collect();
        let submitted = if self.target.supports_tracked_submission() {
            self.quad_renderer
                .submit_outputs_with_uploads_tracked(&mut self.target, &texture_uploads, &passes)
                .map_err(|error| match error {
                    QuadSubmitError::Busy => GpuCompositionError::Busy,
                    QuadSubmitError::Recording(error) => GpuCompositionError::Backend(error),
                    QuadSubmitError::Execution(error) => {
                        GpuCompositionError::TrackedExecution(error)
                    }
                })
        } else {
            (|| {
                for (index, pass) in passes.iter().enumerate() {
                    self.target
                        .select_surface(recorded[index].0)
                        .map_err(|_| "GPU output is missing")?;
                    self.quad_renderer.submit_region_with_uploads(
                        &mut self.target,
                        pass.area,
                        pass.load,
                        if index == 0 { &texture_uploads } else { &[] },
                        pass.operations,
                        pass.backdrops,
                    )?;
                }
                Ok(Vec::new())
            })()
        };
        drop(passes);
        for (index, _, _, _, _, operations, _) in recorded {
            if index == 0 {
                *self.operations.borrow_mut() = operations;
            } else {
                self.output_operations[index - 1] = operations;
            }
        }
        self.target
            .select_surface(0)
            .map_err(|_| "GPU output is missing")?;
        let receipts = submitted?;
        self.admitted_outputs = publication.outputs.iter().map(|(index, _)| *index).collect();
        for &index in &self.admitted_outputs { self.full_repaint_outputs[index] = false; }
        for texture_index in uploaded_texture_indices {
            self.textures[texture_index].pending_damage = None;
        }
        publication.profile[3] = profiling.then(Instant::now);
        publication.timings = self.target.frame_timings.clone();
        if self.target.supports_tracked_submission() {
            let groups = publication.outputs.iter().map(|(index, _)| *index).zip(receipts).collect();
            self.target.queue_output_gpu_completions(groups).map_err(GpuCompositionError::Backend)?;
            // Publications and buffer leases retire independently per output.
            for &(index, region) in &publication.outputs {
                let readers = self.output_dependencies[index].clone();
                self.pending_frames.push(PendingComposition {
                    outputs: vec![(index, region)],
                    sampled_windows: readers.clone(),
                    timings: publication.timings.clone(),
                    other_outputs_pending_at_admission: publication.other_outputs_pending_at_admission,
                    logical_damage: if index == 0 { publication.logical_damage } else { None },
                    damage_profile: publication.damage_profile,
                    shared: publication.shared.iter().filter(|state| readers.contains(&state.window_id)).cloned().collect(),
                    backdrops: publication.backdrops.iter().copied().filter(|(i, _)| readers.contains(&self.backdrops[*i].0)).collect(),
                    scenes: publication.scenes.iter().filter(|scene| readers.contains(&scene.window_id)).cloned().collect(),
                    profile: publication.profile,
                    quads: publication.quads,
                    damage_pixels: publication.damage_pixels,
                });
                self.full_repaint_outputs[index] = false;
            }
            return Err(GpuCompositionError::Pending);
        }
        let logical_damage = publication.logical_damage;
        self.publish_completed_frame(
            &outputs.iter().map(|f| f.display).collect::<Vec<_>>(),
            publication,
        )
        .map(|releases| (releases, logical_damage))
    }

    fn publish_completed_frame(
        &mut self,
        displays: &[&DisplaySurface],
        frame: PendingComposition,
    ) -> Result<Vec<SgfxCommitToken>, GpuCompositionError> {
        super::trace::set_compositor_stage(super::trace::STAGE_GPU_PRESENT);
        // Preserve the existing stage-file wall intervals: submit spans fence
        // retirement (now overlapped with input), present measures enqueue only.
        let previous_timings = self.target.frame_timings.take();
        self.target.frame_timings = frame.timings;
        let gpu_retired = frame.profile[3].map(|_| Instant::now());
        for &(index, region) in &frame.outputs {
            let display = displays.get(index).ok_or(GpuCompositionError::Backend(
                "GPU output disappeared before presentation",
            ))?;
            self.target
                .select_surface(index)
                .map_err(|_| GpuCompositionError::Backend("GPU output is missing"))?;
            self.target
                .present(display, region)
                .map_err(GpuCompositionError::Backend)?;
            self.completed_outputs.push(index);
        }
        self.target
            .select_surface(0)
            .map_err(|_| GpuCompositionError::Backend("GPU primary output is missing"))?;
        let presented = self.frame_profile.as_ref().map(|_| Instant::now());
        for (index, geometry) in frame.backdrops {
            if let Some((owner, current, textures)) = self.backdrops.get_mut(index) {
                if *current == geometry && self.active_backdrops.contains(&(*owner, geometry)) {
                    textures.valid = true;
                }
            }
        }
        for update in frame.scenes {
            if let Some(entry) = self.scene_textures.iter_mut().find(|entry| {
                entry.window_id == update.window_id
                    && entry.texture == update.texture
                    && entry.direct
            }) {
                entry.serial = Some(update.serial);
                entry.uploaded_source_buffers = update.buffers;
            }
        }
        let mut releases = Vec::new();
        for snapshot in frame.shared {
            if let Some(state) = self
                .shared_windows
                .iter_mut()
                .find(|state| state.window_id == snapshot.window_id)
            {
                releases.extend(
                    promote_completed_shared_frame(state, &snapshot)
                        .map_err(GpuCompositionError::Backend)?
                        .into_iter()
                        .flatten(),
                );
            }
        }
        for texture in &mut self.textures {
            if texture.window_id.is_some_and(|id| {
                self.shared_windows
                    .iter()
                    .any(|state| state.window_id == id && state.presented.is_some())
            }) {
                texture.window_id = None;
                texture.pending_damage = None;
            }
        }
        self.schedule_retired_texture_rebuild();
        let [start, synced, encoded, _admitted] = frame.profile;
        let submitted = gpu_retired;
        if let (
            Some(profile),
            Some(start),
            Some(synced),
            Some(encoded),
            Some(submitted),
            Some(presented),
            Some(timings),
        ) = (
            &mut self.frame_profile,
            start,
            synced,
            encoded,
            submitted,
            presented,
            &self.target.frame_timings,
        ) {
            if profile
                .record(
                    [start, synced, encoded, submitted, presented, Instant::now()],
                    timings,
                    frame.quads,
                    frame.damage_pixels,
                    frame.damage_profile.as_ref(),
                    frame.outputs[0].0,
                    frame.other_outputs_pending_at_admission,
                )
                .is_err()
            {
                self.frame_profile = None;
                std::println!("[SWS_PROFILE] frame file write failed; profiling disabled");
            }
        }
        self.target.frame_timings = previous_timings;
        for window in core::mem::take(&mut self.deferred_texture_removals) {
            self.remove_window_texture(window)
                .map_err(GpuCompositionError::Backend)?;
        }
        for window in core::mem::take(&mut self.deferred_window_removals) {
            self.remove_window(window)
                .map_err(GpuCompositionError::Backend)?;
        }
        Ok(releases)
    }

    pub(super) fn has_committed_shared_buffer(&self, window_id: WindowId) -> bool {
        self.shared_windows.iter().any(|state| {
            state.window_id == window_id && (state.pending.is_some() || state.presented.is_some())
        })
    }

    fn committed_shared_texture(&self, window: &Window) -> Option<(TextureId, u32, u32)> {
        let state = self
            .shared_windows
            .iter()
            .find(|state| state.window_id == window.id)?;
        let identity = if window.retains_native_transition_frame() {
            state.presented.or(state.pending)
        } else {
            state.pending.or(state.presented)
        }?
        .identity;
        self.shared_textures
            .iter()
            .find(|entry| entry.identity == identity)
            .map(|entry| (entry.texture, entry.width, entry.height))
    }

    /// Keep the latest offscreen shell image without filling the client's pool.
    /// Event processing runs after composition has completed (or admission has
    /// retired its work). The hidden surface is not sampled by a new display
    /// frame, so replacing its retained image needs no display presentation.
    pub(super) fn take_prepared_shell_releases(&mut self, window: &Window) -> Vec<SgfxCommitToken> {
        if self.window_in_flight(window.id) || !window.prepares_while_workspace_hidden() || window.retains_native_transition_frame() {
            return Vec::new();
        }
        self.shared_windows
            .iter_mut()
            .find(|state| state.window_id == window.id)
            .map(promote_pending_shared_frame)
            .into_iter()
            .flatten()
            .flatten()
            .collect()
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

    fn prepare_scene_textures<'a>(
        &mut self,
        scenes: &[SceneView<'a>],
        windows: &[Window],
        sampled: &[WindowId],
        direct_uploads: &mut Vec<TextureUpload<'a>>,
    ) -> Result<(), GpuCompositionError> {
        for view in scenes {
            let scene = view.scene;
            if !sampled.contains(&scene.window_id) { continue; }
            if scene.layers.is_empty()
                || !windows
                    .iter()
                    .any(|w| w.id == scene.window_id && w.is_presented())
            {
                continue;
            }
            let index = match self
                .scene_textures
                .iter()
                .position(|entry| entry.window_id == scene.window_id)
            {
                Some(index) => index,
                None => {
                    let texture = define_scene_texture(
                        self.target.resources.as_ref(),
                        scene.width,
                        scene.height,
                    )
                    .map_err(|_| "Failed to allocate GPU scene target")?;
                    let mut sources = Vec::with_capacity(view.sources.len());
                    for source in &view.sources {
                        let texture = if source.gpu_buffer.is_some() {
                            None
                        } else {
                            Some(
                                define_bgra_texture(
                                    self.target.resources.as_ref(),
                                    source.width,
                                    source.height,
                                )
                                .map_err(|_| "Failed to allocate GPU scene source")?,
                            )
                        };
                        sources.push((source.width, source.height, texture));
                    }
                    self.scene_textures.push(SceneTexture {
                        window_id: scene.window_id,
                        width: scene.width,
                        height: scene.height,
                        serial: None,
                        texture,
                        direct: false,
                        direct_layers: Vec::new(),
                        uploaded_source_buffers: vec![None; sources.len()],
                        sources,
                    });
                    self.scene_textures.len() - 1
                }
            };
            let inline = windows
                .iter()
                .find(|window| window.id == scene.window_id)
                .is_some_and(|window| {
                    let (x, y, width, height) = window.presentation_geometry();
                    window.presentation_transform.is_none()
                        && window.presentation_instances.is_empty()
                        && window.presentation_opacity() == 1.0
                        && width == scene.width
                        && height == scene.height
                        && self.output_rects.iter().any(|&(ox, oy, ow, oh)| {
                            i64::from(x) >= i64::from(ox)
                                && i64::from(y) >= i64::from(oy)
                                && i64::from(x) + i64::from(width) <= i64::from(ox) + i64::from(ow)
                                && i64::from(y) + i64::from(height) <= i64::from(oy) + i64::from(oh)
                        })
                });
            let entry = &self.scene_textures[index];
            if entry.serial == Some(scene.serial) && entry.direct == inline {
                continue;
            }
            if inline {
                let entry = &mut self.scene_textures[index];
                entry.direct_layers.clear();
                let white = Color::rgba(1.0, 1.0, 1.0, 1.0).map_err(|_| "Invalid scene tint")?;
                for (source_index, ((layer, source), &(width, height, upload_texture))) in scene
                    .layers
                    .iter()
                    .zip(&view.sources)
                    .zip(&entry.sources)
                    .enumerate()
                {
                    let area = PixelRect::new(0, 0, width, height)
                        .map_err(|_| "Invalid scene source extent")?;
                    let texture = if let Some((client_id, buffer_id)) = source.gpu_buffer {
                        self.extension_gpu_textures
                            .iter()
                            .find(|image| {
                                (image.client_id, image.buffer_id, image.width, image.height)
                                    == (client_id, buffer_id, width, height)
                            })
                            .ok_or("GPU scene image is not registered")?
                            .texture
                    } else {
                        let texture = upload_texture.ok_or("Missing SHM scene texture")?;
                        // A retained SHM buffer remains leased and immutable.
                        // Reuse its uploaded texture while only the GPU layer flips.
                        if entry.uploaded_source_buffers[source_index] != Some(layer.buffer_id) {
                            direct_uploads.push(TextureUpload {
                                texture,
                                destination: area,
                                stride: source.stride as u32,
                                bytes: source.bytes,
                            });
                        }
                        texture
                    };
                    entry.direct_layers.push(Quad::SampledUv {
                        rect: SampledRect {
                            texture,
                            texture_width: width,
                            texture_height: height,
                            destination: PixelRect::new(
                                layer.x as u32,
                                layer.y as u32,
                                layer.width,
                                layer.height,
                            )
                            .map_err(|_| "Invalid scene layer extent")?,
                            source: area,
                            tint: white,
                            ignore_source_alpha: source.opaque,
                            clip: None,
                        },
                        uv: source_uv(layer, width, height),
                        premultiplied: true,
                    });
                }
                entry.direct = true;
                entry.serial = None;
                continue;
            }
            let mut uploads = Vec::with_capacity(view.sources.len());
            let mut quads = Vec::with_capacity(view.sources.len());
            let white = Color::rgba(1.0, 1.0, 1.0, 1.0).map_err(|_| "Invalid scene tint")?;
            for ((layer, source), &(width, height, upload_texture)) in
                scene.layers.iter().zip(&view.sources).zip(&entry.sources)
            {
                let area = PixelRect::new(0, 0, width, height)
                    .map_err(|_| "Invalid scene source extent")?;
                let texture = if let Some((client_id, buffer_id)) = source.gpu_buffer {
                    self.extension_gpu_textures
                        .iter()
                        .find(|image| {
                            (image.client_id, image.buffer_id, image.width, image.height)
                                == (client_id, buffer_id, width, height)
                        })
                        .ok_or("GPU scene image is not registered")?
                        .texture
                } else {
                    upload_texture.ok_or("Missing SHM scene texture")?
                };
                if source.gpu_buffer.is_none() {
                    uploads.push(TextureUpload {
                        texture,
                        destination: area,
                        stride: source.stride as u32,
                        bytes: source.bytes,
                    });
                }
                quads.push(Quad::SampledUv {
                    rect: SampledRect {
                        texture,
                        texture_width: width,
                        texture_height: height,
                        destination: PixelRect::new(
                            layer.x as u32,
                            layer.y as u32,
                            layer.width,
                            layer.height,
                        )
                        .map_err(|_| "Invalid scene layer extent")?,
                        source: area,
                        tint: white,
                        ignore_source_alpha: source.opaque,
                        clip: None,
                    },
                    uv: source_uv(layer, width, height),
                    premultiplied: true,
                });
            }
            if self.target.supports_tracked_submission() {
                let area = PixelRect::new(0, 0, scene.width, scene.height)
                    .map_err(|_| "Invalid scene extent")?;
                self.quad_renderer
                    .submit_texture_with_uploads_tracked(
                        &mut self.target,
                        entry.texture,
                        (scene.width, scene.height),
                        area,
                        LoadOp::Clear(
                            Color::rgba(0.0, 0.0, 0.0, 0.0).map_err(|_| "Invalid scene clear")?,
                        ),
                        &uploads,
                        &quads,
                        &[],
                    )
                    .map_err(|error| match error {
                        QuadSubmitError::Busy => GpuCompositionError::Busy,
                        QuadSubmitError::Recording(error) => GpuCompositionError::Backend(error),
                        QuadSubmitError::Execution(error) => {
                            GpuCompositionError::TrackedExecution(error)
                        }
                    })?;
            } else {
                self.quad_renderer.submit_texture_with_uploads(
                    &mut self.target,
                    entry.texture,
                    (scene.width, scene.height),
                    &uploads,
                    &quads,
                )?;
            }
            self.scene_textures[index].direct = false;
            self.scene_textures[index].serial = Some(scene.serial);
        }
        Ok(())
    }

    fn prepare_window_texture_upload<'a>(
        &mut self,
        window: &'a Window,
    ) -> Result<Option<(usize, TextureUpload<'a>)>, &'static str> {
        let pixels = window.presentation_pixels().ok();
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
            return Ok(None);
        };
        let texture = entry.texture;
        let damage_rect = PixelRect::new(damage.0, damage.1, damage.2, damage.3)
            .map_err(|_| "Invalid GPU window damage")?;
        let source = pixels.damage_bytes(damage.0, damage.1, damage.2, damage.3)?;
        Ok(Some((
            texture_index,
            TextureUpload {
                texture,
                destination: damage_rect,
                stride: pixels.stride(),
                bytes: source,
            },
        )))
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
    ) -> Result<bool, &'static str> {
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
                    window.presentation_pixels().ok().is_some_and(|pixels| {
                        pixels.width() != texture.width || pixels.height() != texture.height
                    })
                })
        });
        if !self.rebuild_pending
            && !cursor_layout_changed
            && !texture_extent_changed
            && !self.target.retired_imports_need_rebuild()
        {
            self.drain_for_rebuild = false;
            return Ok(false);
        }

        if self.has_pending_frame() {
            self.drain_for_rebuild = true;
            return Ok(false);
        }
        self.drain_for_rebuild = false;
        let extents = self
            .rebuild_extents
            .clone()
            .unwrap_or_else(|| self.target.surface_extents());
        let mut target = open_composition_target(&extents)?;
        let quad_renderer =
            QuadRenderer::define(target.resources.as_ref(), COMPOSITION_QUAD_CAPACITY)
                .map_err(|_| "Failed to redefine GPU composition resources")?;
        let cursor_images = create_cursor_images(&mut target, cursor)?;
        let mut extension_gpu_ids = Vec::with_capacity(self.extension_gpu_textures.len());
        for entry in &self.extension_gpu_textures {
            extension_gpu_ids.push(
                target
                    .import_shared_bgra_texture(
                        entry.width,
                        entry.height,
                        entry
                            .handle
                            .duplicate()
                            .map_err(|_| "GPU scene image duplication failed")?,
                    )
                    .map_err(|_| "GPU scene image reimport failed")?,
            );
        }
        let mut imported_shm_texture_ids = Vec::new();
        imported_shm_texture_ids
            .try_reserve_exact(self.imported_shm_textures.len())
            .map_err(|_| "Failed to reserve rebuilt extension SHM texture mappings")?;
        for imported in &self.imported_shm_textures {
            let handle = imported
                .image
                .as_handle()
                .duplicate()
                .map_err(|_| "Failed to duplicate imported extension SHM image")?;
            let texture = target
                .import_shared_bgra_texture(imported.width, imported.height, handle)
                .map_err(|_| "Failed to reimport extension SHM image")?;
            imported_shm_texture_ids.push(texture);
        }
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
        self.backdrops.clear();
        self.cursor_images = cursor_images;
        self.textures.clear();
        self.scene_textures.clear();
        for (entry, texture) in self
            .extension_gpu_textures
            .iter_mut()
            .zip(extension_gpu_ids)
        {
            entry.texture = texture;
        }
        for (imported, texture) in self
            .imported_shm_textures
            .iter_mut()
            .zip(imported_shm_texture_ids)
        {
            imported.texture = texture;
        }
        for (shared, texture) in self.shared_textures.iter_mut().zip(shared_texture_ids) {
            shared.texture = texture;
        }
        self.rebuild_pending = false;
        self.rebuild_extents = None;
        self.force_full_repaint = true;
        Ok(true)
    }
}

/// Promote a completed display frame or an offscreen shell preparation frame.
///
/// The previously presented commit remains retained until this transition.
/// This is also the generation boundary used during a window resize: SWS may
/// sample the old extent until the replacement extent has actually presented.
fn shared_lease_snapshot(state: &SharedWindowState) -> [Option<SgfxCommitToken>; 5] {
    [
        state.presented,
        state.pending,
        state.retire_after_present[0],
        state.retire_after_present[1],
        state.retire_after_present[2],
    ]
}

fn install_shared_lease_snapshot(
    state: &mut SharedWindowState,
    leases: [Option<SgfxCommitToken>; 5],
) {
    [
        state.presented,
        state.pending,
        state.retire_after_present[0],
        state.retire_after_present[1],
        state.retire_after_present[2],
    ] = leases;
}

fn promote_pending_shared_frame(state: &mut SharedWindowState) -> [Option<SgfxCommitToken>; 5] {
    // The caller's offscreen-shell policy already establishes that no display
    // frame samples this preparation. Use the same exact snapshot transition.
    let snapshot = shared_lease_snapshot(state);
    let (current, released) = crate::sgfx_ir_support::retire_shared_snapshot(snapshot, snapshot)
        .expect("current shared frame snapshot must retain its own leases");
    install_shared_lease_snapshot(state, current);
    released
}

fn promote_completed_shared_frame(
    state: &mut SharedWindowState,
    submitted: &SharedWindowState,
) -> Result<[Option<SgfxCommitToken>; 5], &'static str> {
    if state.compositor_epoch != submitted.compositor_epoch {
        return Err("Shared frame epoch changed before completion");
    }
    let (current, released) = crate::sgfx_ir_support::retire_shared_snapshot(
        shared_lease_snapshot(state),
        shared_lease_snapshot(submitted),
    )?;
    install_shared_lease_snapshot(state, current);
    Ok(released)
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

    #[test]
    fn tracked_failure_invalidates_shared_images_without_proving_retirement() {
        for error in [
            FrameSubmissionError::Pending,
            FrameSubmissionError::Invalidated,
            FrameSubmissionError::Completion(sgfx::Error::InvalidBackendPreference),
            FrameSubmissionError::Submit(sgfx::backend::SubmitError::Busy),
        ] {
            assert!(GpuCompositionError::TrackedExecution(error).invalidates_shared_images());
        }
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
            retire_after_present: [None; 3],
        };

        assert_eq!(state.presented, Some(old_presented));
        assert_eq!(state.pending, Some(replacement));

        let releases = promote_pending_shared_frame(&mut state);

        assert_eq!(releases, [Some(old_presented), None, None, None, None]);
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
        Quad::SampledUv {
            mut rect,
            uv,
            premultiplied,
        } => {
            rect.clip = clip;
            Quad::SampledUv {
                rect,
                uv,
                premultiplied,
            }
        }
        Quad::SampledOffset { mut rect, offset } => {
            rect.clip = clip;
            Quad::SampledOffset { rect, offset }
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
