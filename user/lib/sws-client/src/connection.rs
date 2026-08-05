//! Connection management for SWS client

use crate::TransientFlags;
use crate::WindowSizeLimits;
use crate::error::Error;
use crate::event::{Event, ImeContextState, InputEvent};
use crate::os::{Arc, BTreeMap, Handle, Mutex, SharedMemory, Socket, String, Vec, mutex_lock};
use crate::os::{
    socket_flush, socket_read, socket_recv_handle_and_data, socket_send_handle_and_data,
    socket_write,
};
use crate::surface::Surface;
use sws_protocol::{self as protocol, ServerMessage};

const HANDLE_RECORD_CAPACITY: usize = protocol::MessageHeader::SIZE + protocol::MAX_PAYLOAD_SIZE;
const MAX_DISPATCH_FRAMES: usize = 64;
const MAX_STREAM_WRITE_CHUNK: usize = 16 * 1024;

/// Window list entry
#[derive(Debug, Clone)]
pub struct WindowListEntry {
    pub window_id: u32,
    pub app_id: String,
    pub title: String,
    pub window_type: u32,
    pub visible: bool,
    pub focused: bool,
    pub minimized: bool,
}

/// Registered input method information.
#[derive(Debug, Clone)]
pub struct InputMethodInfo {
    /// Server-assigned input method ID.
    pub ime_id: u32,
    /// Human-readable input method name.
    pub name: String,
    /// Capability flags advertised by the input method.
    pub capabilities: u32,
    /// Whether this input method is currently active.
    pub active: bool,
}

/// Capabilities negotiated with the connected SWS instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// Negotiated SWS protocol version.
    pub protocol_version: u32,
    /// Optional capability bits from [`sws_protocol::capabilities`].
    pub capabilities: u64,
    /// Current SWS shared-image compositor epoch.
    pub compositor_epoch: u32,
    /// Active compositor backend from [`sws_protocol::compositor_backends`].
    pub compositor_backend: u32,
}

/// Stable identity for one registered shared SGFX buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SgfxBufferIdentity {
    /// Target SWS window.
    pub window_id: u32,
    /// Client-assigned buffer slot identifier.
    pub buffer_id: u32,
    /// Window-buffer generation.
    pub generation: u32,
    /// SWS compositor epoch that imported the buffer.
    pub compositor_epoch: u32,
}

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

    fn poll(
        &mut self,
        socket: &mut Socket,
        out_payload: &mut Vec<u8>,
    ) -> Result<Option<protocol::MessageHeader>, Error> {
        loop {
            if !self.header_parsed {
                match socket_read(socket, &mut self.header[self.header_filled..]) {
                    Ok(n) => {
                        self.header_filled += n;
                        if self.header_filled < self.header.len() {
                            continue;
                        }
                        let header = protocol::MessageHeader::from_le_bytes(self.header);
                        let payload_len = header.payload_size as usize;
                        if payload_len > protocol::MAX_PAYLOAD_SIZE {
                            return Err(Error::ProtocolError);
                        }
                        self.header_value = header;
                        self.payload_len = payload_len;
                        self.payload.clear();
                        if payload_len > 0 {
                            self.payload.resize(payload_len, 0);
                        }
                        self.payload_filled = 0;
                        self.header_parsed = true;
                        if payload_len == 0 {
                            out_payload.clear();
                            let header = self.header_value;
                            self.reset();
                            return Ok(Some(header));
                        }
                    }
                    Err(Error::WouldBlock) => return Ok(None),
                    Err(error) => return Err(error),
                }
            }

            if self.header_parsed {
                match socket_read(socket, &mut self.payload[self.payload_filled..]) {
                    Ok(n) => {
                        self.payload_filled += n;
                        if self.payload_filled < self.payload_len {
                            continue;
                        }
                        out_payload.clear();
                        out_payload.extend_from_slice(&self.payload);
                        let header = self.header_value;
                        self.reset();
                        return Ok(Some(header));
                    }
                    Err(Error::WouldBlock) => return Ok(None),
                    Err(error) => return Err(error),
                }
            }
        }
    }
}

/// Identifier for one in-flight request on an SWS connection.
///
/// Tokens are created by [`Connection::send_request`] and consumed by
/// [`Connection::wait_response`]. Request identifier zero is reserved for
/// unsolicited server events.
pub struct RequestToken {
    request_id: u8,
    transport: Option<Arc<Mutex<TransportState>>>,
}

impl RequestToken {
    /// Return the protocol request identifier carried by this token.
    ///
    /// # Returns
    ///
    /// A non-zero per-connection request identifier.
    pub fn request_id(&self) -> u8 {
        self.request_id
    }
}

impl core::fmt::Debug for RequestToken {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RequestToken")
            .field("request_id", &self.request_id)
            .finish()
    }
}

impl Drop for RequestToken {
    fn drop(&mut self) {
        if let Some(transport) = self.transport.take() {
            mutex_lock(&transport).cancel_response(self.request_id);
        }
    }
}

/// Fully received response routed to a request token.
///
/// The payload is owned so another thread can continue receiving frames while
/// the caller parses this response. Handle-bearing responses carry their
/// transferred handle in the same envelope.
pub struct Response {
    request_id: u8,
    message: ServerMessage,
    payload: Vec<u8>,
    handle: Option<Handle>,
}

#[derive(Clone, Copy)]
enum EventFilter {
    Window(u32),
    Sgfx(u32),
}

struct EventMailbox {
    filter: EventFilter,
    events: Vec<Event>,
    head: usize,
}

impl EventMailbox {
    fn new(filter: EventFilter) -> Self {
        Self {
            filter,
            events: Vec::new(),
            head: 0,
        }
    }

    fn push(&mut self, event: Event) {
        self.events.push(event);
    }

    fn poll(&mut self) -> Option<Event> {
        if self.head >= self.events.len() {
            self.events.clear();
            self.head = 0;
            return None;
        }
        let event = self.events[self.head].clone();
        self.head += 1;
        if self.head >= self.events.len() {
            self.events.clear();
            self.head = 0;
        }
        Some(event)
    }

    fn drain(&mut self) -> Vec<Event> {
        if self.head == 0 {
            core::mem::take(&mut self.events)
        } else {
            let events = self.events[self.head..].to_vec();
            self.events.clear();
            self.head = 0;
            events
        }
    }
}

/// Independent event mailbox for one window or its SGFX buffer lifecycle.
///
/// Every subscription receives its own copy of broadcast events. Dropping the
/// receiver unregisters it without affecting other consumers.
pub struct EventReceiver {
    subscriber_id: u64,
    transport: Arc<Mutex<TransportState>>,
    surfaces: Arc<Mutex<BTreeMap<u32, Surface>>>,
}

impl EventReceiver {
    /// Pop the next event routed to this subscription.
    ///
    /// # Returns
    ///
    /// The next event, or `None` when this mailbox is empty.
    pub fn poll_event(&self) -> Option<Event> {
        let event = mutex_lock(&self.transport)
            .subscribers
            .get_mut(&self.subscriber_id)
            .and_then(EventMailbox::poll);
        if let Some(Event::SurfaceDestroyed { surface_id }) = event.as_ref() {
            mutex_lock(&self.surfaces).remove(surface_id);
        }
        event
    }

    /// Drain all events currently routed to this subscription.
    ///
    /// # Returns
    ///
    /// Events in server receive order.
    pub fn drain_events(&self) -> Vec<Event> {
        let events = mutex_lock(&self.transport)
            .subscribers
            .get_mut(&self.subscriber_id)
            .map(EventMailbox::drain)
            .unwrap_or_default();
        let mut surfaces = mutex_lock(&self.surfaces);
        for event in &events {
            if let Event::SurfaceDestroyed { surface_id } = event {
                surfaces.remove(surface_id);
            }
        }
        events
    }

    /// Return whether this subscription currently has queued events.
    ///
    /// # Returns
    ///
    /// `true` when [`Self::poll_event`] can return an event immediately.
    pub fn has_events(&self) -> bool {
        mutex_lock(&self.transport)
            .subscribers
            .get(&self.subscriber_id)
            .map_or(false, |mailbox| mailbox.head < mailbox.events.len())
    }
}

impl Drop for EventReceiver {
    fn drop(&mut self) {
        mutex_lock(&self.transport)
            .subscribers
            .remove(&self.subscriber_id);
    }
}

impl Response {
    /// Return the request identifier copied from the response header.
    ///
    /// # Returns
    ///
    /// The non-zero request identifier used to route this envelope.
    pub fn request_id(&self) -> u8 {
        self.request_id
    }

    /// Return the parsed server message.
    ///
    /// # Returns
    ///
    /// The parsed message corresponding to [`Self::payload`].
    pub fn message(&self) -> ServerMessage {
        self.message
    }

    /// Return the exact response payload bytes.
    ///
    /// # Returns
    ///
    /// The owned protocol payload as a borrowed slice.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Take the transferred handle, if this response carries one.
    ///
    /// # Returns
    ///
    /// The response handle, or `None` for an ordinary response.
    pub fn take_handle(&mut self) -> Option<Handle> {
        self.handle.take()
    }

    /// Consume the envelope and return all of its parts.
    ///
    /// # Returns
    ///
    /// The request identifier, parsed message, raw payload, and optional
    /// transferred handle.
    pub fn into_parts(self) -> (u8, ServerMessage, Vec<u8>, Option<Handle>) {
        (self.request_id, self.message, self.payload, self.handle)
    }
}

enum PendingResponse {
    Awaiting,
    Cancelled,
    Ready(Response),
    Failed(Error),
}

struct PumpResult {
    progressed: bool,
    event_queued: bool,
}

impl PumpResult {
    const IDLE: Self = Self {
        progressed: false,
        event_queued: false,
    };
}

struct TransportState {
    socket: Socket,
    frame_reader: FrameReader,
    stream_payload: Vec<u8>,
    handle_record: Vec<u8>,
    responses: BTreeMap<u8, PendingResponse>,
    next_request_id: u8,
    pending_events: Vec<Event>,
    pending_head: usize,
    subscribers: BTreeMap<u64, EventMailbox>,
    next_subscriber_id: u64,
    text_input_windows: BTreeMap<u32, u32>,
    terminal_error: Option<Error>,
}

impl TransportState {
    fn new(socket: Socket) -> Self {
        Self {
            socket,
            frame_reader: FrameReader::new(),
            stream_payload: Vec::new(),
            handle_record: Vec::new(),
            responses: BTreeMap::new(),
            next_request_id: 1,
            pending_events: Vec::new(),
            pending_head: 0,
            subscribers: BTreeMap::new(),
            next_subscriber_id: 1,
            text_input_windows: BTreeMap::new(),
            terminal_error: None,
        }
    }

    fn pump_write_backpressure(&mut self) -> Result<(), Error> {
        match self.pump_once() {
            Ok(_) => {
                crate::os::yield_now();
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn write_all_pumping(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let mut written = 0usize;
        while written < bytes.len() {
            let chunk_end = written
                .saturating_add(MAX_STREAM_WRITE_CHUNK)
                .min(bytes.len());
            match socket_write(&mut self.socket, &bytes[written..chunk_end]) {
                Ok(0) => return Err(Error::SendFailed),
                Ok(count) => written += count,
                Err(Error::WouldBlock) => self.pump_write_backpressure()?,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn write_frame_routed(
        &mut self,
        msg_type: u32,
        flags: u8,
        request_id: u8,
        payload: &[u8],
    ) -> Result<(), Error> {
        let header = protocol::MessageHeader::with_routing(
            msg_type,
            flags,
            request_id,
            payload.len() as u32,
        );
        self.write_all_pumping(&header.to_le_bytes())?;
        if !payload.is_empty() {
            self.write_all_pumping(payload)?;
        }
        socket_flush(&mut self.socket)
    }

    fn send_handle_record_pumping(&mut self, handle: &Handle, record: &[u8]) -> Result<(), Error> {
        loop {
            match socket_send_handle_and_data(&self.socket, handle, record) {
                Ok(()) => return Ok(()),
                Err(Error::WouldBlock) => self.pump_write_backpressure()?,
                Err(error) => return Err(error),
            }
        }
    }

    fn fail_pending(&mut self, error: Error) {
        if self.terminal_error.is_none() {
            self.terminal_error = Some(error);
        }
        self.responses.retain(|_, response| match response {
            PendingResponse::Cancelled => false,
            PendingResponse::Awaiting => {
                *response = PendingResponse::Failed(error);
                true
            }
            PendingResponse::Ready(_) | PendingResponse::Failed(_) => true,
        });
    }

    fn alloc_request_id(&mut self) -> Result<u8, Error> {
        if let Some(error) = self.terminal_error {
            return Err(error);
        }

        for _ in 0..u8::MAX {
            let request_id = self.next_request_id.max(1);
            self.next_request_id = request_id.wrapping_add(1).max(1);
            if !self.responses.contains_key(&request_id) {
                return Ok(request_id);
            }
        }
        Err(Error::RequestIdExhausted)
    }

    fn register_request(&mut self) -> Result<u8, Error> {
        let request_id = self.alloc_request_id()?;
        self.responses.insert(request_id, PendingResponse::Awaiting);
        Ok(request_id)
    }

    fn send_request(
        &mut self,
        msg_type: u32,
        payload: &[u8],
        handle: Option<&Handle>,
    ) -> Result<u8, Error> {
        if payload.len() > protocol::MAX_PAYLOAD_SIZE {
            return Err(Error::ProtocolError);
        }
        if handle.is_some()
            && protocol::MessageHeader::SIZE + payload.len() > HANDLE_RECORD_CAPACITY
        {
            return Err(Error::ProtocolError);
        }

        let request_id = self.register_request()?;
        let send_result = if let Some(handle) = handle {
            let header =
                protocol::MessageHeader::request(msg_type, request_id, payload.len() as u32);
            let mut record = Vec::with_capacity(protocol::MessageHeader::SIZE + payload.len());
            record.extend_from_slice(&header.to_le_bytes());
            record.extend_from_slice(payload);
            self.send_handle_record_pumping(handle, &record)
        } else {
            self.write_frame_routed(msg_type, 0, request_id, payload)
                .map_err(|_| Error::SendFailed)
        };

        if let Err(error) = send_result {
            self.responses.remove(&request_id);
            self.fail_pending(error);
            return Err(error);
        }
        Ok(request_id)
    }

    fn send_message(&mut self, msg_type: u32, payload: &[u8]) -> Result<(), Error> {
        if let Some(error) = self.terminal_error {
            return Err(error);
        }
        self.write_frame_routed(msg_type, 0, 0, payload)
            .map_err(|error| {
                self.fail_pending(error);
                Error::SendFailed
            })
    }

    fn take_response(&mut self, request_id: u8) -> Option<Result<Response, Error>> {
        match self.responses.remove(&request_id) {
            Some(PendingResponse::Ready(response)) => Some(Ok(response)),
            Some(PendingResponse::Failed(error)) => Some(Err(error)),
            Some(PendingResponse::Awaiting) => {
                self.responses.insert(request_id, PendingResponse::Awaiting);
                None
            }
            Some(PendingResponse::Cancelled) => {
                self.responses
                    .insert(request_id, PendingResponse::Cancelled);
                Some(Err(Error::InvalidRequest))
            }
            None => Some(Err(Error::InvalidRequest)),
        }
    }

    fn cancel_response(&mut self, request_id: u8) {
        let remove = match self.responses.get_mut(&request_id) {
            Some(response @ PendingResponse::Awaiting) => {
                *response = PendingResponse::Cancelled;
                false
            }
            Some(PendingResponse::Cancelled) => false,
            Some(PendingResponse::Ready(_)) | Some(PendingResponse::Failed(_)) => true,
            None => false,
        };
        if remove {
            self.responses.remove(&request_id);
        }
    }

    fn response_requires_handle(message: ServerMessage) -> bool {
        matches!(
            message,
            ServerMessage::WindowCreated { .. } | ServerMessage::WindowResized { .. }
        )
    }

    fn route_response(&mut self, request_id: u8, response: Response) -> Result<(), Error> {
        if request_id == 0 {
            self.fail_pending(Error::ProtocolError);
            return Err(Error::ProtocolError);
        }
        match self.responses.get(&request_id) {
            Some(PendingResponse::Awaiting) => {
                self.responses
                    .insert(request_id, PendingResponse::Ready(response));
                Ok(())
            }
            Some(PendingResponse::Cancelled) => {
                self.responses.remove(&request_id);
                Ok(())
            }
            Some(PendingResponse::Ready(_)) | Some(PendingResponse::Failed(_)) | None => {
                self.fail_pending(Error::ProtocolError);
                Err(Error::ProtocolError)
            }
        }
    }

    fn route_frame(
        &mut self,
        header: protocol::MessageHeader,
        payload: &[u8],
        handle: Option<Handle>,
    ) -> Result<PumpResult, Error> {
        let message = protocol::parse_server_message(header.msg_type_u32(), payload)
            .map_err(|_| Error::InvalidResponse)?;

        if header.is_response() {
            if Self::response_requires_handle(message) != handle.is_some() {
                self.fail_pending(Error::ProtocolError);
                return Err(Error::ProtocolError);
            }
            self.route_response(
                header.request_id,
                Response {
                    request_id: header.request_id,
                    message,
                    payload: payload.to_vec(),
                    handle,
                },
            )?;
            return Ok(PumpResult {
                progressed: true,
                event_queued: false,
            });
        }

        if header.request_id != 0 || handle.is_some() {
            self.fail_pending(Error::ProtocolError);
            return Err(Error::ProtocolError);
        }
        let event_queued = self.queue_async_message(message);
        Ok(PumpResult {
            progressed: true,
            event_queued,
        })
    }

    fn poll_handle_record(
        &mut self,
    ) -> Result<Option<(protocol::MessageHeader, Vec<u8>, Handle)>, Error> {
        let mut probe = [];
        let required_len = match socket_recv_handle_and_data(&self.socket, &mut probe) {
            Err(Error::ReceiveBufferTooSmall { required_len }) => required_len,
            Err(Error::WouldBlock) => return Ok(None),
            Ok(_) => return Err(Error::ProtocolError),
            Err(error) => return Err(error),
        };

        if !(protocol::MessageHeader::SIZE..=HANDLE_RECORD_CAPACITY).contains(&required_len) {
            return Err(Error::ProtocolError);
        }
        self.handle_record.clear();
        self.handle_record
            .try_reserve_exact(required_len)
            .map_err(|_| Error::ReceiveFailed)?;
        self.handle_record.resize(required_len, 0);

        let (handle, bytes_read) =
            socket_recv_handle_and_data(&self.socket, &mut self.handle_record)?;

        if bytes_read != required_len {
            return Err(Error::ProtocolError);
        }
        let mut header_bytes = [0u8; protocol::MessageHeader::SIZE];
        header_bytes.copy_from_slice(&self.handle_record[..protocol::MessageHeader::SIZE]);
        let header = protocol::MessageHeader::from_le_bytes(header_bytes);
        let payload_len = header.payload_size as usize;
        if payload_len > protocol::MAX_PAYLOAD_SIZE
            || protocol::MessageHeader::SIZE + payload_len != bytes_read
        {
            return Err(Error::ProtocolError);
        }
        let payload = self.handle_record
            [protocol::MessageHeader::SIZE..protocol::MessageHeader::SIZE + payload_len]
            .to_vec();
        Ok(Some((header, payload, handle)))
    }

    fn pump_once(&mut self) -> Result<PumpResult, Error> {
        if let Some(error) = self.terminal_error {
            return Err(error);
        }

        match self.poll_handle_record() {
            Ok(Some((header, payload, handle))) => {
                let result = self.route_frame(header, &payload, Some(handle));
                if let Err(error) = &result {
                    self.fail_pending(*error);
                }
                return result;
            }
            Ok(None) => {}
            Err(error) => {
                self.fail_pending(error);
                return Err(error);
            }
        }

        match self
            .frame_reader
            .poll(&mut self.socket, &mut self.stream_payload)
        {
            Ok(Some(header)) => {
                let payload = core::mem::take(&mut self.stream_payload);
                let result = self.route_frame(header, &payload, None);
                self.stream_payload = payload;
                if let Err(error) = &result {
                    self.fail_pending(*error);
                }
                result
            }
            Ok(None) => Ok(PumpResult::IDLE),
            Err(error) => {
                self.fail_pending(error);
                Err(error)
            }
        }
    }

    fn poll_event(&mut self) -> Option<Event> {
        if self.pending_head >= self.pending_events.len() {
            self.pending_events.clear();
            self.pending_head = 0;
            return None;
        }
        let event = self.pending_events[self.pending_head].clone();
        self.pending_head += 1;
        if self.pending_head >= self.pending_events.len() {
            self.pending_events.clear();
            self.pending_head = 0;
        }
        Some(event)
    }

    fn drain_events(&mut self) -> Vec<Event> {
        if self.pending_head == 0 {
            core::mem::take(&mut self.pending_events)
        } else {
            let events = self.pending_events[self.pending_head..].to_vec();
            self.pending_events.clear();
            self.pending_head = 0;
            events
        }
    }

    fn register_subscriber(&mut self, filter: EventFilter) -> u64 {
        loop {
            let subscriber_id = self.next_subscriber_id.max(1);
            self.next_subscriber_id = subscriber_id.wrapping_add(1).max(1);
            if !self.subscribers.contains_key(&subscriber_id) {
                let mut mailbox = EventMailbox::new(filter);
                let pending_events = core::mem::take(&mut self.pending_events);
                let mut unmatched_events = Vec::new();
                for event in pending_events.into_iter().skip(self.pending_head) {
                    let target_window = self.event_window_id(&event);
                    if Self::event_matches(filter, target_window, &event) {
                        mailbox.push(event);
                    } else {
                        unmatched_events.push(event);
                    }
                }
                self.pending_events = unmatched_events;
                self.pending_head = 0;
                self.subscribers.insert(subscriber_id, mailbox);
                return subscriber_id;
            }
        }
    }

    fn event_window_id(&self, event: &Event) -> Option<u32> {
        match event {
            Event::Input(event) => Some(event.surface_id),
            Event::TextInputPreedit { context_id, .. }
            | Event::TextInputCommit { context_id, .. }
            | Event::TextInputDeleteSurroundingText { context_id, .. }
            | Event::TextInputDone { context_id, .. }
            | Event::TextInputStatus { context_id, .. }
            | Event::ImeDeactivate { context_id, .. }
            | Event::ImeReset { context_id, .. }
            | Event::ImeTrigger { context_id, .. } => {
                self.text_input_windows.get(context_id).copied()
            }
            Event::ImeActivate(state) | Event::ImeContextState(state) => Some(state.window_id),
            Event::ImeKeyEvent { window_id, .. } => Some(*window_id),
            Event::SurfaceConfigure { surface_id, .. } | Event::SurfaceDestroyed { surface_id } => {
                Some(*surface_id)
            }
            Event::MenuItemActivated { window_id, .. }
            | Event::SgfxFrameRejected { window_id, .. }
            | Event::SgfxBufferReleased { window_id, .. } => Some(*window_id),
            Event::ScreenSizeChanged { .. }
            | Event::OutputScaleChanged { .. }
            | Event::SgfxBackendLost { .. }
            | Event::FocusChanged { .. }
            | Event::ActiveAppChanged { .. }
            | Event::Error { .. } => None,
        }
    }

    fn event_matches(filter: EventFilter, target_window: Option<u32>, event: &Event) -> bool {
        match filter {
            EventFilter::Window(window_id) => {
                target_window.map_or(true, |target| target == window_id)
                    && !matches!(
                        event,
                        Event::SgfxFrameRejected { .. }
                            | Event::SgfxBufferReleased { .. }
                            | Event::SgfxBackendLost { .. }
                    )
            }
            EventFilter::Sgfx(window_id) => match event {
                Event::SgfxFrameRejected {
                    window_id: event_window_id,
                    ..
                }
                | Event::SgfxBufferReleased {
                    window_id: event_window_id,
                    ..
                } => *event_window_id == window_id,
                Event::SgfxBackendLost { .. } => true,
                _ => false,
            },
        }
    }

    fn push_event(&mut self, event: Event) {
        let target_window = self.event_window_id(&event);
        let mut delivered = false;
        for mailbox in self.subscribers.values_mut() {
            if Self::event_matches(mailbox.filter, target_window, &event) {
                mailbox.push(event.clone());
                delivered = true;
            }
        }
        if !delivered {
            self.pending_events.push(event);
        }
    }
}

/// Connection to the Scarlet Window Server
///
/// Clones share one socket, one frame parser, response mailboxes, the event
/// queue, and all surfaces. Any clone may issue a request or dispatch events;
/// only the shared transport state ever reads from the socket.
#[derive(Clone)]
pub struct Connection {
    transport: Arc<Mutex<TransportState>>,
    surfaces: Arc<Mutex<BTreeMap<u32, Surface>>>,
}

impl Connection {
    /// Connect to SWS at the default socket path (/tmp/sws.sock)
    pub fn connect_default() -> Result<Self, Error> {
        Self::connect("/tmp/sws.sock")
    }

    /// Connect to SWS at the specified socket path
    pub fn connect(socket_path: &str) -> Result<Self, Error> {
        let socket = Socket::new().map_err(|_| Error::SocketCreation)?;
        socket
            .connect(socket_path)
            .map_err(|_| Error::ConnectionFailed)?;

        // Set socket to non-blocking mode once at connection time
        socket
            .set_nonblocking(true)
            .map_err(|_| Error::SocketConfig)?;

        Ok(Self {
            transport: Arc::new(Mutex::new(TransportState::new(socket))),
            surfaces: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    /// Send a routed request without blocking for its response.
    ///
    /// The request mailbox is registered before any bytes are sent. The
    /// returned token may be moved to another thread and waited there.
    ///
    /// # Arguments
    ///
    /// * `msg_type` - SWS client message type.
    /// * `payload` - Encoded request payload.
    ///
    /// # Returns
    ///
    /// A unique in-flight request token.
    pub fn send_request(&self, msg_type: u32, payload: &[u8]) -> Result<RequestToken, Error> {
        let request_id = mutex_lock(&self.transport).send_request(msg_type, payload, None)?;
        Ok(RequestToken {
            request_id,
            transport: Some(Arc::clone(&self.transport)),
        })
    }

    /// Send a routed request and one handle as an atomic transport record.
    ///
    /// # Arguments
    ///
    /// * `msg_type` - SWS client message type.
    /// * `payload` - Encoded request payload.
    /// * `handle` - Kernel object handle transferred with this request.
    ///
    /// # Returns
    ///
    /// A unique in-flight request token.
    pub fn send_request_with_handle(
        &self,
        msg_type: u32,
        payload: &[u8],
        handle: &Handle,
    ) -> Result<RequestToken, Error> {
        let request_id =
            mutex_lock(&self.transport).send_request(msg_type, payload, Some(handle))?;
        Ok(RequestToken {
            request_id,
            transport: Some(Arc::clone(&self.transport)),
        })
    }

    /// Cancel an in-flight request.
    ///
    /// The identifier remains reserved by a tombstone until the late response
    /// arrives, at which point that response is discarded and the identifier
    /// becomes reusable. It is never routed to another request.
    ///
    /// # Arguments
    ///
    /// * `token` - In-flight request token to cancel.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the token belongs to this connection.
    pub fn cancel_request(&self, mut token: RequestToken) -> Result<(), Error> {
        let Some(transport) = token.transport.take() else {
            return Err(Error::InvalidRequest);
        };
        if !Arc::ptr_eq(&transport, &self.transport) {
            token.transport = Some(transport);
            return Err(Error::InvalidRequest);
        }
        mutex_lock(&self.transport).cancel_response(token.request_id);
        Ok(())
    }

    /// Wait until the response associated with a request token is complete.
    ///
    /// While waiting, this thread cooperatively pumps the shared parser. Frames
    /// for other requests and unsolicited events are retained for their owners.
    ///
    /// # Arguments
    ///
    /// * `token` - Token returned by a request issued on this connection.
    ///
    /// # Returns
    ///
    /// The owned response envelope, including an optional transferred handle.
    pub fn wait_response(&self, mut token: RequestToken) -> Result<Response, Error> {
        if !token
            .transport
            .as_ref()
            .map_or(false, |transport| Arc::ptr_eq(transport, &self.transport))
        {
            return Err(Error::InvalidRequest);
        }
        loop {
            {
                let mut transport = mutex_lock(&self.transport);
                if let Some(response) = transport.take_response(token.request_id) {
                    token.transport = None;
                    return response;
                }
                match transport.pump_once() {
                    Ok(_) => {
                        if let Some(response) = transport.take_response(token.request_id) {
                            token.transport = None;
                            return response;
                        }
                    }
                    Err(error) => {
                        if let Some(response) = transport.take_response(token.request_id) {
                            token.transport = None;
                            return response;
                        }
                        token.transport = None;
                        return Err(error);
                    }
                }
            }
            crate::os::yield_now();
        }
    }

    fn request(&self, msg_type: u32, payload: &[u8]) -> Result<Response, Error> {
        let token = self.send_request(msg_type, payload)?;
        self.wait_response(token)
    }

    /// Send an unsolicited client message.
    ///
    /// # Arguments
    ///
    /// * `msg_type` - SWS client message type.
    /// * `payload` - Encoded message payload.
    ///
    /// # Returns
    ///
    /// `Ok(())` once the complete frame has been serialized to the socket.
    pub fn send_message(&self, msg_type: u32, payload: &[u8]) -> Result<(), Error> {
        mutex_lock(&self.transport).send_message(msg_type, payload)
    }

    /// Subscribe to UI and broadcast events for one window.
    ///
    /// Window-specific events for other windows remain in their own mailboxes.
    /// Global events such as output-scale changes are copied to every active
    /// window subscription. Events claimed by a subscription are not also
    /// retained by [`Self::poll_event`] or [`Self::drain_events`].
    ///
    /// # Arguments
    ///
    /// * `window_id` - Window whose input and lifecycle events are requested.
    ///
    /// # Returns
    ///
    /// An independent event receiver unregistered automatically on drop.
    pub fn subscribe_window_events(&self, window_id: u32) -> EventReceiver {
        let subscriber_id =
            mutex_lock(&self.transport).register_subscriber(EventFilter::Window(window_id));
        EventReceiver {
            subscriber_id,
            transport: Arc::clone(&self.transport),
            surfaces: Arc::clone(&self.surfaces),
        }
    }

    /// Subscribe to SGFX release and backend-loss events for one window.
    /// Events claimed by this subscription are not also retained by
    /// [`Self::poll_event`] or [`Self::drain_events`].
    ///
    /// # Arguments
    ///
    /// * `window_id` - Window whose registered buffers are tracked.
    ///
    /// # Returns
    ///
    /// An independent event receiver. Backend-loss broadcasts are copied to
    /// every active SGFX subscription.
    pub fn subscribe_sgfx_events(&self, window_id: u32) -> EventReceiver {
        let subscriber_id =
            mutex_lock(&self.transport).register_subscriber(EventFilter::Sgfx(window_id));
        EventReceiver {
            subscriber_id,
            transport: Arc::clone(&self.transport),
            surfaces: Arc::clone(&self.surfaces),
        }
    }

    /// Create a text-input context for a surface.
    pub fn create_text_input_context(
        &self,
        surface_id: u32,
        seat_id: u32,
    ) -> Result<(u32, u32), Error> {
        let payload = protocol::payload_text_input_create(surface_id, seat_id);
        let response = self.request(protocol::client_msg::TEXT_INPUT_CREATE, &payload)?;
        match response.message() {
            ServerMessage::TextInputCreated { context_id, serial } => {
                mutex_lock(&self.transport)
                    .text_input_windows
                    .insert(context_id, surface_id);
                Ok((context_id, serial))
            }
            _ => Err(Error::InvalidResponse),
        }
    }

    pub fn destroy_text_input_context(&self, context_id: u32) -> Result<(), Error> {
        let payload = protocol::payload_text_input_context_id(context_id);
        self.send_message(protocol::client_msg::TEXT_INPUT_DESTROY, &payload)?;
        mutex_lock(&self.transport)
            .text_input_windows
            .remove(&context_id);
        Ok(())
    }

    pub fn enable_text_input(&self, context_id: u32) -> Result<(), Error> {
        let payload = protocol::payload_text_input_context_id(context_id);
        self.send_message(protocol::client_msg::TEXT_INPUT_ENABLE, &payload)
    }

    pub fn disable_text_input(&self, context_id: u32) -> Result<(), Error> {
        let payload = protocol::payload_text_input_context_id(context_id);
        self.send_message(protocol::client_msg::TEXT_INPUT_DISABLE, &payload)
    }

    pub fn set_text_input_cursor_rect(
        &self,
        context_id: u32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), Error> {
        let payload = protocol::payload_text_input_set_cursor_rect(context_id, x, y, width, height);
        self.send_message(protocol::client_msg::TEXT_INPUT_SET_CURSOR_RECT, &payload)
    }

    pub fn set_text_input_surrounding_text(
        &self,
        context_id: u32,
        cursor_byte: u32,
        anchor_byte: u32,
        text: &str,
    ) -> Result<(), Error> {
        let payload = protocol::payload_text_input_set_surrounding_text(
            context_id,
            cursor_byte,
            anchor_byte,
            text.as_bytes(),
        );
        self.send_message(
            protocol::client_msg::TEXT_INPUT_SET_SURROUNDING_TEXT,
            &payload,
        )
    }

    pub fn set_text_input_content_type(
        &self,
        context_id: u32,
        hint: u32,
        purpose: u32,
    ) -> Result<(), Error> {
        let payload = protocol::payload_text_input_set_content_type(context_id, hint, purpose);
        self.send_message(protocol::client_msg::TEXT_INPUT_SET_CONTENT_TYPE, &payload)
    }

    pub fn set_text_input_change_cause(&self, context_id: u32, cause: u32) -> Result<(), Error> {
        let payload = protocol::payload_text_input_set_text_change_cause(context_id, cause);
        self.send_message(
            protocol::client_msg::TEXT_INPUT_SET_TEXT_CHANGE_CAUSE,
            &payload,
        )
    }

    pub fn commit_text_input_state(&self, context_id: u32, serial: u32) -> Result<(), Error> {
        let payload = protocol::payload_text_input_commit_state(context_id, serial);
        self.send_message(protocol::client_msg::TEXT_INPUT_COMMIT_STATE, &payload)
    }

    /// Register this connection as an input method service.
    pub fn register_input_method(&self, name: &str, capabilities: u32) -> Result<u32, Error> {
        let payload = protocol::payload_ime_register(name.as_bytes(), capabilities);
        let response = self.request(protocol::client_msg::IME_REGISTER, &payload)?;
        match response.message() {
            ServerMessage::ImeRegistered { ime_id } => Ok(ime_id),
            _ => Err(Error::InvalidResponse),
        }
    }

    /// Select and persist the active input method.
    ///
    /// SWS resolves the runtime identifier to the input method's stable name
    /// before storing the selection in its configuration.
    ///
    /// # Arguments
    ///
    /// * `ime_id` - Runtime identifier returned by `get_input_methods`.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the request was sent to SWS, or a connection error.
    pub fn set_active_input_method(&self, ime_id: u32) -> Result<(), Error> {
        let payload = protocol::payload_ime_set_active(ime_id);
        self.send_message(protocol::client_msg::IME_SET_ACTIVE, &payload)
    }

    /// Get registered input methods.
    ///
    /// # Returns
    ///
    /// List of input methods currently registered with SWS.
    pub fn get_input_methods(&self) -> Result<Vec<InputMethodInfo>, Error> {
        let response = self.request(protocol::client_msg::IME_GET_METHODS, &[])?;
        match response.message() {
            ServerMessage::ImeMethods => {
                let methods = protocol::parse_ime_methods_payload(response.payload())
                    .map_err(|_| Error::InvalidResponse)?;
                Ok(methods
                    .into_iter()
                    .map(|method| InputMethodInfo {
                        ime_id: method.ime_id,
                        name: String::from_utf8_lossy(&method.name[..method.name_len as usize])
                            .into_owned(),
                        capabilities: method.capabilities,
                        active: method.active,
                    })
                    .collect())
            }
            _ => Err(Error::InvalidResponse),
        }
    }

    /// Get the active input method.
    ///
    /// # Returns
    ///
    /// Active input method information, or `None` when no IME is active.
    pub fn get_active_input_method(&self) -> Result<Option<InputMethodInfo>, Error> {
        let response = self.request(protocol::client_msg::IME_GET_ACTIVE, &[])?;
        match response.message() {
            ServerMessage::ImeActive => {
                let method = protocol::parse_ime_active_payload(response.payload())
                    .map_err(|_| Error::InvalidResponse)?;
                Ok(method.map(|method| InputMethodInfo {
                    ime_id: method.ime_id,
                    name: String::from_utf8_lossy(&method.name[..method.name_len as usize])
                        .into_owned(),
                    capabilities: method.capabilities,
                    active: method.active,
                }))
            }
            _ => Err(Error::InvalidResponse),
        }
    }

    pub fn ime_key_handled(&self, key_serial: u32, handled: bool) -> Result<(), Error> {
        let payload = protocol::payload_ime_key_handled(key_serial, handled);
        self.send_message(protocol::client_msg::IME_KEY_HANDLED, &payload)
    }

    pub fn ime_set_preedit(
        &self,
        context_id: u32,
        cursor_byte: u32,
        anchor_byte: u32,
        text: &str,
        spans: &[u8],
    ) -> Result<(), Error> {
        let payload = protocol::payload_ime_set_preedit(
            context_id,
            cursor_byte,
            anchor_byte,
            text.as_bytes(),
            spans,
        );
        self.send_message(protocol::client_msg::IME_SET_PREEDIT, &payload)
    }

    pub fn ime_commit_text(&self, context_id: u32, text: &str) -> Result<(), Error> {
        let payload = protocol::payload_ime_commit_text(context_id, text.as_bytes());
        self.send_message(protocol::client_msg::IME_COMMIT_TEXT, &payload)
    }

    pub fn ime_delete_surrounding_text(
        &self,
        context_id: u32,
        before_bytes: u32,
        after_bytes: u32,
    ) -> Result<(), Error> {
        let payload =
            protocol::payload_ime_delete_surrounding_text(context_id, before_bytes, after_bytes);
        self.send_message(protocol::client_msg::IME_DELETE_SURROUNDING_TEXT, &payload)
    }

    pub fn ime_set_status(
        &self,
        context_id: u32,
        state: u32,
        mode_id: u32,
        flags: u32,
        mode_label: &str,
    ) -> Result<(), Error> {
        let payload = protocol::payload_ime_set_status(
            context_id,
            state,
            mode_id,
            flags,
            mode_label.as_bytes(),
        );
        self.send_message(protocol::client_msg::IME_SET_STATUS, &payload)
    }

    pub fn ime_set_popup_window(
        &self,
        context_id: u32,
        window_id: u32,
        offset_x: i32,
        offset_y: i32,
        visible: bool,
    ) -> Result<(), Error> {
        let payload = protocol::payload_ime_set_popup_window(
            context_id, window_id, offset_x, offset_y, visible,
        );
        self.send_message(protocol::client_msg::IME_SET_POPUP_WINDOW, &payload)
    }

    pub fn ime_grab_keyboard(&self, context_id: u32) -> Result<(), Error> {
        let payload = protocol::payload_ime_grab_keyboard(context_id);
        self.send_message(protocol::client_msg::IME_GRAB_KEYBOARD, &payload)
    }

    pub fn ime_release_keyboard(&self, context_id: u32) -> Result<(), Error> {
        let payload = protocol::payload_ime_release_keyboard(context_id);
        self.send_message(protocol::client_msg::IME_RELEASE_KEYBOARD, &payload)
    }

    /// Create a new surface (window)
    ///
    /// This sends a CreateWindow request and waits for the response.
    /// The returned Surface can be drawn to immediately.
    /// Default window type is NORMAL (0).
    pub fn create_surface(
        &self,
        app_id: &str,
        app_name: &str,
        menu_titles: &str,
        width: u32,
        height: u32,
    ) -> Result<u32, Error> {
        self.create_surface_with_type_and_resizable(
            app_id,
            app_name,
            menu_titles,
            width,
            height,
            0,
            true,
        )
    }

    /// Create a new surface (window) with specific window type
    ///
    /// This sends a CreateWindow request and waits for the response.
    /// The returned Surface can be drawn to immediately.
    pub fn create_surface_with_type(
        &self,
        app_id: &str,
        app_name: &str,
        menu_titles: &str,
        width: u32,
        height: u32,
        window_type: u32,
    ) -> Result<u32, Error> {
        self.create_surface_with_type_and_resizable(
            app_id,
            app_name,
            menu_titles,
            width,
            height,
            window_type,
            true,
        )
    }

    /// Create a new surface (window) with specific window type and resizable flag
    ///
    /// This sends a CreateWindow request and waits for the response.
    /// The returned Surface can be drawn to immediately.
    pub fn create_surface_with_type_and_resizable(
        &self,
        app_id: &str,
        app_name: &str,
        menu_titles: &str,
        width: u32,
        height: u32,
        window_type: u32,
        resizable: bool,
    ) -> Result<u32, Error> {
        let focus_on_create = true;
        let active_on_focus = window_type == 0;
        self.create_surface_with_type_and_policies(
            app_id,
            app_name,
            menu_titles,
            width,
            height,
            window_type,
            resizable,
            focus_on_create,
            active_on_focus,
        )
    }

    /// Create a new surface (window) with explicit focus/active policies
    pub fn create_surface_with_type_and_policies(
        &self,
        app_id: &str,
        app_name: &str,
        menu_titles: &str,
        width: u32,
        height: u32,
        window_type: u32,
        resizable: bool,
        focus_on_create: bool,
        active_on_focus: bool,
    ) -> Result<u32, Error> {
        // Send CreateWindow request
        let payload = protocol::payload_create_window(
            app_id.as_bytes(),
            app_name.as_bytes(),
            menu_titles.as_bytes(),
            width,
            height,
            window_type,
            resizable,
            focus_on_create,
            active_on_focus,
        );
        let mut response = self.request(protocol::client_msg::CREATE_WINDOW, &payload)?;
        let surface_id = match response.message() {
            ServerMessage::WindowCreated { window_id, .. } => window_id,
            _ => return Err(Error::InvalidResponse),
        };
        let shm_handle = response.take_handle().ok_or(Error::ShmHandleFailed)?;
        let shm = SharedMemory::from_handle(shm_handle).map_err(|_| Error::ShmHandleFailed)?;
        let surface = Surface::new(surface_id, width, height, shm)?;
        mutex_lock(&self.surfaces).insert(surface_id, surface);
        Ok(surface_id)
    }

    /// Create a new surface with an explicit initial placement policy.
    pub fn create_surface_with_type_and_policies_with_placement(
        &self,
        app_id: &str,
        app_name: &str,
        menu_titles: &str,
        width: u32,
        height: u32,
        window_type: u32,
        resizable: bool,
        focus_on_create: bool,
        active_on_focus: bool,
        placement: protocol::WindowPlacement,
    ) -> Result<u32, Error> {
        let payload = match placement {
            protocol::WindowPlacement::Default => protocol::payload_create_window(
                app_id.as_bytes(),
                app_name.as_bytes(),
                menu_titles.as_bytes(),
                width,
                height,
                window_type,
                resizable,
                focus_on_create,
                active_on_focus,
            ),
            protocol::WindowPlacement::Centered => protocol::payload_create_window_with_placement(
                app_id.as_bytes(),
                app_name.as_bytes(),
                menu_titles.as_bytes(),
                width,
                height,
                window_type,
                resizable,
                focus_on_create,
                active_on_focus,
                protocol::window_placement::CENTERED,
                0,
                0,
            ),
            protocol::WindowPlacement::Absolute { x, y } => {
                protocol::payload_create_window_with_placement(
                    app_id.as_bytes(),
                    app_name.as_bytes(),
                    menu_titles.as_bytes(),
                    width,
                    height,
                    window_type,
                    resizable,
                    focus_on_create,
                    active_on_focus,
                    protocol::window_placement::ABSOLUTE,
                    x,
                    y,
                )
            }
        };
        let mut response = self.request(protocol::client_msg::CREATE_WINDOW, &payload)?;
        let surface_id = match response.message() {
            ServerMessage::WindowCreated { window_id, .. } => window_id,
            _ => return Err(Error::InvalidResponse),
        };
        let shm_handle = response.take_handle().ok_or(Error::ShmHandleFailed)?;
        let shm = SharedMemory::from_handle(shm_handle).map_err(|_| Error::ShmHandleFailed)?;
        let surface = Surface::new(surface_id, width, height, shm)?;
        mutex_lock(&self.surfaces).insert(surface_id, surface);
        Ok(surface_id)
    }

    /// Create a new surface with an initial placement and activation token.
    ///
    /// # Arguments
    ///
    /// * `app_id` - Stable application identifier for the new toplevel.
    /// * `app_name` - Human-readable application name.
    /// * `menu_titles` - Serialized application menu titles.
    /// * `width` - Initial buffer width in physical pixels.
    /// * `height` - Initial buffer height in physical pixels.
    /// * `window_type` - SWS window role.
    /// * `resizable` - Whether interactive resizing is permitted.
    /// * `focus_on_create` - Whether the new window normally requests focus.
    /// * `active_on_focus` - Whether focus makes this the active application.
    /// * `placement` - Client-side placement hint, subject to compositor policy.
    /// * `activation_token` - Opaque one-shot token previously issued by SWS.
    ///
    /// # Returns
    ///
    /// The new SWS surface identifier.
    pub fn create_surface_with_type_and_policies_with_activation_token(
        &self,
        app_id: &str,
        app_name: &str,
        menu_titles: &str,
        width: u32,
        height: u32,
        window_type: u32,
        resizable: bool,
        focus_on_create: bool,
        active_on_focus: bool,
        placement: protocol::WindowPlacement,
        activation_token: &str,
    ) -> Result<u32, Error> {
        if activation_token.is_empty()
            || activation_token.len() > protocol::ACTIVATION_TOKEN_MAX_BYTES
        {
            return Err(Error::InvalidRequest);
        }

        let (placement, initial_x, initial_y) = match placement {
            protocol::WindowPlacement::Default => (protocol::window_placement::DEFAULT, 0, 0),
            protocol::WindowPlacement::Centered => (protocol::window_placement::CENTERED, 0, 0),
            protocol::WindowPlacement::Absolute { x, y } => {
                (protocol::window_placement::ABSOLUTE, x, y)
            }
        };
        let payload = protocol::payload_create_window_with_placement_and_activation_token(
            app_id.as_bytes(),
            app_name.as_bytes(),
            menu_titles.as_bytes(),
            width,
            height,
            window_type,
            resizable,
            focus_on_create,
            active_on_focus,
            placement,
            initial_x,
            initial_y,
            activation_token.as_bytes(),
        );
        let mut response = self.request(protocol::client_msg::CREATE_WINDOW, &payload)?;
        let surface_id = match response.message() {
            ServerMessage::WindowCreated { window_id, .. } => window_id,
            ServerMessage::Error { code } => return Err(Error::ServerError(code)),
            _ => return Err(Error::InvalidResponse),
        };
        let shm_handle = response.take_handle().ok_or(Error::ShmHandleFailed)?;
        let shm = SharedMemory::from_handle(shm_handle).map_err(|_| Error::ShmHandleFailed)?;
        let surface = Surface::new(surface_id, width, height, shm)?;
        mutex_lock(&self.surfaces).insert(surface_id, surface);
        Ok(surface_id)
    }

    /// Create a new surface (window) with explicit focus/active policies and initial position.
    pub fn create_surface_with_type_and_policies_at(
        &self,
        app_id: &str,
        app_name: &str,
        menu_titles: &str,
        width: u32,
        height: u32,
        window_type: u32,
        resizable: bool,
        focus_on_create: bool,
        active_on_focus: bool,
        x: i32,
        y: i32,
    ) -> Result<u32, Error> {
        let payload = protocol::payload_create_window_with_position(
            app_id.as_bytes(),
            app_name.as_bytes(),
            menu_titles.as_bytes(),
            width,
            height,
            window_type,
            resizable,
            focus_on_create,
            active_on_focus,
            x,
            y,
        );
        let mut response = self.request(protocol::client_msg::CREATE_WINDOW, &payload)?;
        let surface_id = match response.message() {
            ServerMessage::WindowCreated { window_id, .. } => window_id,
            _ => return Err(Error::InvalidResponse),
        };
        let shm_handle = response.take_handle().ok_or(Error::ShmHandleFailed)?;
        let shm = SharedMemory::from_handle(shm_handle).map_err(|_| Error::ShmHandleFailed)?;
        let surface = Surface::new(surface_id, width, height, shm)?;
        mutex_lock(&self.surfaces).insert(surface_id, surface);
        Ok(surface_id)
    }

    /// Destroy a surface
    pub fn destroy_surface(&self, surface_id: u32) -> Result<(), Error> {
        if mutex_lock(&self.surfaces).remove(&surface_id).is_none() {
            return Err(Error::SurfaceNotFound);
        }

        let payload = protocol::payload_destroy_window(surface_id);
        self.send_message(protocol::client_msg::DESTROY_WINDOW, &payload)
            .map_err(|_| Error::SendFailed)?;

        Ok(())
    }

    /// Set per-window size constraints.
    ///
    /// All values are in pixels. `0` means "unset".
    pub fn set_window_size_limits(
        &self,
        surface_id: u32,
        limits: WindowSizeLimits,
    ) -> Result<(), Error> {
        self.set_window_size_limits_raw(
            surface_id,
            limits.min_width,
            limits.min_height,
            limits.max_width,
            limits.max_height,
        )
    }

    /// Set per-window size constraints (raw values).
    ///
    /// Prefer [`set_window_size_limits`] with [`WindowSizeLimits`].
    pub fn set_window_size_limits_raw(
        &self,
        surface_id: u32,
        min_width: u32,
        min_height: u32,
        max_width: u32,
        max_height: u32,
    ) -> Result<(), Error> {
        if !mutex_lock(&self.surfaces).contains_key(&surface_id) {
            return Err(Error::SurfaceNotFound);
        }

        let payload = protocol::payload_set_window_size_limits(
            surface_id, min_width, min_height, max_width, max_height,
        );
        self.send_message(protocol::client_msg::SET_WINDOW_SIZE_LIMITS, &payload)
            .map_err(|_| Error::SendFailed)
    }

    /// Update menu titles for a window (format: "menu1|menu2|menu3").
    pub fn set_window_menu_titles(&self, surface_id: u32, menu_titles: &str) -> Result<(), Error> {
        if !mutex_lock(&self.surfaces).contains_key(&surface_id) {
            return Err(Error::SurfaceNotFound);
        }

        let payload = protocol::payload_set_window_menu_titles(surface_id, menu_titles.as_bytes());
        self.send_message(protocol::client_msg::SET_WINDOW_MENU_TITLES, &payload)
            .map_err(|_| Error::SendFailed)
    }

    /// Notify the server that a menu item was activated for a window.
    pub fn activate_menu_item(&self, window_id: u32, menu_item_id: &str) -> Result<(), Error> {
        let payload = protocol::payload_activate_menu_item(window_id, menu_item_id.as_bytes());
        self.send_message(protocol::client_msg::ACTIVATE_MENU_ITEM, &payload)
            .map_err(|_| Error::SendFailed)
    }

    /// Execute a closure with shared access to a surface.
    ///
    /// # Arguments
    ///
    /// * `surface_id` - Server-assigned surface identifier.
    /// * `f` - Operation performed while the shared surface map is locked.
    ///
    /// # Returns
    ///
    /// The closure result, or `None` if the surface does not exist.
    pub fn with_surface<F, R>(&self, surface_id: u32, f: F) -> Option<R>
    where
        F: FnOnce(&Surface) -> R,
    {
        let surfaces = mutex_lock(&self.surfaces);
        surfaces.get(&surface_id).map(f)
    }

    /// Execute a closure with exclusive access to a surface.
    ///
    /// # Arguments
    ///
    /// * `surface_id` - Server-assigned surface identifier.
    /// * `f` - Operation performed while the shared surface map is locked.
    ///
    /// # Returns
    ///
    /// The closure result, or `None` if the surface does not exist.
    pub fn with_surface_mut<F, R>(&self, surface_id: u32, f: F) -> Option<R>
    where
        F: FnOnce(&mut Surface) -> R,
    {
        let mut surfaces = mutex_lock(&self.surfaces);
        surfaces.get_mut(&surface_id).map(f)
    }

    /// Commit surface changes to the server
    ///
    /// This notifies the server that the surface buffer has been updated.
    pub fn commit(&self, surface_id: u32) -> Result<(), Error> {
        let payload = {
            let mut surfaces = mutex_lock(&self.surfaces);
            let surface = surfaces
                .get_mut(&surface_id)
                .ok_or(Error::SurfaceNotFound)?;
            if !surface.is_dirty() {
                return Ok(());
            }
            let payload = protocol::payload_update_buffer(
                surface_id,
                0,
                0,
                surface.width(),
                surface.height(),
            );
            surface.clear_dirty();
            payload
        };

        if self
            .send_message(protocol::client_msg::UPDATE_BUFFER, &payload)
            .is_err()
        {
            if let Some(surface) = mutex_lock(&self.surfaces).get_mut(&surface_id) {
                surface.mark_dirty();
            }
            return Err(Error::SendFailed);
        }
        Ok(())
    }

    /// Commit a specific region of the surface to the server
    ///
    /// This is more efficient than `commit()` when only a small region changed.
    pub fn commit_region(
        &self,
        surface_id: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<(), Error> {
        let payload = {
            let mut surfaces = mutex_lock(&self.surfaces);
            let surface = surfaces
                .get_mut(&surface_id)
                .ok_or(Error::SurfaceNotFound)?;
            let sw = surface.width();
            let sh = surface.height();
            let x = x.min(sw);
            let y = y.min(sh);
            let width = width.min(sw.saturating_sub(x));
            let height = height.min(sh.saturating_sub(y));
            if width == 0 || height == 0 {
                return Ok(());
            }
            let payload =
                protocol::payload_update_buffer(surface_id, x as i32, y as i32, width, height);
            surface.clear_dirty();
            payload
        };

        if self
            .send_message(protocol::client_msg::UPDATE_BUFFER, &payload)
            .is_err()
        {
            if let Some(surface) = mutex_lock(&self.surfaces).get_mut(&surface_id) {
                surface.mark_dirty();
            }
            return Err(Error::SendFailed);
        }
        Ok(())
    }

    /// Flush pending writes to the socket
    pub fn flush(&self) -> Result<(), Error> {
        let mut transport = mutex_lock(&self.transport);
        socket_flush(&mut transport.socket)
    }

    /// Request that the window manager begins an interactive move for this surface.
    ///
    /// The server is expected to track pointer movement and update the window position
    /// until the primary button is released.
    pub fn request_move_window(&self, surface_id: u32) -> Result<(), Error> {
        let payload = protocol::payload_request_move_window(surface_id);
        self.send_message(protocol::client_msg::REQUEST_MOVE_WINDOW, &payload)
            .map_err(|_| Error::SendFailed)
    }

    /// Set the window position (absolute) for this surface.
    pub fn move_window(&self, surface_id: u32, x: i32, y: i32) -> Result<(), Error> {
        let payload = protocol::payload_move_window(surface_id, x, y);
        self.send_message(protocol::client_msg::MOVE_WINDOW, &payload)
            .map_err(|_| Error::SendFailed)
    }

    /// Set (or clear) the logical parent of a window.
    ///
    /// Use this for transient dialogs/popups so the compositor can keep the child
    /// stacked above its parent and move it together during interactive drags.
    ///
    /// `parent_surface_id == None` clears the parent.
    pub fn set_window_parent(
        &self,
        surface_id: u32,
        parent_surface_id: Option<u32>,
    ) -> Result<(), Error> {
        let parent_id = parent_surface_id.unwrap_or(0);
        let payload = protocol::payload_set_window_parent(surface_id, parent_id);
        self.send_message(protocol::client_msg::SET_WINDOW_PARENT, &payload)
            .map_err(|_| Error::SendFailed)
    }

    /// Configure transient behavior flags for a window.
    pub fn set_window_transient_flags(
        &self,
        surface_id: u32,
        flags: TransientFlags,
    ) -> Result<(), Error> {
        self.set_window_transient_flags_raw(surface_id, flags.bits())
    }

    /// Configure transient behavior flags for a window (raw bits).
    ///
    /// Prefer [`set_window_transient_flags`] with [`TransientFlags`].
    pub fn set_window_transient_flags_raw(&self, surface_id: u32, flags: u32) -> Result<(), Error> {
        let payload = protocol::payload_set_window_transient_flags(surface_id, flags);
        self.send_message(protocol::client_msg::SET_WINDOW_TRANSIENT_FLAGS, &payload)
            .map_err(|_| Error::SendFailed)
    }

    /// Minimize a window (hide it; buffer size remains unchanged).
    pub fn minimize_window(&self, surface_id: u32) -> Result<(), Error> {
        if !mutex_lock(&self.surfaces).contains_key(&surface_id) {
            return Err(Error::SurfaceNotFound);
        }
        let payload = protocol::payload_minimize_window(surface_id);
        self.send_message(protocol::client_msg::MINIMIZE_WINDOW, &payload)
            .map_err(|_| Error::SendFailed)
    }

    /// Maximize a window.
    ///
    /// The server may respond with `WINDOW_CONFIGURE` to request a buffer resize.
    pub fn maximize_window(&self, surface_id: u32) -> Result<(), Error> {
        if !mutex_lock(&self.surfaces).contains_key(&surface_id) {
            return Err(Error::SurfaceNotFound);
        }
        let payload = protocol::payload_maximize_window(surface_id);
        self.send_message(protocol::client_msg::MAXIMIZE_WINDOW, &payload)
            .map_err(|_| Error::SendFailed)
    }

    /// Restore a window from minimized or maximized state.
    pub fn restore_window(&self, surface_id: u32) -> Result<(), Error> {
        if !mutex_lock(&self.surfaces).contains_key(&surface_id) {
            return Err(Error::SurfaceNotFound);
        }
        let payload = protocol::payload_restore_window(surface_id);
        self.send_message(protocol::client_msg::RESTORE_WINDOW, &payload)
            .map_err(|_| Error::SendFailed)
    }

    /// Focus and raise a window to the top of the Z-order.
    ///
    /// This only works for surfaces created by this client connection.
    /// For focusing windows created by other clients, use `focus_window_any`.
    pub fn focus_window(&self, surface_id: u32) -> Result<(), Error> {
        if !mutex_lock(&self.surfaces).contains_key(&surface_id) {
            return Err(Error::SurfaceNotFound);
        }
        let payload = protocol::payload_focus_window(surface_id);
        self.send_message(protocol::client_msg::FOCUS_WINDOW, &payload)
            .map_err(|_| Error::SendFailed)
    }

    /// Focus and raise any window (including those created by other clients).
    ///
    /// Unlike `focus_window`, this does not check if the surface exists locally.
    /// This is useful for system services like stemd that need to focus windows
    /// created by other applications.
    ///
    /// The server will return an error if the window_id does not exist.
    pub fn focus_window_any(&self, window_id: u32) -> Result<(), Error> {
        let payload = protocol::payload_focus_window(window_id);
        self.send_message(protocol::client_msg::FOCUS_WINDOW, &payload)
            .map_err(|_| Error::SendFailed)
    }

    /// Set the window type used for Z-order management.
    ///
    /// The `window_type` argument selects one of the window type constants defined
    /// by the SWS protocol:
    ///
    /// - `NORMAL = 0`: Standard application window.
    /// - `ALWAYS_ON_TOP = 1`: Stays above `NORMAL` and `TASKBAR` windows.
    /// - `TASKBAR = 2`: Taskbar or dock-style window, above `DESKTOP` but
    ///   below `ALWAYS_ON_TOP`.
    /// - `DESKTOP = 3`: Desktop background window, at the bottom of the
    ///   stacking order.
    ///
    /// Higher-priority types (for example `ALWAYS_ON_TOP`) are kept above
    /// lower-priority types in the global Z-order. See
    /// [`sws_protocol::window_types`] for the available constants.
    pub fn set_window_type(&self, surface_id: u32, window_type: u32) -> Result<(), Error> {
        if !mutex_lock(&self.surfaces).contains_key(&surface_id) {
            return Err(Error::SurfaceNotFound);
        }
        let payload = protocol::payload_set_window_type(surface_id, window_type);
        self.send_message(protocol::client_msg::SET_WINDOW_TYPE, &payload)
            .map_err(|_| Error::SendFailed)
    }

    /// Set per-window opacity (0 = fully transparent, 255 = fully opaque).
    pub fn set_window_opacity(&self, surface_id: u32, opacity: u8) -> Result<(), Error> {
        if !mutex_lock(&self.surfaces).contains_key(&surface_id) {
            return Err(Error::SurfaceNotFound);
        }
        let payload = protocol::payload_set_window_opacity(surface_id, opacity);
        self.send_message(protocol::client_msg::SET_WINDOW_OPACITY, &payload)
            .map_err(|_| Error::SendFailed)
    }

    /// Set whether window content contains alpha channel (semi-transparent pixels).
    ///
    /// This is separate from window opacity - this controls whether pixel alpha
    /// values in the window buffer should be respected during composition.
    ///
    /// - false: Window content is fully opaque, use fast copy path (default)
    /// - true: Window content has semi-transparent pixels, use alpha blending
    pub fn set_window_has_alpha_content(
        &self,
        surface_id: u32,
        has_alpha: bool,
    ) -> Result<(), Error> {
        if !mutex_lock(&self.surfaces).contains_key(&surface_id) {
            return Err(Error::SurfaceNotFound);
        }
        let payload = protocol::payload_set_window_has_alpha_content(surface_id, has_alpha);
        self.send_message(protocol::client_msg::SET_WINDOW_HAS_ALPHA_CONTENT, &payload)
            .map_err(|_| Error::SendFailed)
    }

    /// Set the workarea (usable screen area) for the window manager.
    ///
    /// This informs the window manager about the area where normal windows
    /// should be placed, typically excluding the area occupied by the taskbar.
    pub fn set_workarea(&self, x: i32, y: i32, width: u32, height: u32) -> Result<(), Error> {
        let payload = protocol::payload_set_workarea(x, y, width, height);
        self.send_message(protocol::client_msg::SET_WORKAREA, &payload)
            .map_err(|_| Error::SendFailed)
    }

    /// Set whether a window can be resized by the user via interactive resize.
    pub fn set_window_resizable(&self, surface_id: u32, resizable: bool) -> Result<(), Error> {
        if !mutex_lock(&self.surfaces).contains_key(&surface_id) {
            return Err(Error::SurfaceNotFound);
        }
        let payload = protocol::payload_set_window_resizable(surface_id, resizable);
        self.send_message(protocol::client_msg::SET_WINDOW_RESIZABLE, &payload)
            .map_err(|_| Error::SendFailed)
    }

    /// Resize a surface.
    ///
    /// This is a synchronous request: it waits for `WINDOW_RESIZED` and a new SHM handle,
    /// then updates the local surface mapping.
    pub fn resize_window(&self, surface_id: u32, width: u32, height: u32) -> Result<(), Error> {
        if !mutex_lock(&self.surfaces).contains_key(&surface_id) {
            return Err(Error::SurfaceNotFound);
        }

        let payload = protocol::payload_resize_window(surface_id, width, height);
        let mut response = self.request(protocol::client_msg::RESIZE_WINDOW, &payload)?;
        let (window_id, new_w, new_h) = match response.message() {
            ServerMessage::WindowResized {
                window_id,
                width,
                height,
                ..
            } => (window_id, width, height),
            _ => return Err(Error::InvalidResponse),
        };

        if window_id != surface_id {
            return Err(Error::InvalidResponse);
        }

        let shm_handle = response.take_handle().ok_or(Error::ShmHandleFailed)?;
        let shm = SharedMemory::from_handle(shm_handle).map_err(|_| Error::ShmHandleFailed)?;
        if let Some(surface) = mutex_lock(&self.surfaces).get_mut(&surface_id) {
            surface.remap(new_w, new_h, shm)?;
            Ok(())
        } else {
            Err(Error::SurfaceNotFound)
        }
    }

    /// Dispatch pending events (non-blocking)
    ///
    /// Reads a bounded batch of available frames and routes them. Bounding each
    /// call prevents a busy event source from starving request senders.
    /// Returns the number of events read.
    pub fn dispatch(&self) -> Result<usize, Error> {
        let mut transport = mutex_lock(&self.transport);
        if transport.pending_head > 0
            && transport.pending_head * 2 >= transport.pending_events.len()
        {
            let consumed = transport.pending_head;
            transport.pending_events.drain(..consumed);
            transport.pending_head = 0;
        }

        let mut count = 0;
        for _ in 0..MAX_DISPATCH_FRAMES {
            let progress = transport.pump_once()?;
            if !progress.progressed {
                break;
            }
            if progress.event_queued {
                count += 1;
            }
        }
        Ok(count)
    }

    /// Pop the next pending event.
    pub fn poll_event(&self) -> Option<Event> {
        let event = mutex_lock(&self.transport).poll_event();
        if let Some(Event::SurfaceDestroyed { surface_id }) = event.as_ref() {
            mutex_lock(&self.surfaces).remove(surface_id);
        }
        event
    }

    /// Drain all pending events.
    pub fn drain_events(&self) -> Vec<Event> {
        let events = mutex_lock(&self.transport).drain_events();
        {
            let mut surfaces = mutex_lock(&self.surfaces);
            for event in &events {
                if let Event::SurfaceDestroyed { surface_id } = event {
                    surfaces.remove(surface_id);
                }
            }
        }
        events
    }
}

impl TransportState {
    fn queue_async_message(&mut self, message: ServerMessage) -> bool {
        match message {
            ServerMessage::InputEvent {
                window_id,
                time,
                type_,
                code,
                value,
            } => {
                self.push_event(Event::Input(InputEvent {
                    surface_id: window_id,
                    time,
                    type_,
                    code,
                    value,
                }));
                true
            }
            ServerMessage::TextInputPreedit {
                context_id,
                serial,
                cursor_byte,
                anchor_byte,
                text,
                text_len,
                spans,
                spans_len,
            } => {
                let text = String::from_utf8_lossy(&text[..text_len as usize]).into_owned();
                self.push_event(Event::TextInputPreedit {
                    context_id,
                    serial,
                    cursor_byte,
                    anchor_byte,
                    text,
                    spans: spans[..spans_len as usize].to_vec(),
                });
                true
            }
            ServerMessage::TextInputCommit {
                context_id,
                serial,
                text,
                text_len,
            } => {
                let text = String::from_utf8_lossy(&text[..text_len as usize]).into_owned();
                self.push_event(Event::TextInputCommit {
                    context_id,
                    serial,
                    text,
                });
                true
            }
            ServerMessage::TextInputDeleteSurroundingText {
                context_id,
                serial,
                before_bytes,
                after_bytes,
            } => {
                self.push_event(Event::TextInputDeleteSurroundingText {
                    context_id,
                    serial,
                    before_bytes,
                    after_bytes,
                });
                true
            }
            ServerMessage::TextInputDone { context_id, serial } => {
                self.push_event(Event::TextInputDone { context_id, serial });
                true
            }
            ServerMessage::TextInputStatus {
                context_id,
                serial,
                state,
                mode_id,
                flags,
                mode_label,
                mode_label_len,
            } => {
                let mode_label =
                    String::from_utf8_lossy(&mode_label[..mode_label_len as usize]).into_owned();
                self.push_event(Event::TextInputStatus {
                    context_id,
                    serial,
                    state,
                    mode_id,
                    flags,
                    mode_label,
                });
                true
            }
            ServerMessage::ImeActivate {
                context_id,
                window_id,
                serial,
                cursor_x,
                cursor_y,
                cursor_width,
                cursor_height,
                content_hint,
                content_purpose,
                text_change_cause,
                cursor_byte,
                anchor_byte,
                surrounding_text,
                surrounding_text_len,
            } => {
                let surrounding_text =
                    String::from_utf8_lossy(&surrounding_text[..surrounding_text_len as usize])
                        .into_owned();
                self.push_event(Event::ImeActivate(ImeContextState {
                    context_id,
                    window_id,
                    serial,
                    cursor_x,
                    cursor_y,
                    cursor_width,
                    cursor_height,
                    content_hint,
                    content_purpose,
                    text_change_cause,
                    cursor_byte,
                    anchor_byte,
                    surrounding_text,
                }));
                true
            }
            ServerMessage::ImeDeactivate { context_id, serial } => {
                self.push_event(Event::ImeDeactivate { context_id, serial });
                true
            }
            ServerMessage::ImeContextState {
                context_id,
                window_id,
                serial,
                cursor_x,
                cursor_y,
                cursor_width,
                cursor_height,
                content_hint,
                content_purpose,
                text_change_cause,
                cursor_byte,
                anchor_byte,
                surrounding_text,
                surrounding_text_len,
            } => {
                let surrounding_text =
                    String::from_utf8_lossy(&surrounding_text[..surrounding_text_len as usize])
                        .into_owned();
                self.push_event(Event::ImeContextState(ImeContextState {
                    context_id,
                    window_id,
                    serial,
                    cursor_x,
                    cursor_y,
                    cursor_width,
                    cursor_height,
                    content_hint,
                    content_purpose,
                    text_change_cause,
                    cursor_byte,
                    anchor_byte,
                    surrounding_text,
                }));
                true
            }
            ServerMessage::ImeKeyEvent {
                context_id,
                key_serial,
                window_id,
                time,
                type_,
                code,
                value,
            } => {
                self.push_event(Event::ImeKeyEvent {
                    context_id,
                    key_serial,
                    window_id,
                    time,
                    type_,
                    code,
                    value,
                });
                true
            }
            ServerMessage::ImeReset { context_id, serial } => {
                self.push_event(Event::ImeReset { context_id, serial });
                true
            }
            ServerMessage::ImeTrigger {
                context_id,
                serial,
                trigger_id,
                code,
                time,
            } => {
                self.push_event(Event::ImeTrigger {
                    context_id,
                    serial,
                    trigger_id,
                    code,
                    time,
                });
                true
            }
            ServerMessage::SgfxFrameRejected {
                window_id,
                buffer_id,
                generation,
                compositor_epoch,
                commit_serial,
                code,
            } => {
                self.push_event(Event::SgfxFrameRejected {
                    window_id,
                    buffer_id,
                    generation,
                    compositor_epoch,
                    commit_serial,
                    code,
                });
                true
            }
            ServerMessage::SgfxBufferReleased {
                window_id,
                buffer_id,
                generation,
                compositor_epoch,
                commit_serial,
            } => {
                self.push_event(Event::SgfxBufferReleased {
                    window_id,
                    buffer_id,
                    generation,
                    compositor_epoch,
                    commit_serial,
                });
                true
            }
            ServerMessage::SgfxBackendLost { compositor_epoch } => {
                self.push_event(Event::SgfxBackendLost { compositor_epoch });
                true
            }
            ServerMessage::WindowDestroyed { window_id } => {
                self.push_event(Event::SurfaceDestroyed {
                    surface_id: window_id,
                });
                true
            }
            ServerMessage::WindowResized { .. } => false,
            ServerMessage::WindowConfigure {
                window_id,
                width,
                height,
            } => {
                self.push_event(Event::SurfaceConfigure {
                    surface_id: window_id,
                    width,
                    height,
                });
                true
            }
            ServerMessage::ScreenSizeChanged { width, height } => {
                self.push_event(Event::ScreenSizeChanged { width, height });
                true
            }
            ServerMessage::OutputScaleChanged { scale_milli } => {
                self.push_event(Event::OutputScaleChanged { scale_milli });
                true
            }
            ServerMessage::Error { code } => {
                self.push_event(Event::Error { code });
                true
            }
            ServerMessage::FocusChanged {
                window_id,
                app_id,
                app_id_len,
                app_name,
                app_name_len,
                title,
                title_len,
                menu_titles,
                menu_titles_len,
            } => {
                let app_id = String::from_utf8_lossy(&app_id[..app_id_len as usize]).into_owned();
                let app_name =
                    String::from_utf8_lossy(&app_name[..app_name_len as usize]).into_owned();
                let title = String::from_utf8_lossy(&title[..title_len as usize]).into_owned();
                let menu_titles =
                    String::from_utf8_lossy(&menu_titles[..menu_titles_len as usize]).into_owned();
                self.push_event(Event::FocusChanged {
                    window_id,
                    app_id,
                    app_name,
                    title,
                    menu_titles,
                });
                true
            }
            ServerMessage::ActiveAppChanged {
                window_id,
                app_id,
                app_id_len,
                app_name,
                app_name_len,
                title,
                title_len,
                menu_titles,
                menu_titles_len,
            } => {
                let app_id = String::from_utf8_lossy(&app_id[..app_id_len as usize]).into_owned();
                let app_name =
                    String::from_utf8_lossy(&app_name[..app_name_len as usize]).into_owned();
                let title = String::from_utf8_lossy(&title[..title_len as usize]).into_owned();
                let menu_titles =
                    String::from_utf8_lossy(&menu_titles[..menu_titles_len as usize]).into_owned();
                self.push_event(Event::ActiveAppChanged {
                    window_id,
                    app_id,
                    app_name,
                    title,
                    menu_titles,
                });
                true
            }
            ServerMessage::MenuItemActivated {
                window_id,
                menu_item_id,
                menu_item_id_len,
            } => {
                let menu_item_id =
                    String::from_utf8_lossy(&menu_item_id[..menu_item_id_len as usize])
                        .into_owned();
                self.push_event(Event::MenuItemActivated {
                    window_id,
                    menu_item_id,
                });
                true
            }
            _ => false,
        }
    }
}

impl Connection {
    /// Query the negotiated SWS protocol and compositor capabilities.
    ///
    /// # Returns
    ///
    /// The server capability snapshot associated with the current compositor
    /// epoch.
    pub fn get_capabilities(&self) -> Result<Capabilities, Error> {
        let response = self.request(protocol::client_msg::GET_CAPABILITIES, &[])?;
        match response.message() {
            ServerMessage::Capabilities {
                protocol_version,
                capabilities,
                compositor_epoch,
                compositor_backend,
            } => Ok(Capabilities {
                protocol_version,
                capabilities,
                compositor_epoch,
                compositor_backend,
            }),
            ServerMessage::Error { code } => Err(Error::ServerError(code)),
            _ => Err(Error::InvalidResponse),
        }
    }

    /// Register one shared SGFX image capability with SWS.
    ///
    /// The routed frame and image handle are transferred as one atomic socket
    /// record. The method returns only after SWS confirms the complete buffer
    /// identity.
    ///
    /// # Arguments
    ///
    /// * `identity` - Buffer identity scoped to this connection and window.
    /// * `width` - Image width in pixels.
    /// * `height` - Image height in pixels.
    /// * `handle` - Exported SGFX image capability.
    ///
    /// # Returns
    ///
    /// `Ok(())` after SWS imports the image and acknowledges the same identity.
    pub fn register_sgfx_buffer(
        &self,
        identity: SgfxBufferIdentity,
        width: u32,
        height: u32,
        handle: &Handle,
    ) -> Result<(), Error> {
        let payload = protocol::payload_register_sgfx_buffer(
            identity.window_id,
            identity.buffer_id,
            identity.generation,
            identity.compositor_epoch,
            width,
            height,
        );
        let token = self.send_request_with_handle(
            protocol::client_msg::REGISTER_SGFX_BUFFER,
            &payload,
            handle,
        )?;
        let response = self.wait_response(token)?;
        match response.message() {
            ServerMessage::SgfxBufferRegistered {
                window_id,
                buffer_id,
                generation,
                compositor_epoch,
            } if (SgfxBufferIdentity {
                window_id,
                buffer_id,
                generation,
                compositor_epoch,
            }) == identity =>
            {
                Ok(())
            }
            ServerMessage::Error { code } => Err(Error::ServerError(code)),
            _ => Err(Error::InvalidResponse),
        }
    }

    /// Enqueue one registered SGFX buffer and its damage list for commit.
    ///
    /// # Arguments
    ///
    /// * `identity` - Registered buffer to publish.
    /// * `commit_serial` - Non-zero serial uniquely identifying this buffer use.
    /// * `damage` - Non-empty bounded list of window-local damage rectangles.
    ///
    /// # Returns
    ///
    /// Success after the complete one-way commit frame is serialized. SWS
    /// reports semantic rejection and eventual release as asynchronous events.
    pub fn commit_sgfx_frame(
        &self,
        identity: SgfxBufferIdentity,
        commit_serial: u64,
        damage: &[protocol::SgfxDamageRect],
    ) -> Result<(), Error> {
        let payload = protocol::payload_commit_sgfx_frame(
            identity.window_id,
            identity.buffer_id,
            identity.generation,
            identity.compositor_epoch,
            commit_serial,
            damage,
        )
        .map_err(|_| Error::InvalidRequest)?;
        self.send_message(protocol::client_msg::COMMIT_SGFX_FRAME, &payload)
    }

    /// Destroy a released shared SGFX buffer registration.
    ///
    /// # Arguments
    ///
    /// * `identity` - Released buffer registration to remove.
    ///
    /// # Returns
    ///
    /// `Ok(())` after SWS confirms removal of the same identity.
    pub fn destroy_sgfx_buffer(&self, identity: SgfxBufferIdentity) -> Result<(), Error> {
        let payload = protocol::payload_sgfx_buffer_identity(
            identity.window_id,
            identity.buffer_id,
            identity.generation,
            identity.compositor_epoch,
        );
        let response = self.request(protocol::client_msg::DESTROY_SGFX_BUFFER, &payload)?;
        match response.message() {
            ServerMessage::SgfxBufferDestroyed {
                window_id,
                buffer_id,
                generation,
                compositor_epoch,
            } if (SgfxBufferIdentity {
                window_id,
                buffer_id,
                generation,
                compositor_epoch,
            }) == identity =>
            {
                Ok(())
            }
            ServerMessage::Error { code } => Err(Error::ServerError(code)),
            _ => Err(Error::InvalidResponse),
        }
    }

    /// Get the screen size.
    ///
    /// This is a synchronous request: it blocks until the server responds with SCREEN_SIZE.
    pub fn get_screen_size(&self) -> Result<(u32, u32), Error> {
        let response = self.request(protocol::client_msg::GET_SCREEN_SIZE, &[])?;
        match response.message() {
            ServerMessage::ScreenSize { width, height } => Ok((width, height)),
            _ => Err(Error::InvalidResponse),
        }
    }

    /// Get the output scale in milli-units.
    ///
    /// This is a synchronous request: it blocks until the server responds with OUTPUT_SCALE.
    pub fn get_output_scale(&self) -> Result<u32, Error> {
        let response = self.request(protocol::client_msg::GET_OUTPUT_SCALE, &[])?;
        match response.message() {
            ServerMessage::OutputScale { scale_milli } => Ok(scale_milli.max(1)),
            _ => Err(Error::InvalidResponse),
        }
    }

    /// Request an opaque token for transferring activation to another app.
    ///
    /// # Arguments
    ///
    /// * `source_window_id` - Focused window whose user interaction initiated the launch.
    /// * `target_app_id` - Stable application identifier expected on the new toplevel.
    ///
    /// # Returns
    ///
    /// A one-shot opaque token to pass to the target process.
    pub fn request_activation_token(
        &self,
        source_window_id: u32,
        target_app_id: &str,
    ) -> Result<String, Error> {
        if source_window_id == 0 || target_app_id.is_empty() {
            return Err(Error::InvalidRequest);
        }
        let payload =
            protocol::payload_request_activation_token(source_window_id, target_app_id.as_bytes());
        let response = self.request(protocol::client_msg::REQUEST_ACTIVATION_TOKEN, &payload)?;
        match response.message() {
            ServerMessage::ActivationToken { token, token_len } => {
                Ok(String::from_utf8_lossy(&token[..token_len as usize]).into_owned())
            }
            ServerMessage::Error { code } => Err(Error::ServerError(code)),
            _ => Err(Error::InvalidResponse),
        }
    }

    /// Get the list of all windows.
    ///
    /// This is a synchronous request: it blocks until the server responds with WINDOW_LIST.
    pub fn get_window_list(&self) -> Result<Vec<WindowListEntry>, Error> {
        let response = self.request(protocol::client_msg::GET_WINDOW_LIST, &[])?;
        match response.message() {
            ServerMessage::WindowList => {
                let windows = protocol::parse_window_list_payload(response.payload())
                    .map_err(|_| Error::InvalidResponse)?;

                Ok(windows
                    .into_iter()
                    .map(|w| WindowListEntry {
                        window_id: w.window_id,
                        app_id: w.app_id,
                        title: w.title,
                        window_type: w.window_type,
                        visible: w.visible,
                        focused: w.focused,
                        minimized: w.minimized,
                    })
                    .collect())
            }
            _ => Err(Error::InvalidResponse),
        }
    }

    /// Check if there are pending events
    pub fn has_events(&self) -> bool {
        let transport = mutex_lock(&self.transport);
        transport.pending_head < transport.pending_events.len()
    }
}
