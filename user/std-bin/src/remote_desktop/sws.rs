//! Client for the transport-neutral SWS capture and virtual-input protocol.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use scarlet_os::handle::capability::memory_mapping::{MemoryMappingOps, flags as mmap_flags};
use scarlet_os::ipc::{SharedMemory, permissions};
use scarlet_os::socket::{Socket, SocketError};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::vec::Vec;
use sws_remote_protocol::{
    CaptureFormat, ClientMessage, MessageHeader, Rect, ServerMessage, decode_header,
    decode_server_message, encode_client_message,
};

const SWS_REMOTE_SOCKET: &str = "/tmp/sws-remote.sock";
const CONNECT_RETRY_DELAY_MS: u64 = 100;
const IO_POLL_DELAY_MS: u64 = 4;
const MAX_DAMAGE_HISTORY: usize = 64;
const MAX_LOCAL_FRAME_BYTES: usize = 256 * 1024 * 1024;

/// Shared state between the SWS capture thread and all RFB client threads.
pub(crate) struct DesktopState {
    frame: Mutex<FrameState>,
    pending_update_requests: AtomicUsize,
    input: Mutex<Vec<ClientMessage>>,
    stopped: AtomicBool,
}

impl DesktopState {
    /// Construct empty shared desktop state.
    ///
    /// # Returns
    ///
    /// State that has not yet received SWS output dimensions.
    pub(crate) fn new() -> Self {
        Self {
            frame: Mutex::new(FrameState::new()),
            pending_update_requests: AtomicUsize::new(0),
            input: Mutex::new(Vec::new()),
            stopped: AtomicBool::new(false),
        }
    }

    /// Lock the latest captured framebuffer.
    ///
    /// # Returns
    ///
    /// A guard containing dimensions, pixels, sequence, and damage history.
    pub(crate) fn frame(&self) -> MutexGuard<'_, FrameState> {
        self.frame.lock().expect("remote desktop mutex poisoned")
    }

    /// Register one outstanding RFB framebuffer-update request.
    pub(crate) fn add_update_request(&self) {
        self.pending_update_requests.fetch_add(1, Ordering::AcqRel);
    }

    /// Retire one outstanding RFB framebuffer-update request.
    pub(crate) fn remove_update_request(&self) {
        let _ = self.pending_update_requests.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |current| current.checked_sub(1),
        );
    }

    /// Queue virtual input for the SWS connection thread.
    ///
    /// # Arguments
    ///
    /// * `message` - Key or pointer message to inject.
    pub(crate) fn queue_input(&self, message: ClientMessage) {
        self.input
            .lock()
            .expect("remote desktop mutex poisoned")
            .push(message);
    }

    /// Return whether the SWS capture connection has stopped.
    ///
    /// # Returns
    ///
    /// `true` after an unrecoverable capture-loop failure.
    pub(crate) fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    fn has_capture_demand(&self) -> bool {
        self.pending_update_requests.load(Ordering::Acquire) != 0
    }

    fn take_input(&self) -> Vec<ClientMessage> {
        let mut input = self.input.lock().expect("remote desktop mutex poisoned");
        core::mem::take(&mut *input)
    }

    fn resize(&self, width: u32, height: u32) -> Result<(), &'static str> {
        self.frame
            .lock()
            .expect("remote desktop mutex poisoned")
            .resize(width, height)
    }

    fn apply_capture(
        &self,
        buffer: &CaptureBuffer,
        sequence: u64,
        damage: &[Rect],
        previous_sequence: Option<u64>,
    ) -> Result<(), &'static str> {
        self.frame
            .lock()
            .expect("remote desktop mutex poisoned")
            .apply_capture(buffer, sequence, damage, previous_sequence)
    }
}

/// One damage record bridging consecutive on-demand captures.
#[derive(Clone)]
struct DamageRecord {
    base_sequence: Option<u64>,
    sequence: u64,
    damage: Vec<Rect>,
}

/// Latest complete desktop pixels plus bounded damage history.
pub(crate) struct FrameState {
    /// Current output width.
    pub(crate) width: u32,
    /// Current output height.
    pub(crate) height: u32,
    /// Tightly packed BGRA row stride.
    pub(crate) stride: u32,
    /// Latest complete tightly packed BGRA pixels.
    pub(crate) pixels: Vec<u8>,
    /// Latest captured SWS presentation sequence.
    pub(crate) sequence: Option<u64>,
    history: VecDeque<DamageRecord>,
}

impl FrameState {
    pub(crate) fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            stride: 0,
            pixels: Vec::new(),
            sequence: None,
            history: VecDeque::new(),
        }
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), &'static str> {
        let stride = width
            .checked_mul(4)
            .ok_or("Remote desktop stride overflow")?;
        let length = frame_length(stride, height)?;
        self.width = width;
        self.height = height;
        self.stride = stride;
        self.pixels.clear();
        self.pixels
            .try_reserve_exact(length)
            .map_err(|_| "Remote desktop framebuffer allocation failed")?;
        self.pixels.resize(length, 0);
        self.sequence = None;
        self.history.clear();
        Ok(())
    }

    fn apply_capture(
        &mut self,
        buffer: &CaptureBuffer,
        sequence: u64,
        damage: &[Rect],
        previous_sequence: Option<u64>,
    ) -> Result<(), &'static str> {
        if self.width != buffer.width
            || self.height != buffer.height
            || self.stride != buffer.stride
        {
            return Err("SWS capture dimensions changed without OutputChanged");
        }
        for rect in damage {
            let Some(rect) = intersect_rect(*rect, Rect::new(0, 0, self.width, self.height)) else {
                continue;
            };
            let row_bytes = usize::try_from(u64::from(rect.width) * 4)
                .map_err(|_| "Remote desktop damage row is unsupported")?;
            for row in 0..rect.height {
                let offset = usize::try_from(
                    u64::from(rect.y + row) * u64::from(self.stride) + u64::from(rect.x) * 4,
                )
                .map_err(|_| "Remote desktop damage offset is unsupported")?;
                if offset.saturating_add(row_bytes) > self.pixels.len()
                    || offset.saturating_add(row_bytes) > buffer.mapped_length
                {
                    return Err("SWS capture damage exceeds the framebuffer");
                }
                // SAFETY: source and destination row ranges were checked
                // against their live SHM mapping and Vec allocation.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        (buffer.mapped_address as *const u8).add(offset),
                        self.pixels.as_mut_ptr().add(offset),
                        row_bytes,
                    );
                }
            }
        }
        self.sequence = Some(sequence);
        self.history.push_back(DamageRecord {
            base_sequence: previous_sequence,
            sequence,
            damage: damage.to_vec(),
        });
        while self.history.len() > MAX_DAMAGE_HISTORY {
            self.history.pop_front();
        }
        Ok(())
    }

    /// Return damage that transforms a client sequence into the current frame.
    ///
    /// # Arguments
    ///
    /// * `client_sequence` - Sequence last sent to one RFB client.
    ///
    /// # Returns
    ///
    /// Chained damage, an empty list if current, or full-output damage when the
    /// bounded history no longer covers the client.
    pub(crate) fn damage_since(&self, client_sequence: Option<u64>) -> Vec<Rect> {
        let Some(current) = self.sequence else {
            return Vec::new();
        };
        if client_sequence == Some(current) {
            return Vec::new();
        }
        let Some(mut expected_base) = client_sequence else {
            return vec![Rect::new(0, 0, self.width, self.height)];
        };
        let Some(start) = self
            .history
            .iter()
            .position(|record| record.base_sequence == Some(expected_base))
        else {
            return vec![Rect::new(0, 0, self.width, self.height)];
        };

        let mut accumulated = Vec::new();
        for record in self.history.iter().skip(start) {
            if record.base_sequence != Some(expected_base) {
                return vec![Rect::new(0, 0, self.width, self.height)];
            }
            for rect in &record.damage {
                push_damage(&mut accumulated, *rect);
            }
            expected_base = record.sequence;
            if expected_base == current {
                return accumulated;
            }
        }
        vec![Rect::new(0, 0, self.width, self.height)]
    }
}

/// Run the persistent SWS capture connection.
///
/// # Arguments
///
/// * `state` - Desktop state shared with RFB client threads.
pub(crate) fn capture_loop(state: Arc<DesktopState>) {
    if let Err(error) = run_capture(&state) {
        eprintln!("[remote-desktop] SWS capture stopped: {error}");
    }
    state.stopped.store(true, Ordering::Release);
}

fn run_capture(state: &DesktopState) -> Result<(), &'static str> {
    let mut socket = connect_sws()?;
    println!("[remote-desktop] Connected to SWS remote API");
    socket
        .write_all(&encode_client_message(&ClientMessage::CreateCapture {
            output_id: 0,
        }))
        .map_err(|_| "Failed to create SWS capture session")?;
    println!("[remote-desktop] Requested SWS output 0 capture session");
    socket
        .set_nonblocking(true)
        .map_err(|_| "Failed to configure SWS remote socket")?;

    let mut reader = FrameReader::new();
    let mut writer = FrameWriter::new();
    let mut buffer: Option<CaptureBuffer> = None;
    let mut pending_buffer: Option<CaptureBuffer> = None;
    let mut next_buffer_id = 1u32;
    let mut available_sequence: Option<u64> = None;
    let mut captured_sequence: Option<u64> = None;
    let mut request_in_flight = false;

    loop {
        if pending_buffer.is_some() && !writer.has_pending() {
            let pending = pending_buffer.take().expect("pending capture buffer");
            let registration = encode_client_message(&ClientMessage::RegisterBuffer {
                buffer_id: pending.buffer_id,
                width: pending.width,
                height: pending.height,
                stride: pending.stride,
                format: CaptureFormat::Bgra8888,
            });
            send_handle_record(&socket, pending.shared_memory.as_handle(), &registration)?;
            buffer = Some(pending);
        }

        if pending_buffer.is_none() {
            for message in state.take_input() {
                writer.enqueue(encode_client_message(&message))?;
            }
        }

        if !request_in_flight
            && state.has_capture_demand()
            && available_sequence.is_some()
            && available_sequence != captured_sequence
            && let Some(capture_buffer) = buffer.as_ref()
        {
            writer.enqueue(encode_client_message(&ClientMessage::RequestFrame {
                buffer_id: capture_buffer.buffer_id,
            }))?;
            request_in_flight = true;
        }
        writer.flush(&mut socket)?;

        let mut progressed = false;
        while let Some((header, payload)) = reader.poll(&mut socket)? {
            progressed = true;
            match decode_server_message(header, &payload)
                .map_err(|_| "Invalid SWS remote response")?
            {
                ServerMessage::OutputChanged { width, height } => {
                    println!(
                        "[remote-desktop] SWS output changed: {}x{}",
                        width, height
                    );
                    state.resize(width, height)?;
                    let new_buffer = CaptureBuffer::new(next_buffer_id, width, height)?;
                    next_buffer_id = next_buffer_id.wrapping_add(1).max(1);
                    pending_buffer = Some(new_buffer);
                    buffer = None;
                    available_sequence = None;
                    captured_sequence = None;
                    request_in_flight = false;
                }
                ServerMessage::FrameAvailable { sequence } => {
                    available_sequence = Some(sequence);
                }
                ServerMessage::FrameReady {
                    buffer_id,
                    sequence,
                    damage,
                } => {
                    let active = buffer
                        .as_ref()
                        .filter(|active| active.buffer_id == buffer_id)
                        .ok_or("SWS completed an unknown capture buffer")?;
                    state.apply_capture(active, sequence, &damage, captured_sequence)?;
                    captured_sequence = Some(sequence);
                    request_in_flight = false;
                }
            }
        }

        if !progressed {
            thread::sleep(core::time::Duration::from_millis(IO_POLL_DELAY_MS));
        }
    }
}

fn connect_sws() -> Result<Socket, &'static str> {
    loop {
        let socket = Socket::new().map_err(|_| "Failed to create SWS remote socket")?;
        match socket.connect(SWS_REMOTE_SOCKET) {
            Ok(()) => return Ok(socket),
            Err(_) => thread::sleep(core::time::Duration::from_millis(CONNECT_RETRY_DELAY_MS)),
        }
    }
}

struct CaptureBuffer {
    buffer_id: u32,
    width: u32,
    height: u32,
    stride: u32,
    shared_memory: SharedMemory,
    mapped_address: usize,
    mapped_length: usize,
}

impl CaptureBuffer {
    fn new(buffer_id: u32, width: u32, height: u32) -> Result<Self, &'static str> {
        if width == 0 || height == 0 {
            return Err("SWS reported an empty output");
        }
        let stride = width
            .checked_mul(4)
            .ok_or("Capture buffer stride overflow")?;
        let mapped_length = frame_length(stride, height)?;
        let shared_memory = SharedMemory::create(mapped_length, permissions::READ_WRITE)
            .map_err(|_| "Failed to create capture shared memory")?;
        let mapped_address = shared_memory
            .as_handle()
            .as_memory_mapping()
            .map_err(|_| "Capture shared memory cannot be mapped")?
            .mmap(
                0,
                mapped_length,
                permissions::READ_WRITE,
                mmap_flags::SHARED,
                0,
            )
            .map_err(|_| "Failed to map capture shared memory")?;
        Ok(Self {
            buffer_id,
            width,
            height,
            stride,
            shared_memory,
            mapped_address,
            mapped_length,
        })
    }
}

impl Drop for CaptureBuffer {
    fn drop(&mut self) {
        let _ = MemoryMappingOps::munmap(self.mapped_address, self.mapped_length);
    }
}

fn frame_length(stride: u32, height: u32) -> Result<usize, &'static str> {
    let length = usize::try_from(
        u64::from(stride)
            .checked_mul(u64::from(height))
            .ok_or("Remote desktop framebuffer size overflow")?,
    )
    .map_err(|_| "Remote desktop framebuffer size is unsupported")?;
    if length == 0 || length > MAX_LOCAL_FRAME_BYTES {
        return Err("Remote desktop framebuffer size is unsupported");
    }
    Ok(length)
}

fn send_handle_record(
    socket: &Socket,
    handle: &scarlet_os::handle::Handle,
    frame: &[u8],
) -> Result<(), &'static str> {
    loop {
        match socket.send_handle_and_data(handle, frame) {
            Ok(()) => return Ok(()),
            Err(SocketError::WouldBlock) => {
                thread::sleep(core::time::Duration::from_millis(IO_POLL_DELAY_MS));
            }
            Err(_) => return Err("Failed to register SWS capture buffer"),
        }
    }
}

struct FrameReader {
    header: [u8; MessageHeader::SIZE],
    header_filled: usize,
    parsed_header: Option<MessageHeader>,
    payload: Vec<u8>,
    payload_filled: usize,
}

impl FrameReader {
    fn new() -> Self {
        Self {
            header: [0; MessageHeader::SIZE],
            header_filled: 0,
            parsed_header: None,
            payload: Vec::new(),
            payload_filled: 0,
        }
    }

    fn reset(&mut self) {
        self.header_filled = 0;
        self.parsed_header = None;
        self.payload.clear();
        self.payload_filled = 0;
    }

    fn poll(
        &mut self,
        socket: &mut Socket,
    ) -> Result<Option<(MessageHeader, Vec<u8>)>, &'static str> {
        while self.header_filled < MessageHeader::SIZE {
            match socket.read(&mut self.header[self.header_filled..]) {
                Ok(0) => return Err("SWS remote connection closed"),
                Ok(count) => self.header_filled += count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
                Err(_) => return Err("Failed to read SWS remote header"),
            }
        }
        if self.parsed_header.is_none() {
            let header = decode_header(&self.header).map_err(|_| "Invalid SWS remote header")?;
            self.payload.resize(header.payload_size as usize, 0);
            self.parsed_header = Some(header);
        }
        while self.payload_filled < self.payload.len() {
            match socket.read(&mut self.payload[self.payload_filled..]) {
                Ok(0) => return Err("SWS remote connection closed"),
                Ok(count) => self.payload_filled += count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
                Err(_) => return Err("Failed to read SWS remote payload"),
            }
        }
        let header = self.parsed_header.ok_or("Missing SWS remote header")?;
        let payload = core::mem::take(&mut self.payload);
        self.reset();
        Ok(Some((header, payload)))
    }
}

struct PendingFrame {
    bytes: Vec<u8>,
    offset: usize,
}

struct FrameWriter {
    frames: VecDeque<PendingFrame>,
    pending_bytes: usize,
}

impl FrameWriter {
    fn new() -> Self {
        Self {
            frames: VecDeque::new(),
            pending_bytes: 0,
        }
    }

    fn has_pending(&self) -> bool {
        !self.frames.is_empty()
    }

    fn enqueue(&mut self, bytes: Vec<u8>) -> Result<(), &'static str> {
        if self.pending_bytes.saturating_add(bytes.len()) > sws_remote_protocol::MAX_PAYLOAD_SIZE {
            return Err("SWS remote output queue exceeded its limit");
        }
        self.pending_bytes += bytes.len();
        self.frames.push_back(PendingFrame { bytes, offset: 0 });
        Ok(())
    }

    fn flush(&mut self, socket: &mut Socket) -> Result<(), &'static str> {
        while let Some(frame) = self.frames.front_mut() {
            match socket.write(&frame.bytes[frame.offset..]) {
                Ok(0) => return Err("SWS remote connection closed while writing"),
                Ok(count) => {
                    frame.offset += count;
                    self.pending_bytes = self.pending_bytes.saturating_sub(count);
                    if frame.offset == frame.bytes.len() {
                        self.frames.pop_front();
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => return Err("Failed to write SWS remote message"),
            }
        }
        Ok(())
    }
}

fn intersect_rect(a: Rect, b: Rect) -> Option<Rect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = a.x.saturating_add(a.width).min(b.x.saturating_add(b.width));
    let y1 =
        a.y.saturating_add(a.height)
            .min(b.y.saturating_add(b.height));
    (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
}

fn push_damage(rects: &mut Vec<Rect>, rect: Rect) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    for existing in rects.iter_mut() {
        if should_merge(*existing, rect) {
            *existing = union_rect(*existing, rect);
            return;
        }
    }
    if rects.len() < sws_remote_protocol::MAX_DAMAGE_RECTS {
        rects.push(rect);
    } else if let Some(first) = rects.first_mut() {
        *first = union_rect(*first, rect);
    }
}

fn should_merge(a: Rect, b: Rect) -> bool {
    let union = union_rect(a, b);
    area(union) <= area(a).saturating_add(area(b)).saturating_mul(2)
}

fn area(rect: Rect) -> u64 {
    u64::from(rect.width).saturating_mul(u64::from(rect.height))
}

fn union_rect(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.min(b.x);
    let y0 = a.y.min(b.y);
    let x1 = a.x.saturating_add(a.width).max(b.x.saturating_add(b.width));
    let y1 =
        a.y.saturating_add(a.height)
            .max(b.y.saturating_add(b.height));
    Rect::new(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
}
