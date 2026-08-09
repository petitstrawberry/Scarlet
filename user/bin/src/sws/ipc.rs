//! IPC Server module - handles client connections and messages

use super::config;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::collections::BTreeMap;
use std::env;
use std::handle::Handle;
use std::handle::capability::memory_mapping::flags as mmap_flags;
use std::ipc::{SharedMemory, permissions};
use std::poll::{POLLERR, POLLHUP, POLLIN, POLLNVAL, POLLOUT, PollHandle, poll};
use std::println;
use std::socket::Socket;
use std::string::String;
use std::sync::Mutex;
use std::thread::{self, yield_now};
use std::vec::Vec;
use sws_protocol as protocol;
use sws_protocol::ClientMessageRef;

fn is_sws_debug_enabled() -> bool {
    static LOG_CACHE: AtomicU8 = AtomicU8::new(u8::MAX);
    let cached = LOG_CACHE.load(Ordering::Relaxed);
    if cached != u8::MAX {
        return cached != 0;
    }
    let enabled = match env::var("SWS_LOG") {
        Some(val) => matches!(
            val.as_str(),
            "debug" | "DEBUG" | "3" | "trace" | "TRACE" | "4"
        ),
        None => false,
    };
    LOG_CACHE.store(enabled as u8, Ordering::Relaxed);
    enabled
}

fn merge_damage(
    ax: i32,
    ay: i32,
    aw: u32,
    ah: u32,
    bx: i32,
    by: i32,
    bw: u32,
    bh: u32,
) -> (i32, i32, u32, u32) {
    let x0 = i64::from(ax.min(bx));
    let y0 = i64::from(ay.min(by));
    let ax1 = (ax as i64).saturating_add(aw as i64);
    let ay1 = (ay as i64).saturating_add(ah as i64);
    let bx1 = (bx as i64).saturating_add(bw as i64);
    let by1 = (by as i64).saturating_add(bh as i64);
    let x1 = ax1.max(bx1);
    let y1 = ay1.max(by1);
    let w = (x1 - x0).max(0) as u32;
    let h = (y1 - y0).max(0) as u32;
    (x0 as i32, y0 as i32, w, h)
}

fn damage_area(width: u32, height: u32) -> u64 {
    u64::from(width).saturating_mul(u64::from(height))
}

fn should_merge_damage(
    ax: i32,
    ay: i32,
    aw: u32,
    ah: u32,
    bx: i32,
    by: i32,
    bw: u32,
    bh: u32,
) -> bool {
    let (_, _, union_w, union_h) = merge_damage(ax, ay, aw, ah, bx, by, bw, bh);
    let separate_area = damage_area(aw, ah).saturating_add(damage_area(bw, bh));
    let union_area = damage_area(union_w, union_h);
    union_area <= separate_area.saturating_mul(2)
}

/// Application session information
#[derive(Debug, Clone)]
struct AppSession {
    window_id: u32,
    app_id: String,
    app_name: String,
    menu_titles: String, // Format: "menu1|menu2|menu3"
}

/// Active application sessions (window_id -> AppSession)
static APP_SESSIONS: Mutex<BTreeMap<u32, AppSession>> = Mutex::new(BTreeMap::new());

/// Currently focused window ID
static FOCUSED_WINDOW_ID: Mutex<Option<u32>> = Mutex::new(None);

#[derive(Debug, Clone)]
struct TextInputState {
    cursor_x: i32,
    cursor_y: i32,
    cursor_width: u32,
    cursor_height: u32,
    content_hint: u32,
    content_purpose: u32,
    text_change_cause: u32,
    cursor_byte: u32,
    anchor_byte: u32,
    surrounding_text: Vec<u8>,
}

impl TextInputState {
    fn new() -> Self {
        Self {
            cursor_x: 0,
            cursor_y: 0,
            cursor_width: 0,
            cursor_height: 0,
            content_hint: sws_protocol::text_input_content_hints::NONE,
            content_purpose: sws_protocol::text_input_content_purpose::NORMAL,
            text_change_cause: sws_protocol::text_input_change_cause::OTHER,
            cursor_byte: 0,
            anchor_byte: 0,
            surrounding_text: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct TextInputContext {
    client_id: usize,
    context_id: u32,
    window_id: u32,
    seat_id: u32,
    enabled: bool,
    keyboard_grabbed: bool,
    serial: u32,
    pending: TextInputState,
    current: TextInputState,
}

#[derive(Debug, Clone)]
struct InputMethodService {
    client_id: usize,
    ime_id: u32,
    name: String,
    capabilities: u32,
}

#[derive(Debug, Clone)]
struct PendingImeKey {
    context_id: u32,
    window_id: u32,
    time: u64,
    type_: u16,
    code: u16,
    value: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct TextInputCursorRect {
    pub window_id: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

static NEXT_TEXT_INPUT_CONTEXT_ID: AtomicU32 = AtomicU32::new(1);
static NEXT_IME_ID: AtomicU32 = AtomicU32::new(1);
static NEXT_IME_KEY_SERIAL: AtomicU32 = AtomicU32::new(1);
static TEXT_INPUT_CONTEXTS: Mutex<BTreeMap<u32, TextInputContext>> = Mutex::new(BTreeMap::new());
static INPUT_METHODS: Mutex<BTreeMap<u32, InputMethodService>> = Mutex::new(BTreeMap::new());
static ACTIVE_IME_ID: Mutex<Option<u32>> = Mutex::new(None);
static PREFERRED_IME_NAME: Mutex<Option<String>> = Mutex::new(None);
static ACTIVE_TEXT_INPUT_CONTEXT: Mutex<Option<u32>> = Mutex::new(None);
static PENDING_IME_KEYS: Mutex<BTreeMap<u32, PendingImeKey>> = Mutex::new(BTreeMap::new());

#[derive(Debug)]
enum FrameIoError {
    WouldBlock,
    Disconnected,
    Backpressure,
    Io,
    Protocol,
}

const MAX_CLIENT_OUTBOUND_BYTES: usize = 4 * 1024 * 1024;
const MAX_CLIENT_WRITE_BYTES_PER_TICK: usize = 64 * 1024;
const MAX_CLIENT_WRITE_CHUNK: usize = 16 * 1024;
const CLIENT_POLL_TIMEOUT_NS: i64 = 8_000_000;

struct PendingStreamFrame {
    bytes: Vec<u8>,
    offset: usize,
}

struct ClientStreamWriter {
    frames: Vec<PendingStreamFrame>,
    head: usize,
    pending_bytes: usize,
}

impl ClientStreamWriter {
    fn new() -> Self {
        Self {
            frames: Vec::new(),
            head: 0,
            pending_bytes: 0,
        }
    }

    fn has_pending(&self) -> bool {
        self.head < self.frames.len()
    }

    fn enqueue(
        &mut self,
        msg_type: u32,
        flags: u8,
        request_id: u8,
        payload: &[u8],
    ) -> Result<(), FrameIoError> {
        if payload.len() > protocol::MAX_PAYLOAD_SIZE {
            return Err(FrameIoError::Protocol);
        }
        let bytes = protocol::encode_routed_frame(msg_type, flags, request_id, payload);
        if self.pending_bytes.saturating_add(bytes.len()) > MAX_CLIENT_OUTBOUND_BYTES {
            return Err(FrameIoError::Backpressure);
        }
        self.pending_bytes += bytes.len();
        self.frames.push(PendingStreamFrame { bytes, offset: 0 });
        Ok(())
    }

    fn flush(&mut self, socket: &mut Socket) -> Result<bool, FrameIoError> {
        use std::io::Write;

        let mut written_this_tick = 0usize;
        while written_this_tick < MAX_CLIENT_WRITE_BYTES_PER_TICK {
            let Some(frame) = self.frames.get_mut(self.head) else {
                break;
            };
            let remaining_budget = MAX_CLIENT_WRITE_BYTES_PER_TICK - written_this_tick;
            let chunk_len = remaining_budget
                .min(MAX_CLIENT_WRITE_CHUNK)
                .min(frame.bytes.len().saturating_sub(frame.offset));
            if chunk_len == 0 {
                self.head += 1;
                continue;
            }
            let chunk_end = frame.offset + chunk_len;
            match socket.write(&frame.bytes[frame.offset..chunk_end]) {
                Ok(0) => return Err(FrameIoError::Disconnected),
                Ok(written) => {
                    frame.offset += written;
                    written_this_tick += written;
                    self.pending_bytes = self.pending_bytes.saturating_sub(written);
                    if frame.offset == frame.bytes.len() {
                        self.head += 1;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => return Err(FrameIoError::Io),
            }
        }
        if self.head > 0 && self.head.saturating_mul(2) >= self.frames.len() {
            self.frames.drain(..self.head);
            self.head = 0;
        }
        Ok(written_this_tick != 0)
    }
}

/// Non-blocking framed-message reader.
///
/// With non-blocking sockets, reads can return `WouldBlock` after consuming
/// *some* bytes. If we drop that partial progress and restart from a fresh
/// header read, the stream becomes desynchronized and subsequent frames are
/// mis-parsed (e.g., intermittent 0x0 damage rectangles).
struct FrameReader {
    header: [u8; protocol::MessageHeader::SIZE],
    header_filled: usize,
    header_parsed: bool,

    header_value: protocol::MessageHeader,
    payload_len: usize,
    payload: Vec<u8>,
    payload_filled: usize,
}

impl FrameReader {
    fn new() -> Self {
        Self {
            header: [0u8; protocol::MessageHeader::SIZE],
            header_filled: 0,
            header_parsed: false,
            header_value: protocol::MessageHeader::new(0, 0),
            payload_len: 0,
            payload: Vec::new(),
            payload_filled: 0,
        }
    }

    fn reset(&mut self) {
        self.header_filled = 0;
        self.header_parsed = false;
        self.header_value = protocol::MessageHeader::new(0, 0);
        self.payload_len = 0;
        self.payload.clear();
        self.payload_filled = 0;
    }

    /// Poll for the next complete frame.
    ///
    /// - `Ok(Some((header, payload)))` when a full frame is assembled
    /// - `Ok(None)` when no complete frame is available yet
    /// - `Err(..)` on disconnect / I/O / protocol error
    fn poll(
        &mut self,
        socket: &mut Socket,
    ) -> Result<Option<(protocol::MessageHeader, Vec<u8>)>, FrameIoError> {
        use std::io::Read;

        // Read header.
        while self.header_filled < self.header.len() {
            match socket.read(&mut self.header[self.header_filled..]) {
                Ok(0) => return Err(FrameIoError::Disconnected),
                Ok(n) => self.header_filled += n,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        return Ok(None);
                    }
                    return Err(FrameIoError::Io);
                }
            }
        }

        // Parse header once.
        if !self.header_parsed {
            let header = protocol::MessageHeader::from_le_bytes(self.header);
            let payload_len = header.payload_size as usize;
            if payload_len > protocol::MAX_PAYLOAD_SIZE {
                self.reset();
                return Err(FrameIoError::Protocol);
            }
            self.header_value = header;
            self.payload_len = payload_len;
            if payload_len > 0 {
                self.payload.resize(payload_len, 0);
            }
            self.header_parsed = true;
        }

        // Read payload.
        while self.payload_filled < self.payload_len {
            match socket.read(&mut self.payload[self.payload_filled..]) {
                Ok(0) => return Err(FrameIoError::Disconnected),
                Ok(n) => self.payload_filled += n,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        return Ok(None);
                    }
                    return Err(FrameIoError::Io);
                }
            }
        }

        // Complete.
        let header = self.header_value;
        let payload = core::mem::take(&mut self.payload);
        self.reset();
        Ok(Some((header, payload)))
    }
}

fn decode_atomic_handle_frame(
    bytes: &[u8],
) -> Result<(protocol::MessageHeader, Vec<u8>), FrameIoError> {
    if bytes.len() < protocol::MessageHeader::SIZE {
        return Err(FrameIoError::Protocol);
    }
    let mut encoded_header = [0u8; protocol::MessageHeader::SIZE];
    encoded_header.copy_from_slice(&bytes[..protocol::MessageHeader::SIZE]);
    let header = protocol::MessageHeader::from_le_bytes(encoded_header);
    let payload_len = header.payload_size as usize;
    if payload_len > protocol::MAX_PAYLOAD_SIZE
        || bytes.len() != protocol::MessageHeader::SIZE + payload_len
    {
        return Err(FrameIoError::Protocol);
    }
    Ok((header, bytes[protocol::MessageHeader::SIZE..].to_vec()))
}

fn poll_atomic_handle_frame(
    socket: &Socket,
    frame: &mut Vec<u8>,
) -> Result<Option<(protocol::MessageHeader, Vec<u8>, Handle)>, FrameIoError> {
    let mut probe = [];
    let required_len = match socket.recv_handle_and_data(&mut probe) {
        Err(std::socket::SocketError::ReceiveBufferTooSmall { required_len }) => required_len,
        Err(std::socket::SocketError::WouldBlock) => return Ok(None),
        Ok(_) => return Err(FrameIoError::Protocol),
        Err(_) => return Err(FrameIoError::Io),
    };

    let maximum_len = protocol::MessageHeader::SIZE + protocol::MAX_PAYLOAD_SIZE;
    if !(protocol::MessageHeader::SIZE..=maximum_len).contains(&required_len) {
        return Err(FrameIoError::Protocol);
    }
    frame.clear();
    frame
        .try_reserve_exact(required_len)
        .map_err(|_| FrameIoError::Io)?;
    frame.resize(required_len, 0);

    let (handle, length) = socket
        .recv_handle_and_data(frame)
        .map_err(|_| FrameIoError::Io)?;
    if length != required_len {
        return Err(FrameIoError::Protocol);
    }
    let (header, payload) = decode_atomic_handle_frame(frame)?;
    Ok(Some((header, payload, handle)))
}

fn write_frame(
    writer: &mut ClientStreamWriter,
    msg_type: u32,
    payload: &[u8],
) -> Result<(), FrameIoError> {
    write_frame_routed(writer, msg_type, 0, 0, payload)
}

fn write_frame_response(
    writer: &mut ClientStreamWriter,
    msg_type: u32,
    request_id: u8,
    payload: &[u8],
) -> Result<(), FrameIoError> {
    write_frame_routed(
        writer,
        msg_type,
        protocol::MessageHeader::FLAG_IS_RESPONSE,
        request_id,
        payload,
    )
}

fn write_frame_routed(
    writer: &mut ClientStreamWriter,
    msg_type: u32,
    flags: u8,
    request_id: u8,
    payload: &[u8],
) -> Result<(), FrameIoError> {
    writer.enqueue(msg_type, flags, request_id, payload)
}

fn write_protocol_error(
    writer: &mut ClientStreamWriter,
    request_id: u8,
    code: u32,
) -> Result<(), FrameIoError> {
    let payload = protocol::payload_error(code);
    if request_id == 0 {
        write_frame(writer, protocol::server_msg::ERROR, &payload)
    } else {
        write_frame_response(writer, protocol::server_msg::ERROR, request_id, &payload)
    }
}

fn write_sgfx_frame_rejected(
    writer: &mut ClientStreamWriter,
    window_id: u32,
    buffer_id: u32,
    generation: u32,
    compositor_epoch: u32,
    commit_serial: u64,
    code: u32,
) -> Result<(), FrameIoError> {
    let payload = protocol::payload_sgfx_frame_rejected(
        window_id,
        buffer_id,
        generation,
        compositor_epoch,
        commit_serial,
        code,
    );
    write_frame(writer, protocol::server_msg::SGFX_FRAME_REJECTED, &payload)
}

fn write_handle_frame_response(
    socket: &Socket,
    handle: &Handle,
    msg_type: u32,
    request_id: u8,
    payload: &[u8],
) -> Result<(), FrameIoError> {
    let frame = protocol::encode_routed_frame(
        msg_type,
        protocol::MessageHeader::FLAG_IS_RESPONSE,
        request_id,
        payload,
    );
    socket
        .send_handle_and_data(handle, &frame)
        .map_err(|_| FrameIoError::Io)
}

/// Input event to be sent to a client
#[derive(Debug, Clone)]
pub struct PendingInputEvent {
    pub time: u64,
    pub type_: u16,
    pub code: u16,
    pub value: i32,
}

fn is_pointer_motion_packet(events: &[PendingInputEvent], start: usize) -> bool {
    events.len() >= start + 3
        && events[start].type_ == super::input::event_types::EV_ABS
        && events[start].code == super::input::abs_codes::ABS_X
        && events[start + 1].type_ == super::input::event_types::EV_ABS
        && events[start + 1].code == super::input::abs_codes::ABS_Y
        && events[start + 2].type_ == super::input::event_types::EV_SYN
}

fn coalesce_tail_pointer_motion(events: &mut Vec<PendingInputEvent>) {
    loop {
        let len = events.len();
        if len < 6 {
            return;
        }

        let previous_start = len - 6;
        let latest_start = len - 3;
        if !is_pointer_motion_packet(events, previous_start)
            || !is_pointer_motion_packet(events, latest_start)
        {
            return;
        }

        let latest_x = events[latest_start].clone();
        let latest_y = events[latest_start + 1].clone();
        let latest_syn = events[latest_start + 2].clone();
        events.truncate(previous_start);
        events.push(latest_x);
        events.push(latest_y);
        events.push(latest_syn);
    }
}

fn push_input_event_coalesced(events: &mut Vec<PendingInputEvent>, event: PendingInputEvent) {
    let should_coalesce = event.type_ == super::input::event_types::EV_SYN;
    events.push(event);
    if should_coalesce {
        coalesce_tail_pointer_motion(events);
    }
}

/// Wake pipe used by worker threads to interrupt the compositor event loop.
static COMPOSITOR_WAKE_WRITE: Mutex<Option<Handle>> = Mutex::new(None);
static COMPOSITOR_WAKE_PENDING: AtomicBool = AtomicBool::new(false);

struct ClientWake {
    write_handle: Handle,
    pending: bool,
}

static CLIENT_WAKES: Mutex<BTreeMap<usize, ClientWake>> = Mutex::new(BTreeMap::new());
static WINDOW_OWNERS: Mutex<BTreeMap<u32, usize>> = Mutex::new(BTreeMap::new());

/// Whether the selected SWS compositor backend currently accepts shared SGFX images.
static SGFX_SHARED_IMAGES_AVAILABLE: AtomicBool = AtomicBool::new(false);
static COMPOSITOR_EPOCH: AtomicU32 = AtomicU32::new(1);

/// Publish shared-SGFX availability before accepting client connections.
///
/// # Arguments
///
/// * `available` - Whether the active compositor can import shared SGFX images.
///
/// # Returns
///
/// This function returns no value.
pub fn set_sgfx_shared_images_available(available: bool) {
    SGFX_SHARED_IMAGES_AVAILABLE.store(available, Ordering::Release);
}

fn sws_capabilities() -> u64 {
    if SGFX_SHARED_IMAGES_AVAILABLE.load(Ordering::Acquire) {
        protocol::capabilities::SGFX_SHARED_IMAGE
    } else {
        0
    }
}

fn compositor_epoch() -> u32 {
    COMPOSITOR_EPOCH.load(Ordering::Acquire)
}

fn compositor_backend_id() -> u32 {
    if SGFX_SHARED_IMAGES_AVAILABLE.load(Ordering::Acquire) {
        protocol::compositor_backends::SGFX
    } else {
        protocol::compositor_backends::CPU
    }
}

/// Disable shared SGFX registration and notify all connected clients.
///
/// # Returns
///
/// This function returns no value. Repeated calls after the backend has already
/// been disabled do not advance the compositor epoch again.
pub fn notify_sgfx_backend_lost() {
    if !SGFX_SHARED_IMAGES_AVAILABLE.swap(false, Ordering::AcqRel) {
        return;
    }
    let mut next_epoch = COMPOSITOR_EPOCH
        .fetch_add(1, Ordering::AcqRel)
        .wrapping_add(1);
    if next_epoch == 0 {
        COMPOSITOR_EPOCH.store(1, Ordering::Release);
        next_epoch = 1;
    }
    let payload = protocol::payload_sgfx_backend_lost(next_epoch).to_vec();
    broadcast_message_to_all_clients(protocol::server_msg::SGFX_BACKEND_LOST, payload);
}

/// Install the compositor wake pipe write handle.
///
/// # Arguments
///
/// * `write_handle` - Write side of the compositor wake pipe.
pub fn set_compositor_wake_handle(write_handle: Handle) {
    let mut wake = COMPOSITOR_WAKE_WRITE.lock();
    *wake = Some(write_handle);
}

/// Wake the compositor if it is sleeping on the wake pipe.
pub fn wake_compositor() {
    super::trace::wake_call();
    if COMPOSITOR_WAKE_PENDING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        super::trace::wake_coalesced();
        return;
    }

    let wake = COMPOSITOR_WAKE_WRITE.lock();
    let Some(handle) = wake.as_ref() else {
        COMPOSITOR_WAKE_PENDING.store(false, Ordering::Release);
        return;
    };

    let Ok(stream) = handle.as_stream() else {
        COMPOSITOR_WAKE_PENDING.store(false, Ordering::Release);
        return;
    };

    if stream.write(&[1]).is_err() {
        COMPOSITOR_WAKE_PENDING.store(false, Ordering::Release);
    }
}

/// Mark the currently pending compositor wake as consumed.
pub fn consume_compositor_wake() {
    COMPOSITOR_WAKE_PENDING.store(false, Ordering::Release);
}

fn wake_client(client_id: usize) {
    let mut wakes = CLIENT_WAKES.lock();
    let Some(wake) = wakes.get_mut(&client_id) else {
        return;
    };

    // Coalesce wakeups until the client thread consumes the byte. Keeping this
    // state under the same lock as the write handle establishes the invariant
    // that `pending == false` means the pipe has no unread wake byte, so this
    // blocking pipe write can never wait for buffer space.
    if wake.pending {
        return;
    }
    wake.pending = true;

    let wrote_wake = match wake.write_handle.as_stream() {
        Ok(stream) => matches!(stream.write(&[1]), Ok(1)),
        Err(_) => false,
    };
    if !wrote_wake {
        wake.pending = false;
    }
}

fn consume_client_wake(client_id: usize) {
    // Producers enqueue their work before calling `wake_client`. If a producer
    // observes `pending == true` while this byte is being consumed, its wake
    // may be coalesced safely: the client loop immediately rescans every queue
    // after this read. Clear only after a successful read so `pending == false`
    // continues to imply that the pipe contains no unread wake byte.
    let mut wakes = CLIENT_WAKES.lock();
    if let Some(wake) = wakes.get_mut(&client_id) {
        wake.pending = false;
    }
}

fn wake_window_owner(window_id: u32) {
    let client_id = WINDOW_OWNERS.lock().get(&window_id).copied();
    if let Some(client_id) = client_id {
        wake_client(client_id);
    }
}

/// Global event queue for IPC events
static EVENT_QUEUE: Mutex<Vec<IpcEvent>> = Mutex::new(Vec::new());

/// Global pending input events: BTreeMap for O(log n) lookup
static PENDING_INPUT_EVENTS: Mutex<BTreeMap<u32, Vec<PendingInputEvent>>> =
    Mutex::new(BTreeMap::new());

/// Pending server->client frames to be delivered to a specific window.
#[derive(Debug, Clone)]
pub struct PendingServerFrame {
    pub msg_type: u32,
    pub flags: u8,
    pub request_id: u8,
    pub payload: Vec<u8>,
}

static PENDING_SERVER_FRAMES: Mutex<BTreeMap<u32, Vec<PendingServerFrame>>> =
    Mutex::new(BTreeMap::new());

/// Pending server->client responses for specific clients (by client_id)
/// This is used for responses to clients that don't have windows (like stemd)
static PENDING_CLIENT_RESPONSES: Mutex<BTreeMap<usize, Vec<PendingServerFrame>>> =
    Mutex::new(BTreeMap::new());

/// Add an event to the global queue
pub fn push_ipc_event(event: IpcEvent) {
    let mut queue = EVENT_QUEUE.lock();
    if let IpcEvent::ExtensionUpdateBuffer {
        external_client_id,
        window_id,
        damage_x,
        damage_y,
        damage_width,
        damage_height,
    } = event
    {
        for existing in queue.iter_mut().rev() {
            if let IpcEvent::ExtensionUpdateBuffer {
                window_id: existing_window,
                damage_x: existing_x,
                damage_y: existing_y,
                damage_width: existing_w,
                damage_height: existing_h,
                ..
            } = existing
            {
                if *existing_window == window_id {
                    if !should_merge_damage(
                        *existing_x,
                        *existing_y,
                        *existing_w,
                        *existing_h,
                        damage_x,
                        damage_y,
                        damage_width,
                        damage_height,
                    ) {
                        continue;
                    }
                    let (nx, ny, nw, nh) = merge_damage(
                        *existing_x,
                        *existing_y,
                        *existing_w,
                        *existing_h,
                        damage_x,
                        damage_y,
                        damage_width,
                        damage_height,
                    );
                    *existing_x = nx;
                    *existing_y = ny;
                    *existing_w = nw;
                    *existing_h = nh;
                    return;
                }
            }
        }
        let should_wake = queue.is_empty();
        queue.push(IpcEvent::ExtensionUpdateBuffer {
            external_client_id,
            window_id,
            damage_x,
            damage_y,
            damage_width,
            damage_height,
        });
        drop(queue);
        if should_wake {
            wake_compositor();
        }
        return;
    }
    let should_wake = queue.is_empty();
    queue.push(event);
    drop(queue);
    if should_wake {
        wake_compositor();
    }
}

/// Get all pending events from the queue
pub fn pop_all_ipc_events() -> Vec<IpcEvent> {
    let mut queue = EVENT_QUEUE.lock();
    core::mem::take(&mut *queue)
}

/// Return whether the compositor IPC queue has pending events.
///
/// # Returns
///
/// `true` if IPC events are queued for the compositor.
pub fn has_pending_ipc_events() -> bool {
    !EVENT_QUEUE.lock().is_empty()
}

/// Register a window for input event routing
fn register_window(window_id: u32, client_id: usize) {
    WINDOW_OWNERS.lock().insert(window_id, client_id);

    {
        let mut pending = PENDING_INPUT_EVENTS.lock();
        pending.entry(window_id).or_insert_with(Vec::new);
    }

    {
        let mut pending = PENDING_SERVER_FRAMES.lock();
        pending.entry(window_id).or_insert_with(Vec::new);
    }
}

/// Unregister a window
fn unregister_window(window_id: u32) {
    WINDOW_OWNERS.lock().remove(&window_id);

    {
        let mut pending = PENDING_INPUT_EVENTS.lock();
        pending.remove(&window_id);
    }

    {
        let mut pending = PENDING_SERVER_FRAMES.lock();
        pending.remove(&window_id);
    }
}

fn cleanup_window_state(window_id: u32) {
    unregister_window(window_id);

    let mut sessions = APP_SESSIONS.lock();
    sessions.remove(&window_id);
}

/// Queue an input event for a specific window (O(log n) lookup)
pub fn send_input_to_window(window_id: u32, time: u64, type_: u16, code: u16, value: i32) {
    let mut pending = PENDING_INPUT_EVENTS.lock();
    let events = pending.entry(window_id).or_insert_with(Vec::new);
    let should_wake = events.is_empty();
    push_input_event_coalesced(
        events,
        PendingInputEvent {
            time,
            type_,
            code,
            value,
        },
    );
    drop(pending);
    if should_wake {
        wake_window_owner(window_id);
    }
}

/// Pending extension input events: BTreeMap from (extension_id, external_client_id) to events
static PENDING_EXTENSION_INPUT_EVENTS: Mutex<BTreeMap<(u32, u32), Vec<PendingInputEvent>>> =
    Mutex::new(BTreeMap::new());

/// Queue an input event for an extension-owned window (O(log n) lookup)
/// This sends EXTENSION_INPUT_EVENT to the extension client instead of regular INPUT_EVENT
pub fn send_extension_input_event(
    extension_id: u32,
    external_client_id: u32,
    window_id: u32,
    time: u64,
    type_: u16,
    code: u16,
    value: i32,
) {
    let mut pending = PENDING_EXTENSION_INPUT_EVENTS.lock();

    let events = pending
        .entry((extension_id, external_client_id))
        .or_insert_with(Vec::new);
    push_input_event_coalesced(
        events,
        PendingInputEvent {
            time,
            type_,
            code,
            value,
        },
    );
}

/// Get and clear pending extension input events for a specific extension client
pub fn pop_extension_input_events(
    extension_id: u32,
    external_client_id: u32,
) -> Vec<PendingInputEvent> {
    let mut pending = PENDING_EXTENSION_INPUT_EVENTS.lock();

    if let Some(events) = pending.get_mut(&(extension_id, external_client_id)) {
        if events.is_empty() {
            Vec::new()
        } else {
            core::mem::take(events)
        }
    } else {
        Vec::new()
    }
}

fn cleanup_extension_input_events(extension_id: u32, external_client_id: u32) {
    let mut pending = PENDING_EXTENSION_INPUT_EVENTS.lock();
    pending.remove(&(extension_id, external_client_id));
}

/// Queue a server->client protocol message for a specific window.
pub fn send_message_to_window(window_id: u32, msg_type: u32, payload: Vec<u8>) {
    let mut pending = PENDING_SERVER_FRAMES.lock();
    let was_registered = pending.contains_key(&window_id);
    let frames = pending.entry(window_id).or_insert_with(Vec::new);
    let should_wake = frames.is_empty();
    if !was_registered {
        println!(
            "[IpcServer] Warning: server message queued for unregistered window {} (msg_type={}); creating queue",
            window_id, msg_type
        );
    }
    frames.push(PendingServerFrame {
        msg_type,
        flags: 0,
        request_id: 0,
        payload,
    });
    drop(pending);
    if should_wake {
        wake_window_owner(window_id);
    }
}

/// Queue a server->client protocol message for a specific client (by client_id).
/// This is used for responses to clients that don't have windows (like stemd).
pub fn send_message_to_client(client_id: usize, msg_type: u32, payload: Vec<u8>) {
    send_message_to_client_routed(client_id, msg_type, 0, 0, payload);
}

pub fn send_response_to_client(client_id: usize, msg_type: u32, request_id: u8, payload: Vec<u8>) {
    send_message_to_client_routed(
        client_id,
        msg_type,
        protocol::MessageHeader::FLAG_IS_RESPONSE,
        request_id,
        payload,
    );
}

fn send_message_to_client_routed(
    client_id: usize,
    msg_type: u32,
    flags: u8,
    request_id: u8,
    payload: Vec<u8>,
) {
    let mut pending = PENDING_CLIENT_RESPONSES.lock();
    // The client thread removes this entry before publishing disconnect.
    // A response which completes after that point must be dropped; recreating
    // the entry would leak an undeliverable queue and make broadcasts retain a
    // dead client forever.
    let Some(frames) = pending.get_mut(&client_id) else {
        return;
    };
    let should_wake = frames.is_empty();
    frames.push(PendingServerFrame {
        msg_type,
        flags,
        request_id,
        payload,
    });
    drop(pending);
    if should_wake {
        wake_client(client_id);
    }
}

/// Broadcast a server->client protocol message to all connected clients.
/// This is used for events like focus changes that all clients should be aware of.
pub fn broadcast_message_to_all_clients(msg_type: u32, payload: Vec<u8>) {
    let mut pending = PENDING_CLIENT_RESPONSES.lock();
    let client_ids: Vec<usize> = pending.keys().copied().collect();
    drop(pending);

    println!(
        "[IpcServer] Broadcasting message to {} clients (msg_type={}, payload_len={})",
        client_ids.len(),
        msg_type,
        payload.len()
    );

    for client_id in client_ids {
        // Clone the payload for each client
        let payload_clone = payload.clone();
        send_message_to_client(client_id, msg_type, payload_clone);
    }
}

fn active_input_method() -> Option<InputMethodService> {
    let active_ime_id = *ACTIVE_IME_ID.lock();
    let ime_id = active_ime_id?;
    INPUT_METHODS.lock().get(&ime_id).cloned()
}

fn text_input_context(context_id: u32) -> Option<TextInputContext> {
    TEXT_INPUT_CONTEXTS.lock().get(&context_id).cloned()
}

fn send_ime_context_frame(msg_type: u32, context: &TextInputContext) {
    let Some(ime) = active_input_method() else {
        return;
    };
    let _seat_id = context.seat_id;
    let state = &context.current;
    let payload = sws_protocol::payload_ime_context(
        context.context_id,
        context.window_id,
        context.serial,
        (
            state.cursor_x,
            state.cursor_y,
            state.cursor_width,
            state.cursor_height,
        ),
        state.content_hint,
        state.content_purpose,
        state.text_change_cause,
        state.cursor_byte,
        state.anchor_byte,
        &state.surrounding_text,
    );
    send_message_to_client(ime.client_id, msg_type, payload);
}

fn deactivate_context(context_id: u32) {
    {
        let mut contexts = TEXT_INPUT_CONTEXTS.lock();
        if let Some(context) = contexts.get_mut(&context_id) {
            context.keyboard_grabbed = false;
        }
    }
    release_pending_ime_keys(Some(context_id));

    let Some(context) = text_input_context(context_id) else {
        return;
    };
    let Some(ime) = active_input_method() else {
        return;
    };
    let payload = sws_protocol::payload_ime_deactivate(context.context_id, context.serial);
    send_message_to_client(
        ime.client_id,
        sws_protocol::server_msg::IME_DEACTIVATE,
        payload.to_vec(),
    );
}

fn activate_text_input_for_window(window_id: u32) {
    let next_context = {
        let contexts = TEXT_INPUT_CONTEXTS.lock();
        contexts
            .values()
            .find(|context| context.enabled && context.window_id == window_id)
            .cloned()
    };

    let old_context_id = {
        let mut active = ACTIVE_TEXT_INPUT_CONTEXT.lock();
        let old_context_id = *active;
        *active = next_context.as_ref().map(|context| context.context_id);
        old_context_id
    };

    if let Some(old_context_id) = old_context_id
        && next_context
            .as_ref()
            .map_or(true, |context| context.context_id != old_context_id)
    {
        deactivate_context(old_context_id);
    }

    if let Some(context) = next_context {
        send_ime_context_frame(sws_protocol::server_msg::IME_ACTIVATE, &context);
    }
}

/// Update focused window state for text-input activation.
pub fn set_focused_window(window_id: u32) {
    {
        let mut focused = FOCUSED_WINDOW_ID.lock();
        *focused = Some(window_id);
    }
    activate_text_input_for_window(window_id);
}

fn create_text_input_context(client_id: usize, window_id: u32, seat_id: u32) -> TextInputContext {
    let context_id = NEXT_TEXT_INPUT_CONTEXT_ID.fetch_add(1, Ordering::Relaxed);
    let context = TextInputContext {
        client_id,
        context_id,
        window_id,
        seat_id,
        enabled: false,
        keyboard_grabbed: false,
        serial: 1,
        pending: TextInputState::new(),
        current: TextInputState::new(),
    };
    TEXT_INPUT_CONTEXTS
        .lock()
        .insert(context_id, context.clone());
    context
}

fn destroy_text_input_context(client_id: usize, context_id: u32) {
    let was_active = *ACTIVE_TEXT_INPUT_CONTEXT.lock() == Some(context_id);
    if was_active {
        deactivate_context(context_id);
    }

    let removed = {
        let mut contexts = TEXT_INPUT_CONTEXTS.lock();
        match contexts.get(&context_id) {
            Some(context) if context.client_id == client_id => contexts.remove(&context_id),
            _ => None,
        }
    };

    if removed.is_some() {
        let mut active = ACTIVE_TEXT_INPUT_CONTEXT.lock();
        if *active == Some(context_id) {
            *active = None;
        }
    }
}

fn cleanup_text_input_contexts_for_client(client_id: usize) {
    let active_id = *ACTIVE_TEXT_INPUT_CONTEXT.lock();
    if let Some(active_id) = active_id
        && let Some(context) = text_input_context(active_id)
        && context.client_id == client_id
    {
        deactivate_context(active_id);
    }

    let removed: Vec<u32> = {
        let mut contexts = TEXT_INPUT_CONTEXTS.lock();
        let ids: Vec<u32> = contexts
            .values()
            .filter(|context| context.client_id == client_id)
            .map(|context| context.context_id)
            .collect();
        for id in &ids {
            contexts.remove(id);
        }
        ids
    };

    let mut active = ACTIVE_TEXT_INPUT_CONTEXT.lock();
    if let Some(active_id) = *active
        && removed.iter().any(|id| *id == active_id)
    {
        *active = None;
        drop(active);
        deactivate_context(active_id);
    }
}

fn set_text_input_enabled(client_id: usize, context_id: u32, enabled: bool) {
    let window_id = {
        let mut contexts = TEXT_INPUT_CONTEXTS.lock();
        let Some(context) = contexts.get_mut(&context_id) else {
            return;
        };
        if context.client_id != client_id {
            return;
        }
        context.enabled = enabled;
        if !enabled {
            context.keyboard_grabbed = false;
        }
        context.window_id
    };

    let focused_window = *FOCUSED_WINDOW_ID.lock();
    if enabled && focused_window == Some(window_id) {
        activate_text_input_for_window(window_id);
    } else if !enabled {
        let mut active = ACTIVE_TEXT_INPUT_CONTEXT.lock();
        if *active == Some(context_id) {
            *active = None;
            drop(active);
            deactivate_context(context_id);
        }
    }
}

fn update_text_input_cursor_rect(
    client_id: usize,
    context_id: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) {
    let mut contexts = TEXT_INPUT_CONTEXTS.lock();
    let Some(context) = contexts.get_mut(&context_id) else {
        return;
    };
    if context.client_id != client_id {
        return;
    }
    context.pending.cursor_x = x;
    context.pending.cursor_y = y;
    context.pending.cursor_width = width;
    context.pending.cursor_height = height;
}

fn update_text_input_surrounding_text(
    client_id: usize,
    context_id: u32,
    cursor_byte: u32,
    anchor_byte: u32,
    text: &[u8],
) {
    let mut contexts = TEXT_INPUT_CONTEXTS.lock();
    let Some(context) = contexts.get_mut(&context_id) else {
        return;
    };
    if context.client_id != client_id {
        return;
    }
    context.pending.cursor_byte = cursor_byte;
    context.pending.anchor_byte = anchor_byte;
    context.pending.surrounding_text.clear();
    context
        .pending
        .surrounding_text
        .extend_from_slice(&text[..text.len().min(sws_protocol::TEXT_INPUT_MAX_BYTES)]);
}

fn update_text_input_content_type(client_id: usize, context_id: u32, hint: u32, purpose: u32) {
    let mut contexts = TEXT_INPUT_CONTEXTS.lock();
    let Some(context) = contexts.get_mut(&context_id) else {
        return;
    };
    if context.client_id != client_id {
        return;
    }
    context.pending.content_hint = hint;
    context.pending.content_purpose = purpose;
}

fn update_text_input_change_cause(client_id: usize, context_id: u32, cause: u32) {
    let mut contexts = TEXT_INPUT_CONTEXTS.lock();
    let Some(context) = contexts.get_mut(&context_id) else {
        return;
    };
    if context.client_id != client_id {
        return;
    }
    context.pending.text_change_cause = cause;
}

fn commit_text_input_state(client_id: usize, context_id: u32, client_serial: u32) {
    let context = {
        let mut contexts = TEXT_INPUT_CONTEXTS.lock();
        let Some(context) = contexts.get_mut(&context_id) else {
            return;
        };
        if context.client_id != client_id || context.serial != client_serial {
            return;
        }
        context.current = context.pending.clone();
        context.serial = context.serial.saturating_add(1);
        context.clone()
    };

    if *ACTIVE_TEXT_INPUT_CONTEXT.lock() == Some(context_id) && context.enabled {
        send_ime_context_frame(sws_protocol::server_msg::IME_CONTEXT_STATE, &context);
    }
    push_ipc_event(IpcEvent::TextInputContextUpdated { context_id });
}

/// Set the stable input method name SWS should prefer as services register.
///
/// # Arguments
///
/// * `name` - Configured input method name, or `None` for first-registered fallback.
///
/// # Returns
///
/// This function does not return a value.
pub fn set_preferred_input_method(name: Option<String>) {
    *PREFERRED_IME_NAME.lock() = name;
}

fn register_input_method(client_id: usize, name: &[u8], capabilities: u32) -> InputMethodService {
    let ime_id = NEXT_IME_ID.fetch_add(1, Ordering::Relaxed);
    let service = InputMethodService {
        client_id,
        ime_id,
        name: String::from_utf8_lossy(name).into_owned(),
        capabilities,
    };
    INPUT_METHODS.lock().insert(ime_id, service.clone());

    let preferred_name = PREFERRED_IME_NAME.lock().clone();
    let should_activate =
        ACTIVE_IME_ID.lock().is_none() || preferred_name.as_deref() == Some(service.name.as_str());
    if should_activate {
        activate_input_method(ime_id, false);
    }
    service
}

fn set_active_input_method(ime_id: u32) {
    if !activate_input_method(ime_id, true) {
        println!("[SWS] Ignoring unknown input method id={}", ime_id);
    }
}

fn activate_input_method(ime_id: u32, persist_selection: bool) -> bool {
    let Some(service) = INPUT_METHODS.lock().get(&ime_id).cloned() else {
        return false;
    };

    let changed = {
        let mut active = ACTIVE_IME_ID.lock();
        let changed = *active != Some(ime_id);
        *active = Some(ime_id);
        changed
    };

    if changed {
        release_pending_ime_keys(None);
        clear_keyboard_grabs();
        if let Some(context_id) = *ACTIVE_TEXT_INPUT_CONTEXT.lock()
            && let Some(context) = text_input_context(context_id)
        {
            send_ime_context_frame(sws_protocol::server_msg::IME_ACTIVATE, &context);
        }
    }

    if persist_selection {
        *PREFERRED_IME_NAME.lock() = Some(service.name.clone());
        match config::persist_active_input_method(&service.name) {
            Ok(()) => println!(
                "[SWS] Persisted active input method: {} ({})",
                service.name, service.ime_id
            ),
            Err(error) => println!(
                "[SWS] Failed to persist active input method {}: {}",
                service.name, error
            ),
        }
    }

    true
}

fn append_input_method_payload(
    payload: &mut Vec<u8>,
    method: &InputMethodService,
    active_ime_id: Option<u32>,
) {
    let name = method.name.as_bytes();
    let name_len = name.len().min(sws_protocol::TEXT_INPUT_MAX_BYTES);
    payload.extend_from_slice(&method.ime_id.to_le_bytes());
    payload.extend_from_slice(&method.capabilities.to_le_bytes());
    payload.extend_from_slice(&((active_ime_id == Some(method.ime_id)) as u32).to_le_bytes());
    payload.extend_from_slice(&(name_len as u32).to_le_bytes());
    payload.extend_from_slice(&name[..name_len]);
}

fn input_methods_payload() -> Vec<u8> {
    let active_ime_id = *ACTIVE_IME_ID.lock();
    let methods = INPUT_METHODS.lock();
    let mut payload = Vec::new();
    payload.extend_from_slice(&(methods.len() as u32).to_le_bytes());
    for method in methods.values() {
        append_input_method_payload(&mut payload, method, active_ime_id);
    }
    payload
}

fn active_input_method_payload() -> Vec<u8> {
    let active_ime_id = *ACTIVE_IME_ID.lock();
    let methods = INPUT_METHODS.lock();
    let mut payload = Vec::new();
    if let Some(ime_id) = active_ime_id
        && let Some(method) = methods.get(&ime_id)
    {
        payload.extend_from_slice(&1u32.to_le_bytes());
        append_input_method_payload(&mut payload, method, active_ime_id);
        return payload;
    }
    payload.extend_from_slice(&0u32.to_le_bytes());
    payload
}

fn clear_keyboard_grabs() {
    let mut contexts = TEXT_INPUT_CONTEXTS.lock();
    for context in contexts.values_mut() {
        context.keyboard_grabbed = false;
    }
}

fn drain_pending_ime_keys(context_id: Option<u32>) -> Vec<PendingImeKey> {
    let mut pending = PENDING_IME_KEYS.lock();
    if let Some(context_id) = context_id {
        let key_serials: Vec<u32> = pending
            .iter()
            .filter(|(_, key)| key.context_id == context_id)
            .map(|(serial, _)| *serial)
            .collect();
        let mut keys = Vec::new();
        for key_serial in key_serials {
            if let Some(key) = pending.remove(&key_serial) {
                keys.push(key);
            }
        }
        keys
    } else {
        let keys = pending.values().cloned().collect();
        pending.clear();
        keys
    }
}

fn release_pending_ime_keys(context_id: Option<u32>) {
    for pending in drain_pending_ime_keys(context_id) {
        send_input_to_window(
            pending.window_id,
            pending.time,
            pending.type_,
            pending.code,
            pending.value,
        );
        send_input_to_window(
            pending.window_id,
            pending.time,
            super::input::event_types::EV_SYN,
            0,
            0,
        );
    }
}

fn set_ime_keyboard_grabbed(client_id: usize, context_id: u32, grabbed: bool) {
    let Some(ime) = active_input_method() else {
        return;
    };
    if ime.client_id != client_id {
        return;
    }
    let active_context_id = *ACTIVE_TEXT_INPUT_CONTEXT.lock();
    let mut contexts = TEXT_INPUT_CONTEXTS.lock();
    let Some(context) = contexts.get_mut(&context_id) else {
        return;
    };
    if active_context_id != Some(context_id) || !context.enabled {
        return;
    }
    context.keyboard_grabbed = grabbed;
    drop(contexts);

    if !grabbed {
        release_pending_ime_keys(Some(context_id));
    }
}

/// Send an IME trigger to the active input method.
///
/// Returns `true` when the trigger was delivered and should be consumed.
pub fn send_input_method_trigger(window_id: u32, time: u64, code: u16) -> bool {
    let Some(context_id) = *ACTIVE_TEXT_INPUT_CONTEXT.lock() else {
        return false;
    };
    let Some(context) = text_input_context(context_id) else {
        return false;
    };
    if !context.enabled || context.window_id != window_id {
        return false;
    }
    let Some(ime) = active_input_method() else {
        return false;
    };

    let payload = sws_protocol::payload_ime_trigger(
        context.context_id,
        context.serial,
        sws_protocol::ime_trigger::TOGGLE,
        code,
        time,
    );
    send_message_to_client(
        ime.client_id,
        sws_protocol::server_msg::IME_TRIGGER,
        payload.to_vec(),
    );
    true
}

fn cleanup_input_methods_for_client(client_id: usize) {
    let removed: Vec<u32> = {
        let mut methods = INPUT_METHODS.lock();
        let ids: Vec<u32> = methods
            .values()
            .filter(|method| method.client_id == client_id)
            .map(|method| method.ime_id)
            .collect();
        for id in &ids {
            methods.remove(id);
        }
        ids
    };

    let active_was_removed = ACTIVE_IME_ID
        .lock()
        .is_some_and(|active_id| removed.iter().any(|id| *id == active_id));
    if active_was_removed {
        release_pending_ime_keys(None);
        clear_keyboard_grabs();
        *ACTIVE_IME_ID.lock() = None;

        let preferred_name = PREFERRED_IME_NAME.lock().clone();
        let replacement = {
            let methods = INPUT_METHODS.lock();
            preferred_name
                .as_deref()
                .and_then(|name| {
                    methods
                        .values()
                        .find(|method| method.name == name)
                        .map(|method| method.ime_id)
                })
                .or_else(|| methods.keys().next().copied())
        };
        if let Some(ime_id) = replacement {
            activate_input_method(ime_id, false);
        }
    }
}

/// Forward a key event to the active input method.
///
/// Returns `true` when SWS queued the event for IME arbitration and the caller
/// should not deliver it directly to the application yet.
pub fn send_key_to_input_method(
    window_id: u32,
    time: u64,
    type_: u16,
    code: u16,
    value: i32,
) -> bool {
    let Some(context_id) = *ACTIVE_TEXT_INPUT_CONTEXT.lock() else {
        return false;
    };
    let Some(context) = text_input_context(context_id) else {
        return false;
    };
    if !context.enabled || !context.keyboard_grabbed || context.window_id != window_id {
        return false;
    }
    let Some(ime) = active_input_method() else {
        return false;
    };

    let key_serial = NEXT_IME_KEY_SERIAL.fetch_add(1, Ordering::Relaxed);
    PENDING_IME_KEYS.lock().insert(
        key_serial,
        PendingImeKey {
            context_id: context.context_id,
            window_id,
            time,
            type_,
            code,
            value,
        },
    );

    let payload = sws_protocol::payload_ime_key_event(
        context.context_id,
        key_serial,
        window_id,
        time,
        type_,
        code,
        value,
    );
    send_message_to_client(
        ime.client_id,
        sws_protocol::server_msg::IME_KEY_EVENT,
        payload.to_vec(),
    );
    true
}

fn handle_ime_key_handled(key_serial: u32, handled: bool) {
    let pending = PENDING_IME_KEYS.lock().remove(&key_serial);
    let Some(pending) = pending else {
        return;
    };
    if handled {
        return;
    }
    send_input_to_window(
        pending.window_id,
        pending.time,
        pending.type_,
        pending.code,
        pending.value,
    );
    send_input_to_window(
        pending.window_id,
        pending.time,
        super::input::event_types::EV_SYN,
        0,
        0,
    );
}

fn context_serial_and_window(context_id: u32) -> Option<(u32, u32)> {
    text_input_context(context_id).map(|context| (context.serial, context.window_id))
}

pub fn text_input_cursor_rect(context_id: u32) -> Option<TextInputCursorRect> {
    text_input_context(context_id).map(|context| TextInputCursorRect {
        window_id: context.window_id,
        x: context.current.cursor_x,
        y: context.current.cursor_y,
        width: context.current.cursor_width,
        height: context.current.cursor_height,
    })
}

fn client_is_active_input_method(client_id: usize) -> bool {
    let Some(ime_id) = *ACTIVE_IME_ID.lock() else {
        return false;
    };
    INPUT_METHODS
        .lock()
        .get(&ime_id)
        .is_some_and(|service| service.client_id == client_id)
}

fn client_can_mutate_ime_context(client_id: usize, context_id: u32) -> bool {
    if !client_is_active_input_method(client_id) {
        return false;
    }
    if *ACTIVE_TEXT_INPUT_CONTEXT.lock() != Some(context_id) {
        return false;
    }
    text_input_context(context_id).is_some_and(|context| context.enabled)
}

fn send_text_input_done(window_id: u32, context_id: u32, serial: u32) {
    let payload = sws_protocol::payload_text_input_done(context_id, serial);
    send_message_to_window(
        window_id,
        sws_protocol::server_msg::TEXT_INPUT_DONE,
        payload.to_vec(),
    );
}

fn forward_ime_preedit(
    context_id: u32,
    cursor_byte: u32,
    anchor_byte: u32,
    text: &[u8],
    spans: &[u8],
) {
    let Some((serial, window_id)) = context_serial_and_window(context_id) else {
        return;
    };
    let payload = sws_protocol::payload_text_input_preedit(
        context_id,
        serial,
        cursor_byte,
        anchor_byte,
        text,
        spans,
    );
    send_message_to_window(
        window_id,
        sws_protocol::server_msg::TEXT_INPUT_PREEDIT,
        payload,
    );
    send_text_input_done(window_id, context_id, serial);
}

fn forward_ime_commit(context_id: u32, text: &[u8]) {
    let Some((serial, window_id)) = context_serial_and_window(context_id) else {
        return;
    };
    println!(
        "[IpcServer] IME commit context={} window={} serial={} text={}",
        context_id,
        window_id,
        serial,
        String::from_utf8_lossy(text)
    );
    let payload = sws_protocol::payload_text_input_commit(context_id, serial, text);
    send_message_to_window(
        window_id,
        sws_protocol::server_msg::TEXT_INPUT_COMMIT,
        payload,
    );
    send_text_input_done(window_id, context_id, serial);
}

fn forward_ime_delete_surrounding_text(context_id: u32, before_bytes: u32, after_bytes: u32) {
    let Some((serial, window_id)) = context_serial_and_window(context_id) else {
        return;
    };
    let payload = sws_protocol::payload_text_input_delete_surrounding_text(
        context_id,
        serial,
        before_bytes,
        after_bytes,
    );
    send_message_to_window(
        window_id,
        sws_protocol::server_msg::TEXT_INPUT_DELETE_SURROUNDING_TEXT,
        payload.to_vec(),
    );
    send_text_input_done(window_id, context_id, serial);
}

fn forward_ime_status(context_id: u32, state: u32, mode_id: u32, flags: u32, mode_label: &[u8]) {
    let Some((serial, window_id)) = context_serial_and_window(context_id) else {
        return;
    };
    let payload = sws_protocol::payload_text_input_status(
        context_id, serial, state, mode_id, flags, mode_label,
    );
    send_message_to_window(
        window_id,
        sws_protocol::server_msg::TEXT_INPUT_STATUS,
        payload,
    );
}

/// Get application session information for a window.
/// Returns (app_name, menu_titles) if the session exists.
pub fn get_app_session_info(window_id: u32) -> (String, String) {
    let sessions = APP_SESSIONS.lock();
    if let Some(session) = sessions.get(&window_id) {
        (session.app_name.clone(), session.menu_titles.clone())
    } else {
        (String::new(), String::new())
    }
}

pub fn set_app_session_menu_titles(window_id: u32, menu_titles: String) -> bool {
    let mut sessions = APP_SESSIONS.lock();
    if let Some(session) = sessions.get_mut(&window_id) {
        session.menu_titles = menu_titles;
        true
    } else {
        false
    }
}

fn pop_pending_server_frames(window_id: u32) -> Vec<PendingServerFrame> {
    let mut pending = PENDING_SERVER_FRAMES.lock();
    if let Some(frames) = pending.get_mut(&window_id) {
        if frames.is_empty() {
            Vec::new() // Already empty, no reallocation needed
        } else {
            core::mem::take(frames)
        }
    } else {
        Vec::new()
    }
}

/// Get pending input events for a window (called by client thread, O(log n) lookup)
fn pop_pending_input_events(window_id: u32) -> Vec<PendingInputEvent> {
    let mut pending = PENDING_INPUT_EVENTS.lock();

    if let Some(events) = pending.get_mut(&window_id) {
        if events.is_empty() {
            Vec::new() // Already empty, no reallocation needed
        } else {
            core::mem::take(events)
        }
    } else {
        Vec::new()
    }
}

/// Pop pending server responses for a specific client (by client_id)
fn pop_pending_client_responses(client_id: usize) -> Vec<PendingServerFrame> {
    let mut pending = PENDING_CLIENT_RESPONSES.lock();
    if let Some(frames) = pending.get_mut(&client_id) {
        if frames.is_empty() {
            Vec::new()
        } else {
            let frames = core::mem::take(frames);
            if is_sws_debug_enabled() {
                println!(
                    "[IpcServer] Popping {} pending responses for client {}",
                    frames.len(),
                    client_id
                );
            }
            frames
        }
    } else {
        Vec::new()
    }
}

/// IPC Server - manages Socket VFS connections
pub struct IpcServer {
    socket_path: &'static str,
    accept_thread_started: bool,
}

impl IpcServer {
    /// Create a new IPC server
    pub fn new(socket_path: &'static str) -> Result<Self, &'static str> {
        println!("[IpcServer] Initializing at {}", socket_path);

        Ok(Self {
            socket_path,
            accept_thread_started: false,
        })
    }

    /// Start listening for connections in a separate thread
    pub fn listen(&mut self) -> Result<(), &'static str> {
        if self.accept_thread_started {
            return Ok(());
        }

        println!("[IpcServer] Creating socket at {}", self.socket_path);

        // Create and setup socket
        let server_socket = Socket::new().map_err(|e| {
            println!("[IpcServer] Socket::new() failed: {:?}", e);
            "Failed to create socket"
        })?;

        println!(
            "[IpcServer] Socket created (handle {})",
            server_socket.as_raw()
        );

        server_socket.bind(self.socket_path).map_err(|e| {
            println!("[IpcServer] bind() failed: {:?}", e);
            "Failed to bind socket"
        })?;

        println!("[IpcServer] Socket bound to {}", self.socket_path);

        server_socket.listen(10).map_err(|e| {
            println!("[IpcServer] listen() failed: {:?}", e);
            "Failed to listen"
        })?;

        let server_handle = server_socket.as_raw();
        println!("[IpcServer] Socket listening (handle {})", server_handle);

        // Move socket to accept thread
        // HandleTable is cloned but Arc<SocketObject> is shared
        thread::Builder::new()
            .spawn(move || {
                accept_thread_main(server_socket);
            })
            .map_err(|_| "Failed to start SWS accept thread")?;

        self.accept_thread_started = true;
        println!("[IpcServer] Accept thread started");
        Ok(())
    }

    /// Get all pending IPC events (non-blocking)
    pub fn process_messages(&mut self) -> Result<Vec<IpcEvent>, &'static str> {
        Ok(pop_all_ipc_events())
    }

    /// Send a message to a specific client (not yet implemented for multi-threaded)
    #[allow(dead_code)]
    pub fn send_to_client(
        &mut self,
        _client_id: usize,
        message: protocol::ServerMessage,
    ) -> Result<(), &'static str> {
        // TODO: Implement client response mechanism
        // For now, just log
        println!(
            "[IpcServer] Would send message {:?} to client {}",
            message, _client_id
        );
        Ok(())
    }
}

/// Accept thread main function
fn accept_thread_main(server_socket: Socket) {
    println!(
        "[AcceptThread] Starting with socket handle {}",
        server_socket.as_raw()
    );

    // Evidence-only: log a stack address hint for this thread.
    // If this falls inside the compositor backbuffer range, it indicates overlap risk.
    let stack_marker: u8 = 0;
    let sp_hint = (&stack_marker as *const u8) as usize;
    println!("[AcceptThread] stack marker addr: 0x{:x}", sp_hint);

    let mut client_id_counter: usize = 0;
    let mut poll_count: u64 = 0;

    loop {
        // Kernel accept() currently returns WouldBlock when no connections are
        // pending, so we poll with a short sleep to avoid busy looping.
        poll_count = poll_count.wrapping_add(1);
        if poll_count % 100 == 1 {
            println!(
                "[AcceptThread] Polling accept() on handle {}...",
                server_socket.as_raw()
            );
        }
        match server_socket.accept() {
            Ok(client_socket) => {
                let client_id = client_id_counter;
                client_id_counter += 1;

                println!(
                    "[AcceptThread] Accepted client {} (socket handle: {})",
                    client_id,
                    client_socket.as_raw()
                );

                // Register client for broadcast messages
                {
                    let mut pending = PENDING_CLIENT_RESPONSES.lock();
                    pending.entry(client_id).or_insert_with(Vec::new);
                    println!(
                        "[AcceptThread] Registered client {} for broadcast messages",
                        client_id
                    );
                }

                let client_wake = match std::task::pipe() {
                    Ok((wake_read, wake_write)) => {
                        CLIENT_WAKES.lock().insert(
                            client_id,
                            ClientWake {
                                write_handle: wake_write,
                                pending: false,
                            },
                        );
                        Some(wake_read)
                    }
                    Err(_) => {
                        println!(
                            "[AcceptThread] Failed to create wake pipe for client {}",
                            client_id
                        );
                        None
                    }
                };

                // A transient thread-allocation failure must reject only this
                // connection, not panic the process that owns the desktop.
                if thread::Builder::new()
                    .spawn(move || {
                        client_thread_main(client_id, client_socket, client_wake);
                    })
                    .is_err()
                {
                    PENDING_CLIENT_RESPONSES.lock().remove(&client_id);
                    CLIENT_WAKES.lock().remove(&client_id);
                    println!(
                        "[AcceptThread] Failed to start handler for client {}",
                        client_id
                    );
                }
            }
            Err(std::socket::SocketError::WouldBlock) => {
                thread::sleep(core::time::Duration::from_millis(10));
                continue;
            }
            Err(_) => {
                // At the moment `Socket::accept()` maps all failures to `WouldBlock`,
                // but keep this arm for forward compatibility.
                thread::sleep(core::time::Duration::from_millis(10));
                continue;
            }
        }
    }
}

/// Client thread main function
fn client_thread_main(client_id: usize, mut socket: Socket, wake_read: Option<Handle>) {
    println!(
        "[ClientThread {}] Started (socket handle: {})",
        client_id,
        socket.as_raw()
    );

    // Enable non-blocking mode for event-driven I/O
    if let Err(e) = socket.set_nonblocking(true) {
        println!(
            "[ClientThread {}] Failed to set non-blocking mode: {:?}",
            client_id, e
        );
        return;
    }
    println!("[ClientThread {}] Enabled non-blocking mode", client_id);

    // Evidence-only: log a stack address hint for this thread.
    let stack_marker: u8 = 0;
    let sp_hint = (&stack_marker as *const u8) as usize;
    println!(
        "[ClientThread {}] stack marker addr: 0x{:x}",
        client_id, sp_hint
    );

    // Per-client window id generator (avoid collision between clients)
    let mut next_window_id: u32 = 100 + (client_id as u32 * 1000);
    let mut managed_windows: Vec<u32> = Vec::new();
    let mut window_resizable: BTreeMap<u32, bool> = BTreeMap::new();
    // Track if this client is an extension client (e.g., wayland_bridge)
    let mut is_extension_client: bool = false;
    let mut extension_id: u32 = 0;
    // Map window_id to external_client_id for extension windows
    let mut window_to_external_client: BTreeMap<u32, u32> = BTreeMap::new();

    // Debug: loop counter for periodic logging
    let mut loop_count: u64 = 0;

    let mut frame_reader = FrameReader::new();
    let mut atomic_frame = Vec::new();
    let mut stream_writer = ClientStreamWriter::new();
    println!("[ClientThread {}] Entering main loop", client_id);

    'main: loop {
        super::trace::ipc_client_loop();
        loop_count += 1;
        let _should_log = loop_count % 100 == 0; // Log every 100 iterations (more frequent)

        let mut has_events = match stream_writer.flush(&mut socket) {
            Ok(progressed) => {
                if progressed {
                    super::trace::ipc_flush_progress();
                }
                progressed
            }
            Err(error) => {
                println!(
                    "[ClientThread {}] Failed to flush queued output: {:?}",
                    client_id, error
                );
                break;
            }
        };

        // Send any pending input events for this client's windows
        let mut _total_events = 0;

        // First, check for pending responses addressed directly to this client
        // (for clients that don't have windows, like stemd)
        let client_responses = pop_pending_client_responses(client_id);
        for frame in client_responses {
            if is_sws_debug_enabled() {
                println!(
                    "[ClientThread {}] Queueing client response (msg_type={}, payload_len={})",
                    client_id,
                    frame.msg_type,
                    frame.payload.len()
                );
            }
            if let Err(e) = write_frame_routed(
                &mut stream_writer,
                frame.msg_type,
                frame.flags,
                frame.request_id,
                &frame.payload,
            ) {
                println!(
                    "[ClientThread {}] Failed to send client response: {:?}",
                    client_id, e
                );
                break 'main;
            }
            has_events = true;
        }

        for &window_id in &managed_windows {
            // Send queued server->client control messages for this window.
            let frames = pop_pending_server_frames(window_id);
            for frame in frames {
                if let Err(e) = write_frame_routed(
                    &mut stream_writer,
                    frame.msg_type,
                    frame.flags,
                    frame.request_id,
                    &frame.payload,
                ) {
                    println!(
                        "[ClientThread {}] Failed to send server message to window {}: {:?}",
                        client_id, window_id, e
                    );
                    break 'main;
                }
                has_events = true;
            }

            if is_extension_client {
                // Extension clients receive EXTENSION_INPUT_EVENT
                if let Some(&external_client_id) = window_to_external_client.get(&window_id) {
                    let events = pop_extension_input_events(extension_id, external_client_id);
                    if !events.is_empty() {
                        has_events = true;
                        _total_events += events.len();
                        for event in events {
                            let payload = protocol::payload_extension_input_event(
                                external_client_id,
                                window_id,
                                event.time,
                                event.type_,
                                event.code,
                                event.value,
                            );
                            if let Err(e) = write_frame(
                                &mut stream_writer,
                                protocol::server_msg::EXTENSION_INPUT_EVENT,
                                &payload,
                            ) {
                                println!(
                                    "[ClientThread {}] Failed to send extension input event to window {}: {:?}",
                                    client_id, window_id, e
                                );
                                break 'main;
                            }
                        }
                    }
                }
            } else {
                // Regular clients receive INPUT_EVENT
                let events = pop_pending_input_events(window_id);
                if !events.is_empty() {
                    has_events = true;
                    _total_events += events.len();
                    // println!(
                    //     "[ClientThread {}] Loop #{}: Found {} events for window {}",
                    //     client_id,
                    //     loop_count,
                    //     events.len(),
                    //     window_id
                    // );
                    for event in events {
                        let payload = protocol::payload_input_event(
                            window_id,
                            event.time,
                            event.type_,
                            event.code,
                            event.value,
                        );
                        if let Err(e) = write_frame(
                            &mut stream_writer,
                            protocol::server_msg::INPUT_EVENT,
                            &payload,
                        ) {
                            println!(
                                "[ClientThread {}] Failed to send input event to window {}: {:?}",
                                client_id, window_id, e
                            );
                            break 'main;
                        } else {
                            // println!(
                            //     "[ClientThread {}] Sent event: type={} code={} value={}",
                            //     client_id, event.type_, event.code, event.value
                            // );
                        }
                    }
                }
            }
        }

        match stream_writer.flush(&mut socket) {
            Ok(progressed) => {
                if progressed {
                    super::trace::ipc_flush_progress();
                }
                has_events |= progressed;
            }
            Err(error) => {
                println!(
                    "[ClientThread {}] Failed to flush queued output: {:?}",
                    client_id, error
                );
                break;
            }
        }

        // // Debug: log event queue status periodically
        // if should_log && !managed_windows.is_empty() {
        //     println!(
        //         "[ClientThread {}] Loop #{}: checked {} windows, found {} events",
        //         client_id,
        //         loop_count,
        //         managed_windows.len(),
        //         total_events
        //     );
        // }

        // // Always log before first read attempt or periodically
        // if loop_count <= 5 || loop_count % 100 == 0 {
        //     println!(
        //         "[ClientThread {}] Loop #{}: about to call read_frame (socket handle: {}, windows: {})",
        //         client_id,
        //         loop_count,
        //         socket.as_raw(),
        //         managed_windows.len()
        //     );
        // }

        let atomic_record = match poll_atomic_handle_frame(&socket, &mut atomic_frame) {
            Ok(record) => record,
            Err(FrameIoError::Protocol) => {
                println!("[ClientThread {}] Invalid atomic handle frame", client_id);
                break;
            }
            Err(error) => {
                println!(
                    "[ClientThread {}] Failed to receive atomic handle frame: {:?}",
                    client_id, error
                );
                break;
            }
        };

        let (header, payload, received_handle) = match atomic_record {
            Some((header, payload, handle)) => (header, payload, Some(handle)),
            None => match frame_reader.poll(&mut socket) {
                Ok(Some((header, payload))) => (header, payload, None),
                Ok(None) => {
                    if has_events && !stream_writer.has_pending() {
                        continue;
                    }
                    let socket_interest = POLLIN
                        | if stream_writer.has_pending() {
                            POLLOUT
                        } else {
                            0
                        };
                    let mut handles = match wake_read.as_ref() {
                        Some(wake) => [
                            PollHandle::new(socket.as_raw() as u32, socket_interest),
                            PollHandle::new(wake.as_raw() as u32, POLLIN),
                        ],
                        None => [
                            PollHandle::new(socket.as_raw() as u32, socket_interest),
                            PollHandle::new(socket.as_raw() as u32, 0),
                        ],
                    };
                    super::trace::ipc_poll();
                    let ready = match poll(&mut handles, CLIENT_POLL_TIMEOUT_NS) {
                        Ok(ready) => ready,
                        Err(error) => {
                            println!("[ClientThread {}] poll failed: {:?}", client_id, error);
                            thread::sleep(core::time::Duration::from_millis(10));
                            continue;
                        }
                    };
                    if ready > 0 {
                        super::trace::ipc_poll_ready();
                    }
                    let socket_revents = handles[0].revents;
                    let wake_revents = handles[1].revents;
                    let fatal_mask = POLLERR | POLLHUP | POLLNVAL;
                    super::trace::ipc_poll_result(ready, socket_revents, wake_revents, fatal_mask);
                    if (socket_revents & fatal_mask) != 0 {
                        println!(
                            "[ClientThread {}] Socket poll failed: revents=0x{:x}",
                            client_id, socket_revents
                        );
                        break;
                    }
                    if (wake_revents & fatal_mask) != 0 {
                        println!(
                            "[ClientThread {}] Wake pipe poll failed: revents=0x{:x}",
                            client_id, wake_revents
                        );
                        break;
                    }
                    if let Some(wake) = wake_read.as_ref()
                        && (handles[1].revents & POLLIN) != 0
                        && let Ok(stream) = wake.as_stream()
                    {
                        let mut byte = [0u8; 1];
                        if matches!(stream.read(&mut byte), Ok(1)) {
                            consume_client_wake(client_id);
                        }
                    }
                    continue;
                }
                Err(FrameIoError::Disconnected) => {
                    println!("[ClientThread {}] Client disconnected", client_id);
                    break;
                }
                Err(e) => {
                    println!("[ClientThread {}] Failed to read frame: {:?}", client_id, e);
                    break;
                }
            },
        };

        let request_id = header.request_id;
        super::trace::ipc_frame();
        let handle_required = matches!(
            header.msg_type_u32(),
            protocol::client_msg::REGISTER_SGFX_BUFFER
                | protocol::client_msg::EXTENSION_ATTACH_BUFFER
        );
        if handle_required != received_handle.is_some() {
            let _ = write_protocol_error(
                &mut stream_writer,
                request_id,
                protocol::error_codes::INVALID_SGFX_BUFFER,
            );
            continue;
        }

        match protocol::parse_client_message(header.msg_type_u32(), &payload) {
            Ok(ClientMessageRef::CreateWindow {
                app_id,
                app_name,
                menu_titles,
                width,
                height,
                window_type,
                resizable,
                focus_on_create,
                active_on_focus,
                initial_position,
                activation_token,
            }) => {
                // Convert &[u8] to String
                let app_id_str = String::from_utf8_lossy(app_id).into_owned();
                let app_name_str = String::from_utf8_lossy(app_name).into_owned();
                let menu_titles_str = String::from_utf8_lossy(menu_titles).into_owned();

                // Calculate buffer size
                let buffer_size = (width as u64)
                    .saturating_mul(height as u64)
                    .saturating_mul(4);

                let window_id = next_window_id;
                next_window_id = next_window_id.saturating_add(1);

                // Create AppSession
                let session = AppSession {
                    window_id,
                    app_id: app_id_str.clone(),
                    app_name: app_name_str.clone(),
                    menu_titles: menu_titles_str.clone(),
                };
                {
                    let mut sessions = APP_SESSIONS.lock();
                    sessions.insert(window_id, session);
                }

                // Create shared memory region for this window
                println!(
                    "[ClientThread {}] Creating SHM for window {} ({}x{} = {} bytes) [app={}, name={}]",
                    client_id, window_id, width, height, buffer_size, app_id_str, app_name_str
                );

                match SharedMemory::create(buffer_size as usize, permissions::READ_WRITE) {
                    Ok(shm) => {
                        // Map SHM into server's address space for compositor access
                        let shm_mapped_addr = match shm.as_handle().as_memory_mapping() {
                            Ok(mapper) => {
                                match mapper.mmap(
                                    0,
                                    buffer_size as usize,
                                    permissions::READ_WRITE,
                                    mmap_flags::SHARED,
                                    0,
                                ) {
                                    Ok(addr) => {
                                        println!(
                                            "[ClientThread {}] SHM mapped at 0x{:x}",
                                            client_id, addr
                                        );

                                        // Zero-initialize the SHM for deterministic behavior
                                        unsafe {
                                            let ptr = addr as *mut u8;
                                            for i in 0..buffer_size as usize {
                                                *ptr.add(i) = 0;
                                            }
                                        }
                                        println!(
                                            "[ClientThread {}] SHM zero-initialized",
                                            client_id
                                        );

                                        // Sample first few bytes to verify
                                        unsafe {
                                            let ptr = addr as *const u8;
                                            println!(
                                                "[ClientThread {}] SHM first 16 bytes: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                                                client_id,
                                                *ptr.add(0),
                                                *ptr.add(1),
                                                *ptr.add(2),
                                                *ptr.add(3),
                                                *ptr.add(4),
                                                *ptr.add(5),
                                                *ptr.add(6),
                                                *ptr.add(7),
                                                *ptr.add(8),
                                                *ptr.add(9),
                                                *ptr.add(10),
                                                *ptr.add(11),
                                                *ptr.add(12),
                                                *ptr.add(13),
                                                *ptr.add(14),
                                                *ptr.add(15)
                                            );
                                        }

                                        Some(addr)
                                    }
                                    Err(_) => {
                                        println!("[ClientThread {}] Failed to mmap SHM", client_id);
                                        None
                                    }
                                }
                            }
                            Err(_) => {
                                println!(
                                    "[ClientThread {}] SHM does not support mapping",
                                    client_id
                                );
                                None
                            }
                        };

                        window_resizable.insert(window_id, resizable);

                        let payload = protocol::payload_window_created(window_id, buffer_size);
                        if let Err(e) = write_handle_frame_response(
                            &socket,
                            shm.as_handle(),
                            protocol::server_msg::WINDOW_CREATED,
                            request_id,
                            &payload,
                        ) {
                            println!(
                                "[ClientThread {}] Failed to send atomic WINDOW_CREATED: {:?}",
                                client_id, e
                            );
                            continue;
                        }

                        // Register window for input event routing
                        register_window(window_id, client_id);

                        // Track this window for input event polling
                        managed_windows.push(window_id);

                        // Notify compositor to create window entry with SHM ownership
                        push_ipc_event(IpcEvent::CreateWindow {
                            client_id,
                            app_id: app_id.to_vec(),
                            window_id,
                            width,
                            height,
                            window_type,
                            resizable,
                            focus_on_create,
                            active_on_focus,
                            initial_position,
                            activation_token: activation_token.map(|token| token.to_vec()),
                            shm: Some(shm),
                            shm_mapped_addr,
                            shm_size: buffer_size as usize,
                        });
                    }
                    Err(e) => {
                        println!("[ClientThread {}] Failed to create SHM: {:?}", client_id, e);
                        // Send error response (optional)
                        continue;
                    }
                }
            }
            Ok(ClientMessageRef::DestroyWindow { window_id }) => {
                println!(
                    "[ClientThread {}] DestroyWindow request for window {}",
                    client_id, window_id
                );

                cleanup_window_state(window_id);
                window_resizable.remove(&window_id);
                if let Some(external_client_id) = window_to_external_client.remove(&window_id) {
                    cleanup_extension_input_events(extension_id, external_client_id);
                }

                // Remove from managed windows
                managed_windows.retain(|&id| id != window_id);

                push_ipc_event(IpcEvent::DestroyWindow {
                    client_id,
                    window_id,
                });
            }
            Ok(ClientMessageRef::UpdateBuffer {
                window_id,
                x,
                y,
                width,
                height,
            }) => {
                // UpdateBuffer (damage notification) - optional
                push_ipc_event(IpcEvent::BufferUpdated {
                    window_id,
                    damage_x: x,
                    damage_y: y,
                    damage_width: width,
                    damage_height: height,
                });
            }
            Ok(ClientMessageRef::RequestMoveWindow { window_id }) => {
                println!(
                    "[IpcServer] RequestMoveWindow received for window {}",
                    window_id
                );
                push_ipc_event(IpcEvent::RequestMove { window_id });
            }
            Ok(ClientMessageRef::MoveWindow { window_id, x, y }) => {
                push_ipc_event(IpcEvent::MoveWindow { window_id, x, y });
            }
            Ok(ClientMessageRef::SetWindowParent {
                window_id,
                parent_id,
            }) => {
                push_ipc_event(IpcEvent::SetWindowParent {
                    window_id,
                    parent_id,
                });
            }
            Ok(ClientMessageRef::SetWindowTransientFlags { window_id, flags }) => {
                push_ipc_event(IpcEvent::SetWindowTransientFlags { window_id, flags });
            }
            Ok(ClientMessageRef::SetWindowSizeLimits {
                window_id,
                min_width,
                min_height,
                max_width,
                max_height,
            }) => {
                println!(
                    "[ClientThread {}] SetWindowSizeLimits: window_id={} min={}x{} max={}x{}",
                    client_id, window_id, min_width, min_height, max_width, max_height
                );

                push_ipc_event(IpcEvent::SetWindowSizeLimits {
                    window_id,
                    min_width,
                    min_height,
                    max_width,
                    max_height,
                });
            }
            Ok(ClientMessageRef::ResizeWindow {
                window_id,
                width,
                height,
            }) => {
                // `resizable=false` disables user/compositor-driven interactive
                // resizing. The owning client must still be able to replace its
                // backing buffer when the compositor sends WINDOW_CONFIGURE, for
                // example after a display-size change.
                let (width, height) = (width.max(1), height.max(1));

                let buffer_size = (width as u64)
                    .saturating_mul(height as u64)
                    .saturating_mul(4);

                println!(
                    "[ClientThread {}] ResizeWindow: window_id={} {}x{} ({} bytes)",
                    client_id, window_id, width, height, buffer_size
                );

                match SharedMemory::create(buffer_size as usize, permissions::READ_WRITE) {
                    Ok(shm) => {
                        // Map for compositor
                        let mapper = match shm.as_handle().as_memory_mapping() {
                            Ok(m) => m,
                            Err(_) => {
                                println!(
                                    "[ClientThread {}] ResizeWindow: SHM mapping unsupported",
                                    client_id
                                );
                                continue;
                            }
                        };

                        let mapped_addr = match mapper.mmap(
                            0,
                            buffer_size as usize,
                            permissions::READ_WRITE,
                            mmap_flags::SHARED,
                            0,
                        ) {
                            Ok(a) => a,
                            Err(_) => {
                                println!("[ClientThread {}] ResizeWindow: mmap failed", client_id);
                                continue;
                            }
                        };

                        // Reply to client with WINDOW_RESIZED + SHM handle.
                        let payload =
                            protocol::payload_window_resized(window_id, buffer_size, width, height);
                        if let Err(e) = write_handle_frame_response(
                            &socket,
                            shm.as_handle(),
                            protocol::server_msg::WINDOW_RESIZED,
                            request_id,
                            &payload,
                        ) {
                            println!(
                                "[ClientThread {}] ResizeWindow: failed to send WINDOW_RESIZED: {:?}",
                                client_id, e
                            );
                            continue;
                        }

                        push_ipc_event(IpcEvent::ResizeWindow {
                            window_id,
                            width,
                            height,
                            shm: Some(shm),
                            shm_mapped_addr: Some(mapped_addr),
                            shm_size: buffer_size as usize,
                        });
                    }
                    Err(_) => {
                        println!(
                            "[ClientThread {}] ResizeWindow: failed to create SHM",
                            client_id
                        );
                    }
                }
            }
            Ok(ClientMessageRef::MinimizeWindow { window_id }) => {
                println!(
                    "[ClientThread {}] MinimizeWindow: window_id={}",
                    client_id, window_id
                );
                push_ipc_event(IpcEvent::MinimizeWindow { window_id });
            }
            Ok(ClientMessageRef::MaximizeWindow { window_id }) => {
                println!(
                    "[ClientThread {}] MaximizeWindow: window_id={}",
                    client_id, window_id
                );
                push_ipc_event(IpcEvent::MaximizeWindow { window_id });
            }
            Ok(ClientMessageRef::RestoreWindow { window_id }) => {
                println!(
                    "[ClientThread {}] RestoreWindow: window_id={}",
                    client_id, window_id
                );
                push_ipc_event(IpcEvent::RestoreWindow { window_id });
            }
            Ok(ClientMessageRef::FocusWindow { window_id }) => {
                println!(
                    "[ClientThread {}] FocusWindow: window_id={}",
                    client_id, window_id
                );
                push_ipc_event(IpcEvent::FocusWindow { window_id });
            }
            Ok(ClientMessageRef::SetWindowType {
                window_id,
                window_type,
            }) => {
                println!(
                    "[ClientThread {}] SetWindowType: window_id={} type={}",
                    client_id, window_id, window_type
                );
                push_ipc_event(IpcEvent::SetWindowType {
                    window_id,
                    window_type,
                });
            }
            Ok(ClientMessageRef::SetWindowOpacity { window_id, opacity }) => {
                println!(
                    "[ClientThread {}] SetWindowOpacity: window_id={} opacity={}",
                    client_id, window_id, opacity
                );
                push_ipc_event(IpcEvent::SetWindowOpacity { window_id, opacity });
            }
            Ok(ClientMessageRef::RegisterExtension { extension_name }) => {
                println!(
                    "[ClientThread {}] RegisterExtension: name={:?}",
                    client_id,
                    std::string::String::from_utf8_lossy(extension_name)
                );

                // Allocate extension ID
                let extension_id = next_window_id; // Reuse window ID counter for simplicity
                next_window_id = next_window_id.saturating_add(1);

                let name = std::string::String::from_utf8_lossy(extension_name).into_owned();
                push_ipc_event(IpcEvent::ExtensionRegistered {
                    client_id,
                    extension_id,
                    extension_name: name,
                });

                // Mark this client as an extension client
                is_extension_client = true;
                println!(
                    "[ClientThread {}] Registered as extension client with ID {}",
                    client_id, extension_id
                );

                // Send ExtensionRegistered response
                let payload = protocol::payload_extension_registered(extension_id);
                if let Err(e) = write_frame_response(
                    &mut stream_writer,
                    protocol::server_msg::EXTENSION_REGISTERED,
                    request_id,
                    &payload,
                ) {
                    println!(
                        "[ClientThread {}] Failed to send ExtensionRegistered: {:?}",
                        client_id, e
                    );
                }
            }
            Ok(ClientMessageRef::ExtensionCreateWindow {
                external_client_id,
                width,
                height,
            }) => {
                println!(
                    "[ClientThread {}] ExtensionCreateWindow: external_client_id={} {}x{}",
                    client_id, external_client_id, width, height
                );

                // Calculate buffer size
                let buffer_size = (width as u64)
                    .saturating_mul(height as u64)
                    .saturating_mul(4);

                let window_id = next_window_id;
                next_window_id = next_window_id.saturating_add(1);

                // Track mapping from window_id to external_client_id
                window_to_external_client.insert(window_id, external_client_id);

                // Create shared memory for this window
                match SharedMemory::create(buffer_size as usize, permissions::READ_WRITE) {
                    Ok(shm) => {
                        let shm_mapped_addr = match shm.as_handle().as_memory_mapping() {
                            Ok(mapper) => {
                                match mapper.mmap(
                                    0,
                                    buffer_size as usize,
                                    permissions::READ_WRITE,
                                    mmap_flags::SHARED,
                                    0,
                                ) {
                                    Ok(addr) => {
                                        // Zero-initialize
                                        unsafe {
                                            let ptr = addr as *mut u8;
                                            for i in 0..buffer_size as usize {
                                                *ptr.add(i) = 0;
                                            }
                                        }
                                        Some(addr)
                                    }
                                    Err(_) => None,
                                }
                            }
                            Err(_) => None,
                        };

                        let payload = protocol::payload_window_created(window_id, buffer_size);
                        if let Err(e) = write_handle_frame_response(
                            &socket,
                            shm.as_handle(),
                            protocol::server_msg::WINDOW_CREATED,
                            request_id,
                            &payload,
                        ) {
                            println!(
                                "[ClientThread {}] Failed to send atomic WINDOW_CREATED: {:?}",
                                client_id, e
                            );
                            continue;
                        }

                        // TODO: Map extension_id to client_id for routing
                        register_window(window_id, client_id);
                        managed_windows.push(window_id);

                        push_ipc_event(IpcEvent::ExtensionCreateWindow {
                            extension_id,
                            external_client_id,
                            window_id,
                            width,
                            height,
                            shm: Some(shm),
                            shm_mapped_addr,
                            shm_size: buffer_size as usize,
                        });
                    }
                    Err(e) => {
                        println!("[ClientThread {}] Failed to create SHM: {:?}", client_id, e);
                    }
                }
            }
            Ok(ClientMessageRef::ExtensionUpdateBuffer {
                external_client_id,
                window_id,
                x,
                y,
                width,
                height,
            }) => {
                if is_sws_debug_enabled() {
                    println!(
                        "[ClientThread {}] ExtensionUpdateBuffer: external_client_id={} window_id={} damage=[{},{} {}x{}]",
                        client_id, external_client_id, window_id, x, y, width, height
                    );
                }
                push_ipc_event(IpcEvent::ExtensionUpdateBuffer {
                    external_client_id,
                    window_id,
                    damage_x: x,
                    damage_y: y,
                    damage_width: width,
                    damage_height: height,
                });
            }
            Ok(ClientMessageRef::ExtensionAttachBuffer {
                external_client_id,
                window_id,
                width,
                height,
                offset,
                stride,
                format,
                shm_size,
            }) => {
                // println!(
                //     "[ClientThread {}] === EXTENSION_ATTACH_BUFFER ===",
                //     client_id
                // );
                // println!(
                //     "[ClientThread {}]   external_client_id={} window_id={} {}x{}",
                //     client_id, external_client_id, window_id, width, height
                // );
                // println!(
                //     "[ClientThread {}]   offset={} stride={} format={} shm_size={}",
                //     client_id, offset, stride, format, shm_size
                // );

                // The frame and its capability are one ordered socket record.
                // Presence was validated before protocol parsing.
                let Some(shm_handle) = received_handle else {
                    continue;
                };

                let shm_size_usize = shm_size as usize;
                // println!("[ClientThread {}]   Attempting to map handle directly (size={} bytes)...", client_id, shm_size_usize);

                // Try to map the handle directly without requiring SharedMemory type
                // This allows File handles from the Linux compatibility layer to work
                let result = if shm_size_usize > 0 {
                    match shm_handle.as_memory_mapping() {
                        Ok(mapper) => {
                            match mapper.mmap(
                                0,
                                shm_size_usize,
                                permissions::READ,
                                mmap_flags::SHARED,
                                0,
                            ) {
                                Ok(addr) => {
                                    // println!("[ClientThread {}]   Handle mapped at 0x{:x}", client_id, addr);
                                    Some(addr)
                                }
                                Err(e) => {
                                    println!(
                                        "[ClientThread {}] Failed to map handle: {:?}",
                                        client_id, e
                                    );
                                    None
                                }
                            }
                        }
                        Err(e) => {
                            println!(
                                "[ClientThread {}] Handle doesn't support memory_mapping: {:?}",
                                client_id, e
                            );
                            None
                        }
                    }
                } else {
                    None
                };

                match result {
                    Some(shm_mapped_addr) => {
                        // For zero-copy mode, we only need the mapped address
                        // The compositor will use it directly without needing SharedMemory ownership
                        push_ipc_event(IpcEvent::ExtensionAttachBuffer {
                            external_client_id,
                            window_id,
                            width,
                            height,
                            offset,
                            stride,
                            format,
                            shm: None, // No SharedMemory wrapper - using mapped address directly
                            shm_mapped_addr: Some(shm_mapped_addr),
                            shm_size: shm_size_usize,
                        });
                    }
                    None => {
                        push_ipc_event(IpcEvent::ExtensionAttachBuffer {
                            external_client_id,
                            window_id,
                            width,
                            height,
                            offset,
                            stride,
                            format,
                            shm: None,
                            shm_mapped_addr: None,
                            shm_size: 0,
                        });
                    }
                }
                // println!("[ClientThread {}] === EXTENSION_ATTACH_BUFFER COMPLETE ===", client_id);
            }
            Ok(ClientMessageRef::SetWindowHasAlphaContent {
                window_id,
                has_alpha,
            }) => {
                println!(
                    "[ClientThread {}] SetWindowHasAlphaContent: window_id={} has_alpha={}",
                    client_id, window_id, has_alpha
                );
                push_ipc_event(IpcEvent::SetWindowHasAlphaContent {
                    window_id,
                    has_alpha,
                });
            }
            Ok(ClientMessageRef::SetWindowMenuTitles {
                window_id,
                menu_titles,
            }) => {
                let menu_titles_str = String::from_utf8_lossy(menu_titles).into_owned();
                println!(
                    "[ClientThread {}] SetWindowMenuTitles: window_id={} titles_len={}",
                    client_id,
                    window_id,
                    menu_titles_str.len()
                );
                push_ipc_event(IpcEvent::SetWindowMenuTitles {
                    window_id,
                    menu_titles: menu_titles_str,
                });
            }
            Ok(ClientMessageRef::ActivateMenuItem {
                window_id,
                menu_item_id,
            }) => {
                let menu_item_id_str = String::from_utf8_lossy(menu_item_id).into_owned();
                println!(
                    "[ClientThread {}] ActivateMenuItem: window_id={} item_len={}",
                    client_id,
                    window_id,
                    menu_item_id_str.len()
                );
                push_ipc_event(IpcEvent::ActivateMenuItem {
                    window_id,
                    menu_item_id: menu_item_id_str,
                });
            }
            Ok(ClientMessageRef::SetWorkarea {
                x,
                y,
                width,
                height,
            }) => {
                println!(
                    "[ClientThread {}] SetWorkarea: x={}, y={}, width={}, height={}",
                    client_id, x, y, width, height
                );
                push_ipc_event(IpcEvent::SetWorkarea {
                    x,
                    y,
                    width,
                    height,
                });
            }
            Ok(ClientMessageRef::SetWindowResizable {
                window_id,
                resizable,
            }) => {
                println!(
                    "[ClientThread {}] SetWindowResizable: window_id={} resizable={}",
                    client_id, window_id, resizable
                );
                window_resizable.insert(window_id, resizable);
                push_ipc_event(IpcEvent::SetWindowResizable {
                    window_id,
                    resizable,
                });
            }
            Ok(ClientMessageRef::GetScreenSize {}) => {
                println!(
                    "[ClientThread {}] GetScreenSize: forwarding to compositor",
                    client_id
                );
                push_ipc_event(IpcEvent::GetScreenSize {
                    client_id,
                    request_id,
                });
            }
            Ok(ClientMessageRef::GetOutputScale {}) => {
                println!(
                    "[ClientThread {}] GetOutputScale: forwarding to compositor",
                    client_id
                );
                push_ipc_event(IpcEvent::GetOutputScale {
                    client_id,
                    request_id,
                });
            }
            Ok(ClientMessageRef::GetWindowList {}) => {
                println!(
                    "[ClientThread {}] GetWindowList: requesting window list",
                    client_id
                );
                push_ipc_event(IpcEvent::GetWindowList {
                    client_id,
                    request_id,
                });
            }
            Ok(ClientMessageRef::RequestActivationToken {
                source_window_id,
                target_app_id,
            }) => {
                push_ipc_event(IpcEvent::RequestActivationToken {
                    client_id,
                    request_id,
                    source_window_id,
                    target_app_id: target_app_id.to_vec(),
                });
            }
            Ok(ClientMessageRef::GetCapabilities {}) => {
                let payload = protocol::payload_capabilities(
                    protocol::SWS_PROTOCOL_VERSION,
                    sws_capabilities(),
                    compositor_epoch(),
                    compositor_backend_id(),
                );
                if let Err(error) = write_frame_response(
                    &mut stream_writer,
                    protocol::server_msg::CAPABILITIES,
                    request_id,
                    &payload,
                ) {
                    println!(
                        "[ClientThread {}] Failed to send SWS capabilities: {:?}",
                        client_id, error
                    );
                    break;
                }
            }
            Ok(ClientMessageRef::RegisterSgfxBuffer {
                window_id,
                buffer_id,
                generation,
                compositor_epoch: request_epoch,
                width,
                height,
            }) => {
                let Some(handle) = received_handle else {
                    let _ = write_protocol_error(
                        &mut stream_writer,
                        request_id,
                        protocol::error_codes::INVALID_SGFX_BUFFER,
                    );
                    continue;
                };
                if !managed_windows.contains(&window_id) {
                    let _ = write_protocol_error(
                        &mut stream_writer,
                        request_id,
                        protocol::error_codes::WINDOW_NOT_OWNED,
                    );
                    continue;
                }
                if request_epoch != compositor_epoch() {
                    let _ = write_protocol_error(
                        &mut stream_writer,
                        request_id,
                        protocol::error_codes::STALE_SGFX_GENERATION,
                    );
                    continue;
                }
                if !SGFX_SHARED_IMAGES_AVAILABLE.load(Ordering::Acquire) {
                    let _ = write_protocol_error(
                        &mut stream_writer,
                        request_id,
                        protocol::error_codes::SGFX_UNAVAILABLE,
                    );
                    continue;
                }
                push_ipc_event(IpcEvent::RegisterSgfxBuffer {
                    client_id,
                    request_id,
                    window_id,
                    buffer_id,
                    generation,
                    compositor_epoch: request_epoch,
                    width,
                    height,
                    handle,
                });
            }
            Ok(ClientMessageRef::CommitSgfxFrame {
                window_id,
                buffer_id,
                generation,
                compositor_epoch: request_epoch,
                commit_serial,
                damage_rects,
            }) => {
                let mut reject = |code| {
                    write_sgfx_frame_rejected(
                        &mut stream_writer,
                        window_id,
                        buffer_id,
                        generation,
                        request_epoch,
                        commit_serial,
                        code,
                    )
                };
                if header.flags != 0 || request_id != 0 {
                    let _ = reject(protocol::error_codes::INVALID_SGFX_BUFFER);
                    continue;
                }
                if !managed_windows.contains(&window_id) {
                    let _ = reject(protocol::error_codes::WINDOW_NOT_OWNED);
                    continue;
                }
                if request_epoch != compositor_epoch() {
                    let _ = reject(protocol::error_codes::STALE_SGFX_GENERATION);
                    continue;
                }
                if !SGFX_SHARED_IMAGES_AVAILABLE.load(Ordering::Acquire) {
                    let _ = reject(protocol::error_codes::SGFX_UNAVAILABLE);
                    continue;
                }
                let damage_rects = match protocol::parse_sgfx_damage_rects(damage_rects) {
                    Ok(rects) => rects,
                    Err(_) => {
                        let _ = reject(protocol::error_codes::INVALID_SGFX_BUFFER);
                        continue;
                    }
                };
                push_ipc_event(IpcEvent::CommitSgfxFrame {
                    client_id,
                    window_id,
                    buffer_id,
                    generation,
                    compositor_epoch: request_epoch,
                    commit_serial,
                    damage_rects,
                });
            }
            Ok(ClientMessageRef::DestroySgfxBuffer {
                window_id,
                buffer_id,
                generation,
                compositor_epoch: request_epoch,
            }) => {
                if !managed_windows.contains(&window_id) {
                    let _ = write_protocol_error(
                        &mut stream_writer,
                        request_id,
                        protocol::error_codes::WINDOW_NOT_OWNED,
                    );
                    continue;
                }
                if request_epoch != compositor_epoch() {
                    let _ = write_protocol_error(
                        &mut stream_writer,
                        request_id,
                        protocol::error_codes::STALE_SGFX_GENERATION,
                    );
                    continue;
                }
                if !SGFX_SHARED_IMAGES_AVAILABLE.load(Ordering::Acquire) {
                    let _ = write_protocol_error(
                        &mut stream_writer,
                        request_id,
                        protocol::error_codes::SGFX_UNAVAILABLE,
                    );
                    continue;
                }
                push_ipc_event(IpcEvent::DestroySgfxBuffer {
                    client_id,
                    request_id,
                    window_id,
                    buffer_id,
                    generation,
                    compositor_epoch: request_epoch,
                });
            }
            Ok(ClientMessageRef::TextInputCreate { window_id, seat_id }) => {
                if !managed_windows.iter().any(|id| *id == window_id) {
                    continue;
                }
                let context = create_text_input_context(client_id, window_id, seat_id);
                let payload =
                    protocol::payload_text_input_created(context.context_id, context.serial);
                if let Err(e) = write_frame_response(
                    &mut stream_writer,
                    protocol::server_msg::TEXT_INPUT_CREATED,
                    request_id,
                    &payload,
                ) {
                    println!(
                        "[ClientThread {}] Failed to send TEXT_INPUT_CREATED: {:?}",
                        client_id, e
                    );
                    break;
                }
            }
            Ok(ClientMessageRef::TextInputDestroy { context_id }) => {
                destroy_text_input_context(client_id, context_id);
            }
            Ok(ClientMessageRef::TextInputEnable { context_id }) => {
                set_text_input_enabled(client_id, context_id, true);
            }
            Ok(ClientMessageRef::TextInputDisable { context_id }) => {
                set_text_input_enabled(client_id, context_id, false);
            }
            Ok(ClientMessageRef::TextInputSetCursorRect {
                context_id,
                x,
                y,
                width,
                height,
            }) => {
                update_text_input_cursor_rect(client_id, context_id, x, y, width, height);
            }
            Ok(ClientMessageRef::TextInputSetSurroundingText {
                context_id,
                cursor_byte,
                anchor_byte,
                text,
            }) => {
                update_text_input_surrounding_text(
                    client_id,
                    context_id,
                    cursor_byte,
                    anchor_byte,
                    text,
                );
            }
            Ok(ClientMessageRef::TextInputSetContentType {
                context_id,
                hint,
                purpose,
            }) => {
                update_text_input_content_type(client_id, context_id, hint, purpose);
            }
            Ok(ClientMessageRef::TextInputSetTextChangeCause { context_id, cause }) => {
                update_text_input_change_cause(client_id, context_id, cause);
            }
            Ok(ClientMessageRef::TextInputCommitState { context_id, serial }) => {
                commit_text_input_state(client_id, context_id, serial);
            }
            Ok(ClientMessageRef::ImeGetMethods {}) => {
                let payload = input_methods_payload();
                if let Err(e) = write_frame_response(
                    &mut stream_writer,
                    protocol::server_msg::IME_METHODS,
                    request_id,
                    &payload,
                ) {
                    println!(
                        "[ClientThread {}] Failed to send IME_METHODS: {:?}",
                        client_id, e
                    );
                    break;
                }
            }
            Ok(ClientMessageRef::ImeGetActive {}) => {
                let payload = active_input_method_payload();
                if let Err(e) = write_frame_response(
                    &mut stream_writer,
                    protocol::server_msg::IME_ACTIVE,
                    request_id,
                    &payload,
                ) {
                    println!(
                        "[ClientThread {}] Failed to send IME_ACTIVE: {:?}",
                        client_id, e
                    );
                    break;
                }
            }
            Ok(ClientMessageRef::ImeRegister { name, capabilities }) => {
                let service = register_input_method(client_id, name, capabilities);
                println!(
                    "[ClientThread {}] Registered IME #{} name={} capabilities=0x{:x}",
                    client_id, service.ime_id, service.name, service.capabilities
                );
                let payload = protocol::payload_ime_registered(service.ime_id);
                if let Err(e) = write_frame_response(
                    &mut stream_writer,
                    protocol::server_msg::IME_REGISTERED,
                    request_id,
                    &payload,
                ) {
                    println!(
                        "[ClientThread {}] Failed to send IME_REGISTERED: {:?}",
                        client_id, e
                    );
                    break;
                }
            }
            Ok(ClientMessageRef::ImeSetActive { ime_id }) => {
                set_active_input_method(ime_id);
            }
            Ok(ClientMessageRef::ImeKeyHandled {
                key_serial,
                handled,
            }) => {
                handle_ime_key_handled(key_serial, handled);
            }
            Ok(ClientMessageRef::ImeSetPreedit {
                context_id,
                cursor_byte,
                anchor_byte,
                text,
                spans,
            }) => {
                let clearing_preedit = text.is_empty() && spans.is_empty();
                if !client_can_mutate_ime_context(client_id, context_id)
                    && !(clearing_preedit
                        && client_is_active_input_method(client_id)
                        && text_input_context(context_id).is_some())
                {
                    continue;
                }
                forward_ime_preedit(context_id, cursor_byte, anchor_byte, text, spans);
            }
            Ok(ClientMessageRef::ImeCommitText { context_id, text }) => {
                if !client_can_mutate_ime_context(client_id, context_id) {
                    continue;
                }
                forward_ime_commit(context_id, text);
            }
            Ok(ClientMessageRef::ImeDeleteSurroundingText {
                context_id,
                before_bytes,
                after_bytes,
            }) => {
                if !client_can_mutate_ime_context(client_id, context_id) {
                    continue;
                }
                forward_ime_delete_surrounding_text(context_id, before_bytes, after_bytes);
            }
            Ok(ClientMessageRef::ImeSetStatus {
                context_id,
                state,
                mode_id,
                flags,
                mode_label,
            }) => {
                if !client_can_mutate_ime_context(client_id, context_id) {
                    continue;
                }
                forward_ime_status(context_id, state, mode_id, flags, mode_label);
            }
            Ok(ClientMessageRef::ImeSetPopupWindow {
                context_id,
                window_id,
                offset_x,
                offset_y,
                visible,
            }) => {
                if !client_is_active_input_method(client_id) {
                    println!(
                        "[ClientThread {}] Ignoring IME popup from non-active IME client",
                        client_id
                    );
                    continue;
                }
                if visible && !client_can_mutate_ime_context(client_id, context_id) {
                    continue;
                }
                if !managed_windows.iter().any(|id| *id == window_id) {
                    println!(
                        "[ClientThread {}] Ignoring IME popup for foreign window {}",
                        client_id, window_id
                    );
                    continue;
                }
                push_ipc_event(IpcEvent::ImeSetPopupWindow {
                    context_id,
                    window_id,
                    offset_x,
                    offset_y,
                    visible,
                });
            }
            Ok(ClientMessageRef::ImeGrabKeyboard { context_id }) => {
                set_ime_keyboard_grabbed(client_id, context_id, true);
            }
            Ok(ClientMessageRef::ImeReleaseKeyboard { context_id }) => {
                set_ime_keyboard_grabbed(client_id, context_id, false);
            }
            Ok(_) => {
                // Ignore other messages for now
            }
            Err(e) => {
                println!(
                    "[ClientThread {}] Failed to parse message (type {}): {:?}",
                    client_id,
                    header.msg_type_u32(),
                    e
                );
            }
        }
        // sleep(std::time::Duration::from_millis(16));
        yield_now();
    }

    // Cleanup: ensure per-window routing queues don't leak when the client disappears.
    // Also notify the compositor so orphaned windows don't stick around. A disconnected
    // client cannot receive WINDOW_DESTROYED, so use a separate event that only updates
    // server-side state and other live clients.
    let disconnected_windows = core::mem::take(&mut managed_windows);
    for &window_id in &disconnected_windows {
        cleanup_window_state(window_id);
        window_resizable.remove(&window_id);
        if let Some(external_client_id) = window_to_external_client.remove(&window_id) {
            cleanup_extension_input_events(extension_id, external_client_id);
        }
    }

    // Unregister client from broadcast messages before publishing the disconnect event;
    // broadcasts produced by window removal must not recreate this dead client's queue.
    {
        let mut pending = PENDING_CLIENT_RESPONSES.lock();
        pending.remove(&client_id);
        println!(
            "[ClientThread {}] Unregistered client {} from broadcast messages",
            client_id, client_id
        );
    }
    CLIENT_WAKES.lock().remove(&client_id);
    cleanup_text_input_contexts_for_client(client_id);
    cleanup_input_methods_for_client(client_id);

    if !disconnected_windows.is_empty() {
        println!(
            "[ClientThread {}] Scheduling cleanup for {} disconnected windows",
            client_id,
            disconnected_windows.len()
        );
        push_ipc_event(IpcEvent::ClientDisconnected {
            client_id,
            window_ids: disconnected_windows,
        });
    }

    println!("[ClientThread {}] Exiting", client_id);
}

/// IPC Events that can be sent from clients
#[derive(Debug)]
pub enum IpcEvent {
    /// Client requested to create a window
    CreateWindow {
        client_id: usize,
        app_id: Vec<u8>,
        window_id: u32,
        width: u32,
        height: u32,
        window_type: u32, // Window type (0=Normal, 1=AlwaysOnTop, 2=Taskbar, 3=Desktop)
        resizable: bool,
        focus_on_create: bool,
        active_on_focus: bool,
        initial_position: protocol::WindowPlacement,
        activation_token: Option<Vec<u8>>,
        /// Shared memory for the window buffer (server-allocated)
        shm: Option<SharedMemory>,
        shm_mapped_addr: Option<usize>,
        /// Size of the SHM mapping in bytes.
        shm_size: usize,
    },
    /// Client requested to destroy a window
    DestroyWindow {
        client_id: usize,
        window_id: u32,
    },
    /// Client connection was lost; all windows owned by that client must be removed.
    ClientDisconnected {
        client_id: usize,
        window_ids: Vec<u32>,
    },
    /// Client updated their window buffer (damage region only)
    BufferUpdated {
        window_id: u32,
        damage_x: i32,
        damage_y: i32,
        damage_width: u32,
        damage_height: u32,
    },
    /// Import one transferred shared SGFX image into the compositor context.
    RegisterSgfxBuffer {
        client_id: usize,
        request_id: u8,
        window_id: u32,
        buffer_id: u32,
        generation: u32,
        compositor_epoch: u32,
        width: u32,
        height: u32,
        handle: Handle,
    },
    /// Atomically publish one registered SGFX buffer and its damage list.
    CommitSgfxFrame {
        client_id: usize,
        window_id: u32,
        buffer_id: u32,
        generation: u32,
        compositor_epoch: u32,
        commit_serial: u64,
        damage_rects: Vec<protocol::SgfxDamageRect>,
    },
    /// Drop one released shared SGFX buffer capability.
    DestroySgfxBuffer {
        client_id: usize,
        request_id: u8,
        window_id: u32,
        buffer_id: u32,
        generation: u32,
        compositor_epoch: u32,
    },
    /// Client requested window move
    RequestMove {
        window_id: u32,
    },
    /// Client moved window
    MoveWindow {
        window_id: u32,
        x: i32,
        y: i32,
    },

    /// Set (or clear) parent window relationship
    ///
    /// `parent_id == 0` means "no parent".
    SetWindowParent {
        window_id: u32,
        parent_id: u32,
    },

    /// Set transient behavior flags for a window (bitset).
    SetWindowTransientFlags {
        window_id: u32,
        flags: u32,
    },

    /// Set min/max size constraints for a window.
    SetWindowSizeLimits {
        window_id: u32,
        min_width: u32,
        min_height: u32,
        max_width: u32,
        max_height: u32,
    },

    /// Resize a window and replace its SHM buffer.
    ResizeWindow {
        window_id: u32,
        width: u32,
        height: u32,
        shm: Option<SharedMemory>,
        shm_mapped_addr: Option<usize>,
        /// Size of the SHM mapping in bytes.
        shm_size: usize,
    },

    /// Minimize a window
    MinimizeWindow {
        window_id: u32,
    },

    /// Maximize a window
    MaximizeWindow {
        window_id: u32,
    },

    /// Restore a window from minimized or maximized state
    RestoreWindow {
        window_id: u32,
    },

    /// Focus and raise a window
    FocusWindow {
        window_id: u32,
    },

    /// Set window type for Z-order management
    SetWindowType {
        window_id: u32,
        window_type: u32,
    },

    /// Set window opacity
    SetWindowOpacity {
        window_id: u32,
        opacity: u8,
    },

    TextInputContextUpdated {
        context_id: u32,
    },

    ImeSetPopupWindow {
        context_id: u32,
        window_id: u32,
        offset_x: i32,
        offset_y: i32,
        visible: bool,
    },

    // Extension API events
    /// Extension registered
    ExtensionRegistered {
        client_id: usize,
        extension_id: u32,
        extension_name: std::string::String,
    },

    /// Extension created a window for external client
    ExtensionCreateWindow {
        extension_id: u32,
        external_client_id: u32,
        window_id: u32,
        width: u32,
        height: u32,
        shm: Option<SharedMemory>,
        shm_mapped_addr: Option<usize>,
        shm_size: usize,
    },

    /// Extension updated buffer for external client
    ExtensionUpdateBuffer {
        external_client_id: u32,
        window_id: u32,
        damage_x: i32,
        damage_y: i32,
        damage_width: u32,
        damage_height: u32,
    },

    /// Extension attached a shared-memory buffer for external client
    ExtensionAttachBuffer {
        external_client_id: u32,
        window_id: u32,
        width: u32,
        height: u32,
        offset: i32,
        stride: i32,
        format: u32,
        shm: Option<SharedMemory>,
        shm_mapped_addr: Option<usize>,
        shm_size: usize,
    },

    /// Set whether window content contains alpha channel
    SetWindowHasAlphaContent {
        window_id: u32,
        has_alpha: bool,
    },
    SetWindowMenuTitles {
        window_id: u32,
        menu_titles: String,
    },
    ActivateMenuItem {
        window_id: u32,
        menu_item_id: String,
    },

    /// Set the workarea (usable screen area) for the window manager
    SetWorkarea {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },

    /// Set whether a window can be resized by the user via interactive resize
    SetWindowResizable {
        window_id: u32,
        resizable: bool,
    },

    /// Get the screen size
    GetScreenSize {
        client_id: usize,
        request_id: u8,
    },

    /// Get the output scale
    GetOutputScale {
        client_id: usize,
        request_id: u8,
    },

    /// Get list of all windows
    GetWindowList {
        client_id: usize,
        request_id: u8,
    },

    /// Request a one-shot token for activating a newly launched application.
    RequestActivationToken {
        client_id: usize,
        request_id: u8,
        source_window_id: u32,
        target_app_id: Vec<u8>,
    },
}
