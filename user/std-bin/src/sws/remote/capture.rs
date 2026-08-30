//! Client-owned output capture buffers and per-session damage accumulation.

use scarlet_os::handle::Handle;
use scarlet_os::handle::capability::memory_mapping::{MemoryMappingOps, flags as mmap_flags};
use scarlet_os::ipc::{SharedMemory, permissions};
use std::collections::BTreeMap;
use std::vec::Vec;
use sws_remote_protocol::{CaptureFormat, MAX_DAMAGE_RECTS, Rect, ServerMessage};

use super::super::damage::PresentDamage;
use super::super::gpu_compositor::GpuCompositor;
use super::server;

const MAX_CAPTURE_BYTES: usize = 256 * 1024 * 1024;

/// One mapped client-owned capture destination.
struct CaptureBuffer {
    _shared_memory: SharedMemory,
    mapped_address: usize,
    mapped_length: usize,
    width: u32,
    height: u32,
    stride: u32,
    format: CaptureFormat,
    initialized: bool,
}

impl Drop for CaptureBuffer {
    fn drop(&mut self) {
        let _ = MemoryMappingOps::munmap(self.mapped_address, self.mapped_length);
    }
}

/// The single capture session exported by SWS.
pub(crate) struct CaptureSession {
    owner_client_id: Option<usize>,
    output_id: u32,
    output_width: u32,
    output_height: u32,
    sequence: u64,
    has_completed_frame: bool,
    has_pending_frame: bool,
    pending_damage: PresentDamage,
    buffers: BTreeMap<u32, CaptureBuffer>,
    last_buffer_id: Option<u32>,
}

impl CaptureSession {
    /// Construct an inactive capture session for the primary output.
    ///
    /// # Arguments
    ///
    /// * `width` - Initial output width in physical pixels.
    /// * `height` - Initial output height in physical pixels.
    ///
    /// # Returns
    ///
    /// An inactive session ready to accept `CreateCapture`.
    pub(crate) fn new(width: u32, height: u32) -> Self {
        Self {
            owner_client_id: None,
            output_id: 0,
            output_width: width,
            output_height: height,
            sequence: 0,
            has_completed_frame: false,
            has_pending_frame: false,
            pending_damage: None,
            buffers: BTreeMap::new(),
            last_buffer_id: None,
        }
    }

    /// Return whether a client owns the active capture session.
    ///
    /// # Arguments
    ///
    /// * `client_id` - Remote transport connection identifier.
    ///
    /// # Returns
    ///
    /// `true` when the connection may capture and inject input.
    pub(crate) fn is_owner(&self, client_id: usize) -> bool {
        self.owner_client_id == Some(client_id)
    }

    /// Create or re-create the caller's primary-output capture session.
    ///
    /// # Arguments
    ///
    /// * `client_id` - Requesting transport connection.
    /// * `output_id` - Requested SWS output identifier.
    ///
    /// # Returns
    ///
    /// `true` when the session was created. Only output zero and one owner are
    /// accepted by the initial implementation.
    pub(crate) fn create(&mut self, client_id: usize, output_id: u32) -> bool {
        if output_id != 0 || self.owner_client_id.is_some_and(|owner| owner != client_id) {
            return false;
        }

        self.owner_client_id = Some(client_id);
        self.output_id = output_id;
        self.buffers.clear();
        self.last_buffer_id = None;
        self.pending_damage = None;
        self.has_pending_frame = self.has_completed_frame;
        server::send_to_client(
            client_id,
            ServerMessage::OutputChanged {
                width: self.output_width,
                height: self.output_height,
            },
        );
        if self.has_completed_frame {
            server::send_to_client(
                client_id,
                ServerMessage::FrameAvailable {
                    sequence: self.sequence,
                },
            );
        }
        true
    }

    /// Release all state owned by a disconnected client.
    ///
    /// # Arguments
    ///
    /// * `client_id` - Disconnected transport connection.
    pub(crate) fn disconnect(&mut self, client_id: usize) {
        if !self.is_owner(client_id) {
            return;
        }
        self.owner_client_id = None;
        self.buffers.clear();
        self.last_buffer_id = None;
        self.has_pending_frame = false;
        self.pending_damage = None;
    }

    /// Register and map one client-owned capture buffer.
    ///
    /// # Arguments
    ///
    /// * `client_id` - Owning transport connection.
    /// * `buffer_id` - Connection-local buffer identifier.
    /// * `width` - Buffer width.
    /// * `height` - Buffer height.
    /// * `stride` - Bytes between rows.
    /// * `format` - Capture pixel format.
    /// * `handle` - Transferred shared-memory capability.
    ///
    /// # Returns
    ///
    /// Success after the complete destination has been mapped writable.
    pub(crate) fn register_buffer(
        &mut self,
        client_id: usize,
        buffer_id: u32,
        width: u32,
        height: u32,
        stride: u32,
        format: CaptureFormat,
        handle: Handle,
    ) -> Result<(), &'static str> {
        if !self.is_owner(client_id) {
            return Err("Remote client does not own the capture session");
        }
        if width != self.output_width || height != self.output_height || width == 0 || height == 0 {
            return Err("Capture buffer dimensions do not match the output");
        }
        let row_bytes = width
            .checked_mul(format.bytes_per_pixel())
            .ok_or("Capture row size overflow")?;
        if stride < row_bytes {
            return Err("Capture buffer stride is too small");
        }
        let mapped_length = usize::try_from(
            u64::from(stride)
                .checked_mul(u64::from(height))
                .ok_or("Capture buffer size overflow")?,
        )
        .map_err(|_| "Capture buffer size is unsupported")?;
        if mapped_length == 0 || mapped_length > MAX_CAPTURE_BYTES {
            return Err("Capture buffer size is unsupported");
        }

        let shared_memory =
            SharedMemory::from_handle(handle).map_err(|_| "Capture handle is not shared memory")?;
        let mapped_address = shared_memory
            .as_handle()
            .as_memory_mapping()
            .map_err(|_| "Capture buffer cannot be mapped")?
            .mmap(
                0,
                mapped_length,
                permissions::READ_WRITE,
                mmap_flags::SHARED,
                0,
            )
            .map_err(|_| "Capture buffer mapping failed")?;

        self.buffers.insert(
            buffer_id,
            CaptureBuffer {
                _shared_memory: shared_memory,
                mapped_address,
                mapped_length,
                width,
                height,
                stride,
                format,
                initialized: false,
            },
        );
        Ok(())
    }

    /// Record a successfully presented output frame and notify the owner.
    ///
    /// Damage is accumulated until a `RequestFrame` is fulfilled, so an idle
    /// client may skip arbitrarily many presents without losing updates.
    ///
    /// # Arguments
    ///
    /// * `damage` - Regions changed by this completed present, or full output.
    pub(crate) fn frame_presented(&mut self, damage: &PresentDamage) {
        self.sequence = self.sequence.wrapping_add(1);
        if self.sequence == 0 {
            self.sequence = 1;
        }
        self.has_completed_frame = true;
        let Some(client_id) = self.owner_client_id else {
            return;
        };
        if self.has_pending_frame {
            merge_damage(&mut self.pending_damage, damage.clone());
        } else {
            self.pending_damage = damage.clone();
            self.has_pending_frame = true;
        }
        server::send_to_client(
            client_id,
            ServerMessage::FrameAvailable {
                sequence: self.sequence,
            },
        );
    }

    /// Invalidate registered buffers after an output resize.
    ///
    /// The following completed present will advertise the first capturable
    /// frame at the new dimensions.
    ///
    /// # Arguments
    ///
    /// * `width` - New output width.
    /// * `height` - New output height.
    pub(crate) fn output_changed(&mut self, width: u32, height: u32) {
        self.output_width = width;
        self.output_height = height;
        self.buffers.clear();
        self.last_buffer_id = None;
        self.pending_damage = None;
        self.has_completed_frame = false;
        self.has_pending_frame = false;
        if let Some(client_id) = self.owner_client_id {
            server::send_to_client(client_id, ServerMessage::OutputChanged { width, height });
        }
    }

    /// Copy the current CPU-compositor backbuffer into a registered destination.
    ///
    /// # Arguments
    ///
    /// * `client_id` - Requesting connection.
    /// * `buffer_id` - Destination buffer identifier.
    /// * `backbuffer` - Persistent tightly packed BGRA compositor output.
    /// * `backbuffer_stride` - Source row stride in bytes.
    ///
    /// # Returns
    ///
    /// Success after the requested damage has been copied and `FrameReady` queued.
    pub(crate) fn capture_cpu(
        &mut self,
        client_id: usize,
        buffer_id: u32,
        backbuffer: &[u8],
        backbuffer_stride: u32,
    ) -> Result<(), &'static str> {
        if !self.is_owner(client_id) {
            return Err("Remote client does not own the capture session");
        }
        let buffer = self
            .buffers
            .get_mut(&buffer_id)
            .ok_or("Unknown remote capture buffer")?;
        if buffer.width != self.output_width
            || buffer.height != self.output_height
            || buffer.format != CaptureFormat::Bgra8888
        {
            return Err("Remote capture buffer is stale");
        }

        let force_full = !buffer.initialized || self.last_buffer_id != Some(buffer_id);
        let damage = if force_full || !self.has_pending_frame {
            vec![Rect::new(0, 0, self.output_width, self.output_height)]
        } else {
            protocol_damage(&self.pending_damage, self.output_width, self.output_height)
        };
        copy_bgra_damage(
            backbuffer,
            backbuffer_stride,
            buffer,
            &damage,
            self.output_width,
            self.output_height,
        )?;

        buffer.initialized = true;
        self.last_buffer_id = Some(buffer_id);
        self.has_pending_frame = false;
        self.pending_damage = Some(Vec::new());
        server::send_to_client(
            client_id,
            ServerMessage::FrameReady {
                buffer_id,
                sequence: self.sequence,
                damage,
            },
        );
        Ok(())
    }

    /// Read the current SGFX presentation target into a registered destination.
    ///
    /// # Arguments
    ///
    /// * `client_id` - Requesting connection.
    /// * `buffer_id` - Destination buffer identifier.
    /// * `gpu` - Active SGFX compositor retaining the last presented texture.
    ///
    /// # Returns
    ///
    /// Success after requested regions have completed GPU readback and
    /// `FrameReady` has been queued.
    pub(crate) fn capture_gpu(
        &mut self,
        client_id: usize,
        buffer_id: u32,
        gpu: &GpuCompositor,
    ) -> Result<(), &'static str> {
        if !self.is_owner(client_id) {
            return Err("Remote client does not own the capture session");
        }
        let buffer = self
            .buffers
            .get_mut(&buffer_id)
            .ok_or("Unknown remote capture buffer")?;
        if buffer.width != self.output_width
            || buffer.height != self.output_height
            || buffer.format != CaptureFormat::Bgra8888
        {
            return Err("Remote capture buffer is stale");
        }

        let force_full = !buffer.initialized || self.last_buffer_id != Some(buffer_id);
        let damage = if force_full || !self.has_pending_frame {
            vec![Rect::new(0, 0, self.output_width, self.output_height)]
        } else {
            protocol_damage(&self.pending_damage, self.output_width, self.output_height)
        };
        // SAFETY: `CaptureBuffer` owns this writable mapping for its entire
        // lifetime. The remote client observes it only after `FrameReady`, and
        // the compositor serializes all capture requests on this thread.
        let destination = unsafe {
            core::slice::from_raw_parts_mut(buffer.mapped_address as *mut u8, buffer.mapped_length)
        };
        gpu.capture_bgra(destination, buffer.stride, &damage)?;

        buffer.initialized = true;
        self.last_buffer_id = Some(buffer_id);
        self.has_pending_frame = false;
        self.pending_damage = Some(Vec::new());
        server::send_to_client(
            client_id,
            ServerMessage::FrameReady {
                buffer_id,
                sequence: self.sequence,
                damage,
            },
        );
        Ok(())
    }
}

fn protocol_damage(damage: &PresentDamage, width: u32, height: u32) -> Vec<Rect> {
    match damage {
        None => vec![Rect::new(0, 0, width, height)],
        Some(rects) => rects
            .iter()
            .filter_map(|rect| clamp_rect(*rect, width, height))
            .take(MAX_DAMAGE_RECTS)
            .collect(),
    }
}

fn clamp_rect(rect: (i32, i32, u32, u32), width: u32, height: u32) -> Option<Rect> {
    let x0 = i64::from(rect.0).max(0).min(i64::from(width));
    let y0 = i64::from(rect.1).max(0).min(i64::from(height));
    let x1 = i64::from(rect.0)
        .saturating_add(i64::from(rect.2))
        .max(0)
        .min(i64::from(width));
    let y1 = i64::from(rect.1)
        .saturating_add(i64::from(rect.3))
        .max(0)
        .min(i64::from(height));
    (x1 > x0 && y1 > y0)
        .then(|| Rect::new(x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
}

fn merge_damage(accumulated: &mut PresentDamage, next: PresentDamage) {
    match (accumulated, next) {
        (current @ Some(_), None) => *current = None,
        (Some(current), Some(next)) => {
            for rect in next {
                push_damage(current, rect);
            }
        }
        (None, _) => {}
    }
}

fn push_damage(rects: &mut Vec<(i32, i32, u32, u32)>, rect: (i32, i32, u32, u32)) {
    if rect.2 == 0 || rect.3 == 0 {
        return;
    }
    for existing in rects.iter_mut() {
        if should_merge(*existing, rect) {
            *existing = union_rect(*existing, rect);
            return;
        }
    }
    if rects.len() < MAX_DAMAGE_RECTS {
        rects.push(rect);
        return;
    }
    if let Some(first) = rects.first_mut() {
        *first = union_rect(*first, rect);
    }
}

fn should_merge(a: (i32, i32, u32, u32), b: (i32, i32, u32, u32)) -> bool {
    let union = union_rect(a, b);
    area(union) <= area(a).saturating_add(area(b)).saturating_mul(2)
}

fn area(rect: (i32, i32, u32, u32)) -> u64 {
    u64::from(rect.2).saturating_mul(u64::from(rect.3))
}

fn union_rect(a: (i32, i32, u32, u32), b: (i32, i32, u32, u32)) -> (i32, i32, u32, u32) {
    let x0 = i64::from(a.0).min(i64::from(b.0));
    let y0 = i64::from(a.1).min(i64::from(b.1));
    let x1 = i64::from(a.0)
        .saturating_add(i64::from(a.2))
        .max(i64::from(b.0).saturating_add(i64::from(b.2)));
    let y1 = i64::from(a.1)
        .saturating_add(i64::from(a.3))
        .max(i64::from(b.1).saturating_add(i64::from(b.3)));
    (
        x0 as i32,
        y0 as i32,
        x1.saturating_sub(x0) as u32,
        y1.saturating_sub(y0) as u32,
    )
}

fn copy_bgra_damage(
    source: &[u8],
    source_stride: u32,
    destination: &CaptureBuffer,
    damage: &[Rect],
    width: u32,
    height: u32,
) -> Result<(), &'static str> {
    let required_source = usize::try_from(
        u64::from(source_stride)
            .checked_mul(u64::from(height))
            .ok_or("Capture source size overflow")?,
    )
    .map_err(|_| "Capture source size is unsupported")?;
    if source_stride < width.saturating_mul(4) || source.len() < required_source {
        return Err("CPU capture source is too small");
    }

    for rect in damage {
        if rect.width == 0
            || rect.height == 0
            || rect.x.saturating_add(rect.width) > width
            || rect.y.saturating_add(rect.height) > height
        {
            continue;
        }
        let row_bytes =
            usize::try_from(u64::from(rect.width) * 4).map_err(|_| "Capture row is unsupported")?;
        for row in 0..rect.height {
            let source_offset = usize::try_from(
                u64::from(rect.y + row) * u64::from(source_stride) + u64::from(rect.x) * 4,
            )
            .map_err(|_| "Capture source offset is unsupported")?;
            let destination_offset = usize::try_from(
                u64::from(rect.y + row) * u64::from(destination.stride) + u64::from(rect.x) * 4,
            )
            .map_err(|_| "Capture destination offset is unsupported")?;
            if source_offset.saturating_add(row_bytes) > source.len()
                || destination_offset.saturating_add(row_bytes) > destination.mapped_length
            {
                return Err("Capture copy exceeds a mapped buffer");
            }
            // SAFETY: both source and destination row ranges were checked
            // against their live allocations, and separate processes own the
            // source backbuffer and destination SHM object.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    source.as_ptr().add(source_offset),
                    (destination.mapped_address as *mut u8).add(destination_offset),
                    row_bytes,
                );
            }
        }
    }
    Ok(())
}
