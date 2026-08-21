//! Scarlet Window Server (SWS) IPC protocol.
//!
//! This crate is the single source of truth for both the SWS server (`sws`)
//! and clients (`sws_client`) for message IDs, framing, and parsing.
//!
//! Wire format
//! -----------
//! Each message is framed as:
//! - Header (8 bytes, little-endian)
//!   - `msg_type: u16`
//!   - `flags: u8`
//!   - `request_id: u8`
//!   - `payload_size: u32`
//! - Payload (`payload_size` bytes)
//!
//! See `docs/sws_ipc_protocol.md` for the detailed specification.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate scarlet_std as std;

use std::vec::Vec;

/// Maximum payload we accept from the socket.
///
/// This prevents unbounded allocations on malformed frames.
pub const MAX_PAYLOAD_SIZE: usize = 1024 * 1024; // 1 MiB

/// Current SWS capability-negotiation protocol version.
pub const SWS_PROTOCOL_VERSION: u32 = 3;

/// Maximum damage rectangles carried by one shared SGFX frame commit.
pub const SGFX_MAX_DAMAGE_RECTS: usize = 16;

/// Optional SWS capabilities returned by `GET_CAPABILITIES`.
pub mod capabilities {
    /// Shared SGFX images may be registered and committed without CPU copies.
    pub const SGFX_SHARED_IMAGE: u64 = 1 << 0;
    /// Focused windows may capture raw relative pointer motion.
    pub const POINTER_LOCK: u64 = 1 << 1;
    /// Windows may select a compositor-provided cursor icon.
    pub const CURSOR_ICONS: u64 = 1 << 2;
    /// System settings may replace the active filesystem-backed cursor theme.
    pub const CURSOR_THEMES: u64 = 1 << 3;
}

/// Compositor-provided mouse cursor icons.
///
/// The numeric discriminants are stable wire values used by `SET_CURSOR_ICON`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum CursorIcon {
    /// Standard arrow pointer.
    #[default]
    Arrow = 0,
    /// Link or other directly actionable content.
    Pointer = 1,
    /// Text selection and insertion.
    Text = 2,
    /// Precise point selection.
    Crosshair = 3,
    /// Movable content or an active window move.
    Move = 4,
    /// Vertical resize.
    ResizeNs = 5,
    /// Horizontal resize.
    ResizeEw = 6,
    /// Bottom-left to top-right diagonal resize.
    ResizeNesw = 7,
    /// Top-left to bottom-right diagonal resize.
    ResizeNwse = 8,
    /// Operation in progress.
    Wait = 9,
    /// Operation is not allowed at the current location.
    NotAllowed = 10,
    /// Context-sensitive help is available.
    Help = 11,
    /// Work continues in the background while interaction remains available.
    Progress = 12,
}

impl CursorIcon {
    /// All standard cursor icons in wire-value order.
    pub const ALL: [Self; 13] = [
        Self::Arrow,
        Self::Pointer,
        Self::Text,
        Self::Crosshair,
        Self::Move,
        Self::ResizeNs,
        Self::ResizeEw,
        Self::ResizeNesw,
        Self::ResizeNwse,
        Self::Wait,
        Self::NotAllowed,
        Self::Help,
        Self::Progress,
    ];

    /// Decode a stable protocol value.
    ///
    /// # Arguments
    ///
    /// * `value` - Raw `u32` received from the wire.
    ///
    /// # Returns
    ///
    /// The corresponding cursor icon, or `None` for an unknown value.
    pub const fn from_raw(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Arrow),
            1 => Some(Self::Pointer),
            2 => Some(Self::Text),
            3 => Some(Self::Crosshair),
            4 => Some(Self::Move),
            5 => Some(Self::ResizeNs),
            6 => Some(Self::ResizeEw),
            7 => Some(Self::ResizeNesw),
            8 => Some(Self::ResizeNwse),
            9 => Some(Self::Wait),
            10 => Some(Self::NotAllowed),
            11 => Some(Self::Help),
            12 => Some(Self::Progress),
            _ => None,
        }
    }

    /// Return the stable protocol value.
    ///
    /// # Returns
    ///
    /// The `u32` value serialized by `SET_CURSOR_ICON`.
    pub const fn as_raw(self) -> u32 {
        self as u32
    }
}

/// SWS compositor backend identifiers reported by capability negotiation.
pub mod compositor_backends {
    /// CPU compositor is active.
    pub const CPU: u32 = 0;
    /// SGFX compositor is active.
    pub const SGFX: u32 = 1;
}

/// Stable protocol error codes used by SGFX lifecycle requests.
pub mod error_codes {
    /// The selected SWS backend does not accept shared SGFX images.
    pub const SGFX_UNAVAILABLE: u32 = 100;
    /// The requesting connection does not own the target window.
    pub const WINDOW_NOT_OWNED: u32 = 101;
    /// The referenced SGFX buffer is unknown or has invalid metadata.
    pub const INVALID_SGFX_BUFFER: u32 = 102;
    /// The buffer generation or negotiated SWS SGFX epoch is stale.
    pub const STALE_SGFX_GENERATION: u32 = 103;
    /// The referenced SGFX buffer is still retained by SWS.
    pub const SGFX_BUFFER_BUSY: u32 = 104;
    /// SWS could not import the transferred GPU image capability.
    pub const SGFX_IMPORT_FAILED: u32 = 105;
    /// The requested activation did not originate from the focused window.
    pub const ACTIVATION_DENIED: u32 = 106;
    /// Another window already owns fullscreen state on the requested output.
    pub const FULLSCREEN_OCCUPIED: u32 = 107;
    /// Pointer lock was requested for a window not owned by the connection.
    pub const POINTER_LOCK_NOT_OWNED: u32 = 108;
    /// Pointer lock requires a visible, non-minimized, keyboard-focused window.
    pub const POINTER_LOCK_DENIED: u32 = 109;
    /// The requested cursor theme path or theme contents are invalid.
    pub const INVALID_CURSOR_THEME: u32 = 110;
    /// SWS loaded the cursor theme but could not persist the selection.
    pub const CURSOR_THEME_PERSIST_FAILED: u32 = 111;
}

/// Message type IDs (client -> server).
pub mod client_msg {
    pub const CREATE_WINDOW: u32 = 1;
    pub const DESTROY_WINDOW: u32 = 2;
    pub const SET_WINDOW_TITLE: u32 = 3;
    pub const UPDATE_BUFFER: u32 = 4;
    pub const REQUEST_MOVE_WINDOW: u32 = 5;
    pub const MOVE_WINDOW: u32 = 6;
    pub const SET_WINDOW_PARENT: u32 = 7;
    pub const SET_WINDOW_TRANSIENT_FLAGS: u32 = 8;
    pub const RESIZE_WINDOW: u32 = 9;
    pub const GET_SCREEN_SIZE: u32 = 10;
    pub const SET_WINDOW_SIZE_LIMITS: u32 = 16;
    pub const MINIMIZE_WINDOW: u32 = 17;
    pub const MAXIMIZE_WINDOW: u32 = 18;
    pub const RESTORE_WINDOW: u32 = 19;
    pub const SET_WINDOW_TYPE: u32 = 20;
    pub const SET_WINDOW_OPACITY: u32 = 21;

    // Extension API messages (100+)
    /// Register as an extension server (e.g., Wayland bridge)
    pub const REGISTER_EXTENSION: u32 = 100;
    /// Create a window on behalf of another client (extension-only)
    pub const EXTENSION_CREATE_WINDOW: u32 = 101;
    /// Update buffer on behalf of another client (extension-only)
    pub const EXTENSION_UPDATE_BUFFER: u32 = 102;
    /// Attach SHM buffer on behalf of another client (extension-only)
    pub const EXTENSION_ATTACH_BUFFER: u32 = 103;
    pub const SET_WORKAREA: u32 = 22;
    pub const SET_WINDOW_RESIZABLE: u32 = 23;
    pub const GET_WINDOW_LIST: u32 = 24;
    pub const LAUNCH_OR_FOCUS: u32 = 25;
    pub const FOCUS_WINDOW: u32 = 26;
    pub const GET_ACTIVE_APP: u32 = 27; // Get active app info for TaskBar
    pub const SET_WINDOW_HAS_ALPHA_CONTENT: u32 = 28; // Set whether window content has alpha channel
    pub const SET_WINDOW_MENU_TITLES: u32 = 29; // Update menu titles for a window
    pub const ACTIVATE_MENU_ITEM: u32 = 30; // Request menu item activation for a window
    pub const GET_OUTPUT_SCALE: u32 = 31; // Get output scale in milli-units (1000 = 1.0)

    // Shared SGFX buffer API (32-37)
    pub const GET_CAPABILITIES: u32 = 32;
    pub const REGISTER_SGFX_BUFFER: u32 = 33;
    pub const COMMIT_SGFX_FRAME: u32 = 34;
    pub const DESTROY_SGFX_BUFFER: u32 = 35;

    // Application activation (38-39)
    pub const REQUEST_ACTIVATION_TOKEN: u32 = 38;

    // Fullscreen window state (40-41)
    pub const SET_FULLSCREEN: u32 = 40;
    pub const UNSET_FULLSCREEN: u32 = 41;
    /// Capture or release raw relative pointer motion for an owned window.
    pub const SET_POINTER_LOCK: u32 = 42;
    /// Select a compositor-provided cursor for an owned window.
    pub const SET_CURSOR_ICON: u32 = 43;
    /// Validate, persist, and activate an installed cursor theme.
    pub const SET_CURSOR_THEME: u32 = 44;

    // Text input client API messages (200-219)
    pub const TEXT_INPUT_CREATE: u32 = 200;
    pub const TEXT_INPUT_DESTROY: u32 = 201;
    pub const TEXT_INPUT_ENABLE: u32 = 202;
    pub const TEXT_INPUT_DISABLE: u32 = 203;
    pub const TEXT_INPUT_SET_CURSOR_RECT: u32 = 204;
    pub const TEXT_INPUT_SET_SURROUNDING_TEXT: u32 = 205;
    pub const TEXT_INPUT_SET_CONTENT_TYPE: u32 = 206;
    pub const TEXT_INPUT_SET_TEXT_CHANGE_CAUSE: u32 = 207;
    pub const TEXT_INPUT_COMMIT_STATE: u32 = 208;
    pub const IME_GET_METHODS: u32 = 209;
    pub const IME_GET_ACTIVE: u32 = 210;

    // Input method service messages (220-239)
    pub const IME_REGISTER: u32 = 220;
    pub const IME_SET_ACTIVE: u32 = 221;
    pub const IME_KEY_HANDLED: u32 = 222;
    pub const IME_SET_PREEDIT: u32 = 223;
    pub const IME_COMMIT_TEXT: u32 = 224;
    pub const IME_DELETE_SURROUNDING_TEXT: u32 = 225;
    pub const IME_GRAB_KEYBOARD: u32 = 226;
    pub const IME_RELEASE_KEYBOARD: u32 = 227;
    pub const IME_SET_STATUS: u32 = 228;
    pub const IME_SET_POPUP_WINDOW: u32 = 229;
}

/// Message type IDs (server -> client).
pub mod server_msg {
    pub const WINDOW_CREATED: u32 = 10;
    pub const WINDOW_DESTROYED: u32 = 11;
    pub const INPUT_EVENT: u32 = 12;
    pub const ERROR: u32 = 13;
    pub const WINDOW_RESIZED: u32 = 14;
    pub const WINDOW_CONFIGURE: u32 = 15;

    // Extension API messages (100+)
    /// Confirmation that extension registration succeeded
    pub const EXTENSION_REGISTERED: u32 = 100;
    /// Forward input event to extension (for extension clients)
    pub const EXTENSION_INPUT_EVENT: u32 = 101;
    pub const SCREEN_SIZE: u32 = 16;
    pub const WINDOW_LIST: u32 = 17;
    pub const FOCUS_CHANGED: u32 = 18;
    pub const ACTIVE_APP: u32 = 19; // Response to GET_ACTIVE_APP
    pub const MENU_ITEM_ACTIVATED: u32 = 20; // Menu item activation for a window
    pub const ACTIVE_APP_CHANGED: u32 = 21; // Broadcast when active application changes (normal windows only)
    pub const SCREEN_SIZE_CHANGED: u32 = 22; // Broadcast when the display size changes
    pub const OUTPUT_SCALE: u32 = 23; // Response to GET_OUTPUT_SCALE
    pub const OUTPUT_SCALE_CHANGED: u32 = 24; // Broadcast when output scale changes

    // Shared SGFX buffer API (25-29)
    pub const CAPABILITIES: u32 = 25;
    pub const SGFX_BUFFER_REGISTERED: u32 = 26;
    pub const SGFX_FRAME_REJECTED: u32 = 27;
    pub const SGFX_BUFFER_RELEASED: u32 = 28;
    pub const SGFX_BACKEND_LOST: u32 = 29;
    pub const SGFX_BUFFER_DESTROYED: u32 = 30;
    pub const ACTIVATION_TOKEN: u32 = 31;
    pub const WINDOW_STATE_CHANGED: u32 = 32;
    /// Pointer lock state changed, including compositor-forced release.
    pub const POINTER_LOCK_CHANGED: u32 = 33;
    /// Confirmation that the active cursor theme changed.
    pub const CURSOR_THEME_CHANGED: u32 = 34;

    // Text input client events (200-219)
    pub const TEXT_INPUT_CREATED: u32 = 200;
    pub const TEXT_INPUT_PREEDIT: u32 = 201;
    pub const TEXT_INPUT_COMMIT: u32 = 202;
    pub const TEXT_INPUT_DELETE_SURROUNDING_TEXT: u32 = 203;
    pub const TEXT_INPUT_DONE: u32 = 204;
    pub const TEXT_INPUT_STATUS: u32 = 205;
    pub const IME_METHODS: u32 = 206;
    pub const IME_ACTIVE: u32 = 207;

    // Input method service events (220-239)
    pub const IME_REGISTERED: u32 = 220;
    pub const IME_ACTIVATE: u32 = 221;
    pub const IME_DEACTIVATE: u32 = 222;
    pub const IME_CONTEXT_STATE: u32 = 223;
    pub const IME_KEY_EVENT: u32 = 224;
    pub const IME_RESET: u32 = 225;
    pub const IME_TRIGGER: u32 = 226;
}

/// Maximum UTF-8 bytes carried by one text-input message.
pub const TEXT_INPUT_MAX_BYTES: usize = 1024;

/// Maximum UTF-8 bytes in one installed cursor theme path.
pub const CURSOR_THEME_PATH_MAX_BYTES: usize = 512;

/// Maximum bytes used for binary preedit span payloads.
pub const TEXT_INPUT_PREEDIT_SPANS_MAX_BYTES: usize = 512;

/// Maximum UTF-8 bytes carried by an opaque SWS activation token.
pub const ACTIVATION_TOKEN_MAX_BYTES: usize = 128;

/// IME service capabilities advertised by `IME_REGISTER`.
pub mod ime_capabilities {
    pub const KEYBOARD_GRAB: u32 = 1 << 0;
    pub const SURROUNDING_TEXT: u32 = 1 << 1;
    pub const DELETE_SURROUNDING_TEXT: u32 = 1 << 2;
    pub const STYLED_PREEDIT: u32 = 1 << 3;
    pub const STATUS: u32 = 1 << 4;
    pub const OWN_CANDIDATE_UI: u32 = 1 << 5;
    pub const PER_CONTEXT_STATE: u32 = 1 << 6;
}

/// Preedit span style flags.
pub mod preedit_style {
    pub const UNDERLINE: u32 = 1 << 0;
    pub const THICK_UNDERLINE: u32 = 1 << 1;
    pub const HIGHLIGHT: u32 = 1 << 2;
    pub const SELECTED: u32 = 1 << 3;
    pub const CONVERTED: u32 = 1 << 4;
    pub const TARGET_CONVERTING: u32 = 1 << 5;
    pub const ERROR: u32 = 1 << 6;
}

/// IME composition states.
pub mod ime_state {
    pub const DISABLED: u32 = 0;
    pub const DIRECT: u32 = 1;
    pub const COMPOSING: u32 = 2;
    pub const CANDIDATES: u32 = 3;
}

/// IME status flags.
///
/// Input modes are IME-local and must not be interpreted by SWS. Toolkits may
/// display the accompanying mode label, while the mode id is only stable within
/// the reporting IME.
pub mod ime_status_flags {
    pub const MODE_ACTIVE: u32 = 1 << 0;
    pub const PRIVATE_MODE: u32 = 1 << 1;
    pub const PREDICTION_ENABLED: u32 = 1 << 2;
    pub const CANDIDATES_VISIBLE: u32 = 1 << 3;
}

/// Registered input method entry serialized in `IME_METHODS` payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputMethodEntry {
    /// Server-assigned input method ID.
    pub ime_id: u32,
    /// Capability flags advertised by the input method.
    pub capabilities: u32,
    /// Whether this input method is currently active.
    pub active: bool,
    /// UTF-8 name bytes padded to the protocol maximum.
    pub name: [u8; TEXT_INPUT_MAX_BYTES],
    /// Number of valid bytes in `name`.
    pub name_len: u32,
}

/// Text input content hints.
pub mod text_input_content_hints {
    pub const NONE: u32 = 0;
    pub const COMPLETION: u32 = 1 << 0;
    pub const SPELLCHECK: u32 = 1 << 1;
    pub const AUTO_CAPITALIZATION: u32 = 1 << 2;
    pub const LOWERCASE: u32 = 1 << 3;
    pub const UPPERCASE: u32 = 1 << 4;
    pub const TITLECASE: u32 = 1 << 5;
    pub const HIDDEN_TEXT: u32 = 1 << 6;
    pub const SENSITIVE_DATA: u32 = 1 << 7;
    pub const LATIN: u32 = 1 << 8;
    pub const MULTILINE: u32 = 1 << 9;
}

/// Text input content purposes.
pub mod text_input_content_purpose {
    pub const NORMAL: u32 = 0;
    pub const ALPHA: u32 = 1;
    pub const DIGITS: u32 = 2;
    pub const NUMBER: u32 = 3;
    pub const PHONE: u32 = 4;
    pub const URL: u32 = 5;
    pub const EMAIL: u32 = 6;
    pub const NAME: u32 = 7;
    pub const PASSWORD: u32 = 8;
    pub const PIN: u32 = 9;
    pub const DATE: u32 = 10;
    pub const TIME: u32 = 11;
    pub const DATETIME: u32 = 12;
    pub const TERMINAL: u32 = 13;
}

/// Why surrounding text changed.
pub mod text_input_change_cause {
    pub const INPUT_METHOD: u32 = 0;
    pub const OTHER: u32 = 1;
}

/// IME trigger identifiers.
pub mod ime_trigger {
    pub const TOGGLE: u32 = 1;
}

/// Flags for transient (parent/child) window behavior.
///
/// These flags are interpreted by the compositor as *policy hints*.
pub mod transient_flags {
    /// If set, the child moves together when its parent is moved.
    pub const FOLLOW_PARENT_MOVE: u32 = 1 << 0;
    /// If set, raising the parent raises the child group.
    pub const RAISE_WITH_PARENT: u32 = 1 << 1;
}

/// Window type constants for Z-order management
pub mod window_types {
    /// Normal application window (default)
    pub const NORMAL: u32 = 0;
    /// Window that always stays on top
    pub const ALWAYS_ON_TOP: u32 = 1;
    /// Taskbar or panel window
    pub const TASKBAR: u32 = 2;
    /// Desktop background window
    pub const DESKTOP: u32 = 3;
    /// Input-method-owned popup surface anchored to the active text input.
    pub const IME_POPUP: u32 = 4;
}

/// Window presentation-state flags reported by `WINDOW_STATE_CHANGED`.
pub mod window_state {
    /// The window is minimized and is not currently visible.
    pub const MINIMIZED: u32 = 1 << 0;
    /// The window occupies the compositor workarea.
    pub const MAXIMIZED: u32 = 1 << 1;
    /// The window occupies the complete output and covers shell surfaces.
    pub const FULLSCREEN: u32 = 1 << 2;
}

/// Initial placement hints understood by the SWS window manager.
pub mod window_placement {
    /// Use the compositor's normal placement policy.
    pub const DEFAULT: u32 = 0;
    /// Center the window in the current workarea.
    pub const CENTERED: u32 = 1;
    /// Place the window at the supplied absolute coordinates.
    pub const ABSOLUTE: u32 = 2;
}

/// One window-local damage rectangle in an SGFX frame commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SgfxDamageRect {
    /// Horizontal origin in window-local pixels.
    pub x: i32,
    /// Vertical origin in window-local pixels.
    pub y: i32,
    /// Damage width in pixels.
    pub width: u32,
    /// Damage height in pixels.
    pub height: u32,
}

impl SgfxDamageRect {
    /// Create a window-local SGFX damage rectangle.
    ///
    /// # Arguments
    ///
    /// * `x` - Horizontal origin in pixels.
    /// * `y` - Vertical origin in pixels.
    /// * `width` - Width in pixels.
    /// * `height` - Height in pixels.
    ///
    /// # Returns
    ///
    /// A damage rectangle with the supplied coordinates.
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// Message header.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageHeader {
    /// Message type.
    pub msg_type: u16,
    /// Routing flags.
    pub flags: u8,
    /// Per-connection request identifier used to match synchronous responses.
    pub request_id: u8,
    /// Payload size in bytes.
    pub payload_size: u32,
}

impl MessageHeader {
    /// Size of the encoded header in bytes.
    pub const SIZE: usize = 8;
    /// Frame flag set on server responses to client requests.
    pub const FLAG_IS_RESPONSE: u8 = 1 << 0;

    /// Create an unrouted frame header.
    ///
    /// # Arguments
    ///
    /// * `msg_type` - Message type.
    /// * `payload_size` - Payload size in bytes.
    ///
    /// # Returns
    ///
    /// Header with no flags and request id 0.
    pub fn new(msg_type: u32, payload_size: u32) -> Self {
        Self::with_routing(msg_type, 0, 0, payload_size)
    }

    /// Create a request frame header.
    ///
    /// # Arguments
    ///
    /// * `msg_type` - Message type.
    /// * `request_id` - Per-connection request identifier.
    /// * `payload_size` - Payload size in bytes.
    ///
    /// # Returns
    ///
    /// Header with no response flag.
    pub fn request(msg_type: u32, request_id: u8, payload_size: u32) -> Self {
        Self::with_routing(msg_type, 0, request_id, payload_size)
    }

    /// Create a response frame header.
    ///
    /// # Arguments
    ///
    /// * `msg_type` - Message type.
    /// * `request_id` - Request identifier being answered.
    /// * `payload_size` - Payload size in bytes.
    ///
    /// # Returns
    ///
    /// Header with the response flag set.
    pub fn response(msg_type: u32, request_id: u8, payload_size: u32) -> Self {
        Self::with_routing(msg_type, Self::FLAG_IS_RESPONSE, request_id, payload_size)
    }

    /// Create a frame header with explicit routing metadata.
    ///
    /// # Arguments
    ///
    /// * `msg_type` - Message type.
    /// * `flags` - Routing flags.
    /// * `request_id` - Per-connection request identifier.
    /// * `payload_size` - Payload size in bytes.
    ///
    /// # Returns
    ///
    /// Header containing the supplied routing metadata.
    pub fn with_routing(msg_type: u32, flags: u8, request_id: u8, payload_size: u32) -> Self {
        Self {
            msg_type: msg_type as u16,
            flags,
            request_id,
            payload_size,
        }
    }

    /// Return the message type as `u32` for compatibility with message constants.
    ///
    /// # Returns
    ///
    /// Message type widened to `u32`.
    pub fn msg_type_u32(self) -> u32 {
        self.msg_type as u32
    }

    /// Check whether this frame is a response.
    ///
    /// # Returns
    ///
    /// `true` when the response flag is set.
    pub fn is_response(self) -> bool {
        (self.flags & Self::FLAG_IS_RESPONSE) != 0
    }

    /// Encode this header as little-endian bytes.
    ///
    /// # Returns
    ///
    /// Encoded 8-byte header.
    pub fn to_le_bytes(self) -> [u8; Self::SIZE] {
        let mut out = [0u8; Self::SIZE];
        out[0..2].copy_from_slice(&self.msg_type.to_le_bytes());
        out[2] = self.flags;
        out[3] = self.request_id;
        out[4..8].copy_from_slice(&self.payload_size.to_le_bytes());
        out
    }

    /// Decode a little-endian header.
    ///
    /// # Arguments
    ///
    /// * `bytes` - Encoded 8-byte header.
    ///
    /// # Returns
    ///
    /// Decoded message header.
    pub fn from_le_bytes(bytes: [u8; Self::SIZE]) -> Self {
        let msg_type = u16::from_le_bytes([bytes[0], bytes[1]]);
        let flags = bytes[2];
        let request_id = bytes[3];
        let payload_size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        Self {
            msg_type,
            flags,
            request_id,
            payload_size,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolError {
    /// Frame payload is too large.
    PayloadTooLarge,
    /// Malformed payload for the given message type.
    MalformedPayload,
    /// Unknown message type.
    UnknownMessageType,
}

/// Encode a full framed message (header + payload) into a single buffer.
///
/// This is a protocol-only helper; actual I/O is implemented by server/client code.
pub fn encode_frame(msg_type: u32, payload: &[u8]) -> Vec<u8> {
    let header = MessageHeader::new(msg_type, payload.len() as u32);
    let mut out = Vec::new();
    out.extend_from_slice(&header.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Encode a routed framed message (header + payload).
pub fn encode_routed_frame(msg_type: u32, flags: u8, request_id: u8, payload: &[u8]) -> Vec<u8> {
    let header = MessageHeader::with_routing(msg_type, flags, request_id, payload.len() as u32);
    let mut out = Vec::new();
    out.extend_from_slice(&header.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn read_u32(payload: &[u8], offset: usize) -> Result<u32, ProtocolError> {
    if payload.len() < offset + 4 {
        return Err(ProtocolError::MalformedPayload);
    }
    Ok(u32::from_le_bytes([
        payload[offset],
        payload[offset + 1],
        payload[offset + 2],
        payload[offset + 3],
    ]))
}

fn read_i32(payload: &[u8], offset: usize) -> Result<i32, ProtocolError> {
    Ok(read_u32(payload, offset)? as i32)
}

fn read_u64(payload: &[u8], offset: usize) -> Result<u64, ProtocolError> {
    if payload.len() < offset + 8 {
        return Err(ProtocolError::MalformedPayload);
    }
    Ok(u64::from_le_bytes([
        payload[offset],
        payload[offset + 1],
        payload[offset + 2],
        payload[offset + 3],
        payload[offset + 4],
        payload[offset + 5],
        payload[offset + 6],
        payload[offset + 7],
    ]))
}

fn read_u16(payload: &[u8], offset: usize) -> Result<u16, ProtocolError> {
    if payload.len() < offset + 2 {
        return Err(ProtocolError::MalformedPayload);
    }
    Ok(u16::from_le_bytes([payload[offset], payload[offset + 1]]))
}

fn read_len_prefixed_bytes<'a>(
    payload: &'a [u8],
    offset: usize,
    max_len: usize,
) -> Result<&'a [u8], ProtocolError> {
    let len = read_u32(payload, offset)? as usize;
    let start = offset + 4;
    if len > max_len || payload.len() != start + len {
        return Err(ProtocolError::MalformedPayload);
    }
    Ok(&payload[start..])
}

fn copy_bounded<const N: usize>(bytes: &[u8]) -> Result<([u8; N], u32), ProtocolError> {
    if bytes.len() > N {
        return Err(ProtocolError::MalformedPayload);
    }
    let mut out = [0u8; N];
    if !bytes.is_empty() {
        out[..bytes.len()].copy_from_slice(bytes);
    }
    Ok((out, bytes.len() as u32))
}

/// Parse an `IME_METHODS` payload.
///
/// # Arguments
///
/// * `payload` - Serialized input method list payload.
///
/// # Returns
///
/// Parsed input method entries.
pub fn parse_ime_methods_payload(payload: &[u8]) -> Result<Vec<InputMethodEntry>, ProtocolError> {
    if payload.len() < 4 {
        return Err(ProtocolError::MalformedPayload);
    }

    let count = read_u32(payload, 0)? as usize;
    let mut offset = 4;
    let mut entries = Vec::new();

    for _ in 0..count {
        let ime_id = read_u32(payload, offset)?;
        let capabilities = read_u32(payload, offset + 4)?;
        let active = read_u32(payload, offset + 8)? != 0;
        let name_len = read_u32(payload, offset + 12)? as usize;
        offset += 16;

        if name_len > TEXT_INPUT_MAX_BYTES || payload.len() < offset + name_len {
            return Err(ProtocolError::MalformedPayload);
        }

        let name_len_usize = name_len;
        let (name, name_len) = copy_bounded(&payload[offset..offset + name_len_usize])?;
        entries.push(InputMethodEntry {
            ime_id,
            capabilities,
            active,
            name,
            name_len,
        });
        offset += name_len_usize;
    }

    if offset != payload.len() {
        return Err(ProtocolError::MalformedPayload);
    }

    Ok(entries)
}

/// Parse an `IME_ACTIVE` payload.
///
/// # Arguments
///
/// * `payload` - Serialized active input method payload.
///
/// # Returns
///
/// The active input method, or `None` when no input method is active.
pub fn parse_ime_active_payload(payload: &[u8]) -> Result<Option<InputMethodEntry>, ProtocolError> {
    let entries = parse_ime_methods_payload(payload)?;
    match entries.len() {
        0 => Ok(None),
        1 => Ok(Some(entries[0])),
        _ => Err(ProtocolError::MalformedPayload),
    }
}

/// Initial placement requested by a client when creating a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowPlacement {
    /// Let the compositor choose the initial position.
    Default,
    /// Center the window in the current workarea.
    Centered,
    /// Request an absolute screen position.
    Absolute { x: i32, y: i32 },
}

/// Borrowed client->server messages (payload may be borrowed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientMessageRef<'a> {
    CreateWindow {
        app_id: &'a [u8],
        app_name: &'a [u8],
        menu_titles: &'a [u8], // Format: "menu1|menu2|menu3"
        width: u32,
        height: u32,
        window_type: u32, // Window type (0=Normal, 1=AlwaysOnTop, 2=Taskbar, 3=Desktop)
        resizable: bool,
        focus_on_create: bool,
        active_on_focus: bool,
        initial_position: WindowPlacement,
        activation_token: Option<&'a [u8]>,
    },
    DestroyWindow {
        window_id: u32,
    },
    SetWindowTitle {
        window_id: u32,
        title: &'a [u8],
    },
    UpdateBuffer {
        window_id: u32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    RequestMoveWindow {
        window_id: u32,
    },
    MoveWindow {
        window_id: u32,
        x: i32,
        y: i32,
    },
    /// Set (or clear) the logical parent of a window.
    ///
    /// `parent_id == 0` means "no parent".
    SetWindowParent {
        window_id: u32,
        parent_id: u32,
    },

    /// Configure transient behavior flags for a window.
    ///
    /// Flags are a bitset from `transient_flags::*`.
    SetWindowTransientFlags {
        window_id: u32,
        flags: u32,
    },

    /// Resize a window buffer.
    ///
    /// This triggers the server to allocate a new shared-memory buffer and
    /// respond with `WINDOW_RESIZED` + a new SHM handle.
    ResizeWindow {
        window_id: u32,
        width: u32,
        height: u32,
    },

    /// Set min/max size constraints for a window.
    ///
    /// Values are in pixels.
    /// - `min_* == 0` means "no minimum".
    /// - `max_* == 0` means "no maximum".
    SetWindowSizeLimits {
        window_id: u32,
        min_width: u32,
        min_height: u32,
        max_width: u32,
        max_height: u32,
    },

    /// Minimize a window (hide but keep in window list)
    MinimizeWindow {
        window_id: u32,
    },

    /// Maximize a window to screen dimensions
    MaximizeWindow {
        window_id: u32,
    },

    /// Restore a window from minimized or maximized state
    RestoreWindow {
        window_id: u32,
    },

    /// Enter output-filling fullscreen state.
    SetFullscreen {
        window_id: u32,
    },

    /// Leave fullscreen and restore the preceding window state.
    UnsetFullscreen {
        window_id: u32,
    },

    /// Capture or release raw relative pointer motion.
    SetPointerLock {
        window_id: u32,
        locked: bool,
    },

    /// Select the cursor shown while the pointer is over an owned window.
    SetCursorIcon {
        window_id: u32,
        icon: CursorIcon,
    },

    /// Replace the global cursor theme with one installed below
    /// `/share/cursors`.
    SetCursorTheme {
        theme_path: &'a [u8],
    },

    /// Set window type for Z-order management
    /// Type: 0 = Normal, 1 = AlwaysOnTop, 2 = Taskbar, 3 = Desktop
    SetWindowType {
        window_id: u32,
        window_type: u32,
    },

    /// Set window opacity (0-255, where 255 is fully opaque)
    SetWindowOpacity {
        window_id: u32,
        opacity: u8,
    },

    // Extension API messages (100+)
    /// Register as an extension server
    RegisterExtension {
        extension_name: &'a [u8],
    },

    /// Create a window on behalf of an external client (extension-only)
    ExtensionCreateWindow {
        external_client_id: u32,
        width: u32,
        height: u32,
    },

    /// Update buffer on behalf of an external client (extension-only)
    ExtensionUpdateBuffer {
        external_client_id: u32,
        window_id: u32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },

    /// Attach a shared-memory buffer on behalf of an external client (extension-only)
    ExtensionAttachBuffer {
        external_client_id: u32,
        window_id: u32,
        width: u32,
        height: u32,
        offset: i32,
        stride: i32,
        format: u32,
        shm_size: u64,
    },

    /// Set the workarea (usable screen area) for the window manager
    ///
    /// This is typically sent by the taskbar to inform the window manager
    /// about the area where normal windows should be placed.
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
    GetScreenSize {},

    /// Get the output scale in milli-units.
    ///
    /// `1000` means 1.0, `2000` means 2.0.
    GetOutputScale {},

    /// Get list of all windows
    GetWindowList {},

    /// Request an opaque token for activating a newly launched application.
    RequestActivationToken {
        source_window_id: u32,
        target_app_id: &'a [u8],
    },

    /// Query protocol version and optional server capabilities.
    GetCapabilities {},

    /// Register one transferred SGFX image capability for a window.
    RegisterSgfxBuffer {
        window_id: u32,
        buffer_id: u32,
        generation: u32,
        compositor_epoch: u32,
        width: u32,
        height: u32,
    },

    /// Atomically commit a registered SGFX buffer and its damage rectangles.
    CommitSgfxFrame {
        window_id: u32,
        buffer_id: u32,
        generation: u32,
        compositor_epoch: u32,
        commit_serial: u64,
        damage_rects: &'a [u8],
    },

    /// Remove a registered SGFX buffer after it has been released.
    DestroySgfxBuffer {
        window_id: u32,
        buffer_id: u32,
        generation: u32,
        compositor_epoch: u32,
    },

    /// Launch an application or focus an existing window
    ///
    /// If a window with the given app_id already exists, focus it.
    /// Otherwise, launch the specified application.
    LaunchOrFocus {
        app_id: &'a [u8],
        exec_path: &'a [u8],
    },

    /// Focus and raise a specific window.
    FocusWindow {
        window_id: u32,
    },

    /// Get active application information (for TaskBar)
    GetActiveApp {},

    /// Set whether window content contains alpha channel (semi-transparent pixels)
    ///
    /// This is separate from window.opacity - this controls whether pixel alpha
    /// values in the window buffer should be respected during composition.
    SetWindowHasAlphaContent {
        window_id: u32,
        has_alpha: bool,
    },
    SetWindowMenuTitles {
        window_id: u32,
        menu_titles: &'a [u8], // Format: "menu1|menu2|menu3"
    },
    ActivateMenuItem {
        window_id: u32,
        menu_item_id: &'a [u8],
    },
    TextInputCreate {
        window_id: u32,
        seat_id: u32,
    },
    TextInputDestroy {
        context_id: u32,
    },
    TextInputEnable {
        context_id: u32,
    },
    TextInputDisable {
        context_id: u32,
    },
    TextInputSetCursorRect {
        context_id: u32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    TextInputSetSurroundingText {
        context_id: u32,
        cursor_byte: u32,
        anchor_byte: u32,
        text: &'a [u8],
    },
    TextInputSetContentType {
        context_id: u32,
        hint: u32,
        purpose: u32,
    },
    TextInputSetTextChangeCause {
        context_id: u32,
        cause: u32,
    },
    TextInputCommitState {
        context_id: u32,
        serial: u32,
    },
    ImeGetMethods {},
    ImeGetActive {},
    ImeRegister {
        name: &'a [u8],
        capabilities: u32,
    },
    ImeSetActive {
        ime_id: u32,
    },
    ImeKeyHandled {
        key_serial: u32,
        handled: bool,
    },
    ImeSetPreedit {
        context_id: u32,
        cursor_byte: u32,
        anchor_byte: u32,
        text: &'a [u8],
        spans: &'a [u8],
    },
    ImeCommitText {
        context_id: u32,
        text: &'a [u8],
    },
    ImeDeleteSurroundingText {
        context_id: u32,
        before_bytes: u32,
        after_bytes: u32,
    },
    ImeGrabKeyboard {
        context_id: u32,
    },
    ImeReleaseKeyboard {
        context_id: u32,
    },
    ImeSetStatus {
        context_id: u32,
        state: u32,
        mode_id: u32,
        flags: u32,
        mode_label: &'a [u8],
    },
    ImeSetPopupWindow {
        context_id: u32,
        window_id: u32,
        offset_x: i32,
        offset_y: i32,
        visible: bool,
    },
}

/// Server->client messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerMessage {
    WindowCreated {
        window_id: u32,
        shm_size: u64,
    },
    WindowDestroyed {
        window_id: u32,
    },
    /// Server acknowledged a resize and provides the new SHM size.
    ///
    /// The server will send the new SHM handle out-of-band immediately after.
    WindowResized {
        window_id: u32,
        shm_size: u64,
        width: u32,
        height: u32,
    },
    /// Compositor requests the client to resize to the given dimensions.
    ///
    /// This does not include a new SHM handle; clients should respond by
    /// issuing a `RESIZE_WINDOW` request.
    WindowConfigure {
        window_id: u32,
        width: u32,
        height: u32,
    },
    /// Compositor-confirmed window presentation state.
    WindowStateChanged {
        window_id: u32,
        state_flags: u32,
    },
    /// Compositor-confirmed pointer lock state.
    PointerLockChanged {
        window_id: u32,
        locked: bool,
    },
    /// The requested global cursor theme was loaded and persisted.
    CursorThemeChanged,
    InputEvent {
        window_id: u32,
        time: u64,
        type_: u16,
        code: u16,
        value: i32,
    },
    TextInputCreated {
        context_id: u32,
        serial: u32,
    },
    TextInputPreedit {
        context_id: u32,
        serial: u32,
        cursor_byte: u32,
        anchor_byte: u32,
        text: [u8; TEXT_INPUT_MAX_BYTES],
        text_len: u32,
        spans: [u8; TEXT_INPUT_PREEDIT_SPANS_MAX_BYTES],
        spans_len: u32,
    },
    TextInputCommit {
        context_id: u32,
        serial: u32,
        text: [u8; TEXT_INPUT_MAX_BYTES],
        text_len: u32,
    },
    TextInputDeleteSurroundingText {
        context_id: u32,
        serial: u32,
        before_bytes: u32,
        after_bytes: u32,
    },
    TextInputDone {
        context_id: u32,
        serial: u32,
    },
    TextInputStatus {
        context_id: u32,
        serial: u32,
        state: u32,
        mode_id: u32,
        flags: u32,
        mode_label: [u8; TEXT_INPUT_MAX_BYTES],
        mode_label_len: u32,
    },
    ImeRegistered {
        ime_id: u32,
    },
    /// Contains a serialized list of registered input methods.
    ImeMethods,
    /// Contains either no active input method or one serialized entry.
    ImeActive,
    ImeActivate {
        context_id: u32,
        window_id: u32,
        serial: u32,
        cursor_x: i32,
        cursor_y: i32,
        cursor_width: u32,
        cursor_height: u32,
        content_hint: u32,
        content_purpose: u32,
        text_change_cause: u32,
        cursor_byte: u32,
        anchor_byte: u32,
        surrounding_text: [u8; TEXT_INPUT_MAX_BYTES],
        surrounding_text_len: u32,
    },
    ImeDeactivate {
        context_id: u32,
        serial: u32,
    },
    ImeContextState {
        context_id: u32,
        window_id: u32,
        serial: u32,
        cursor_x: i32,
        cursor_y: i32,
        cursor_width: u32,
        cursor_height: u32,
        content_hint: u32,
        content_purpose: u32,
        text_change_cause: u32,
        cursor_byte: u32,
        anchor_byte: u32,
        surrounding_text: [u8; TEXT_INPUT_MAX_BYTES],
        surrounding_text_len: u32,
    },
    ImeKeyEvent {
        context_id: u32,
        key_serial: u32,
        window_id: u32,
        time: u64,
        type_: u16,
        code: u16,
        value: i32,
    },
    ImeReset {
        context_id: u32,
        serial: u32,
    },
    ImeTrigger {
        context_id: u32,
        serial: u32,
        trigger_id: u32,
        code: u16,
        time: u64,
    },
    /// Response to GET_SCREEN_SIZE request
    ScreenSize {
        width: u32,
        height: u32,
    },
    /// Display size changed asynchronously.
    ScreenSizeChanged {
        width: u32,
        height: u32,
    },
    /// Response to GET_OUTPUT_SCALE request.
    ///
    /// Scale is encoded in milli-units: `1000` means 1.0, `2000` means 2.0.
    OutputScale {
        scale_milli: u32,
    },
    /// Output scale changed asynchronously.
    ///
    /// Scale is encoded in milli-units: `1000` means 1.0, `2000` means 2.0.
    OutputScaleChanged {
        scale_milli: u32,
    },
    /// Response containing the negotiated SWS version and capability mask.
    Capabilities {
        protocol_version: u32,
        capabilities: u64,
        compositor_epoch: u32,
        compositor_backend: u32,
    },
    /// Confirmation that a transferred SGFX image was imported.
    SgfxBufferRegistered {
        window_id: u32,
        buffer_id: u32,
        generation: u32,
        compositor_epoch: u32,
    },
    /// Asynchronous notification that one SGFX frame commit was rejected.
    SgfxFrameRejected {
        window_id: u32,
        buffer_id: u32,
        generation: u32,
        compositor_epoch: u32,
        commit_serial: u64,
        code: u32,
    },
    /// Asynchronous notification that SWS no longer retains a buffer.
    SgfxBufferReleased {
        window_id: u32,
        buffer_id: u32,
        generation: u32,
        compositor_epoch: u32,
        commit_serial: u64,
    },
    /// Asynchronous notification that shared SGFX composition is unavailable.
    SgfxBackendLost {
        compositor_epoch: u32,
    },
    /// Confirmation that SWS dropped a registered SGFX image capability.
    SgfxBufferDestroyed {
        window_id: u32,
        buffer_id: u32,
        generation: u32,
        compositor_epoch: u32,
    },
    /// Opaque token issued for one application activation.
    ActivationToken {
        token: [u8; ACTIVATION_TOKEN_MAX_BYTES],
        token_len: u32,
    },
    /// Response to GET_WINDOW_LIST request
    /// Contains a serialized list of windows
    WindowList,
    /// Focus changed to a different window (includes app info for TaskBar)
    FocusChanged {
        window_id: u32,
        app_id: [u8; 128],
        app_id_len: u32,
        app_name: [u8; 128],
        app_name_len: u32,
        title: [u8; 256],
        title_len: u32,
        menu_titles: [u8; 2048], // Format: "menu1|menu2|menu3"
        menu_titles_len: u32,
    },
    /// Active application changed (normal window gained focus)
    /// Broadcast to all clients for TaskBar menu updates
    ActiveAppChanged {
        window_id: u32,
        app_id: [u8; 128],
        app_id_len: u32,
        app_name: [u8; 128],
        app_name_len: u32,
        title: [u8; 256],
        title_len: u32,
        menu_titles: [u8; 2048], // Format: "menu1|menu2|menu3"
        menu_titles_len: u32,
    },
    /// Active application information (response to GET_ACTIVE_APP)
    ActiveApp {
        app_id: [u8; 128],
        app_id_len: u32,
        app_name: [u8; 128],
        app_name_len: u32,
        menu_titles: [u8; 2048], // Format: "menu1|menu2|menu3"
        menu_titles_len: u32,
    },
    MenuItemActivated {
        window_id: u32,
        menu_item_id: [u8; 128],
        menu_item_id_len: u32,
    },
    Error {
        code: u32,
    },

    // Extension API messages (100+)
    /// Extension registration successful
    ExtensionRegistered {
        extension_id: u32,
    },

    /// Input event for extension-managed window
    ExtensionInputEvent {
        external_client_id: u32,
        window_id: u32,
        time: u64,
        type_: u16,
        code: u16,
        value: i32,
    },
}

/// Parse a client->server message from `(msg_type, payload)`.
pub fn parse_client_message<'a>(
    msg_type: u32,
    payload: &'a [u8],
) -> Result<ClientMessageRef<'a>, ProtocolError> {
    match msg_type {
        client_msg::CREATE_WINDOW => {
            // Payload: app_id_len (u32) + app_id_bytes + app_name_len (u32) + app_name_bytes
            //          + menu_titles_len (u32) + menu_titles_bytes + width (u32) + height (u32)
            //          + window_type (u32) + resizable (u32, optional)
            if payload.len() < 12 {
                return Err(ProtocolError::MalformedPayload);
            }
            let app_id_len =
                u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;

            let mut offset = 4 + app_id_len;
            if payload.len() < offset + 4 {
                return Err(ProtocolError::MalformedPayload);
            }

            let app_name_len = u32::from_le_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]) as usize;
            offset += 4 + app_name_len;

            if payload.len() < offset + 4 {
                return Err(ProtocolError::MalformedPayload);
            }

            let menu_titles_len = u32::from_le_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]) as usize;
            offset += 4 + menu_titles_len;

            if payload.len() != offset + 12
                && payload.len() != offset + 16
                && payload.len() != offset + 24
                && payload.len() != offset + 32
                && payload.len() != offset + 36
                && payload.len() < offset + 40
            {
                return Err(ProtocolError::MalformedPayload);
            }

            let app_id = &payload[4..4 + app_id_len];
            let app_name = &payload[4 + app_id_len + 4..4 + app_id_len + 4 + app_name_len];
            let menu_titles = &payload[4 + app_id_len + 4 + app_name_len + 4
                ..4 + app_id_len + 4 + app_name_len + 4 + menu_titles_len];
            let width = u32::from_le_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]);
            let height = u32::from_le_bytes([
                payload[offset + 4],
                payload[offset + 5],
                payload[offset + 6],
                payload[offset + 7],
            ]);
            let window_type = u32::from_le_bytes([
                payload[offset + 8],
                payload[offset + 9],
                payload[offset + 10],
                payload[offset + 11],
            ]);
            let resizable = if payload.len() == offset + 16
                || payload.len() == offset + 24
                || payload.len() == offset + 32
                || payload.len() == offset + 36
                || payload.len() >= offset + 40
            {
                u32::from_le_bytes([
                    payload[offset + 12],
                    payload[offset + 13],
                    payload[offset + 14],
                    payload[offset + 15],
                ]) != 0
            } else {
                true
            };
            let mut focus_on_create = true;
            let mut active_on_focus = window_type == window_types::NORMAL;
            if payload.len() == offset + 24
                || payload.len() == offset + 32
                || payload.len() == offset + 36
                || payload.len() >= offset + 40
            {
                focus_on_create = u32::from_le_bytes([
                    payload[offset + 16],
                    payload[offset + 17],
                    payload[offset + 18],
                    payload[offset + 19],
                ]) != 0;
                active_on_focus = u32::from_le_bytes([
                    payload[offset + 20],
                    payload[offset + 21],
                    payload[offset + 22],
                    payload[offset + 23],
                ]) != 0;
            }
            let initial_position = if payload.len() == offset + 32 {
                let x = i32::from_le_bytes([
                    payload[offset + 24],
                    payload[offset + 25],
                    payload[offset + 26],
                    payload[offset + 27],
                ]);
                let y = i32::from_le_bytes([
                    payload[offset + 28],
                    payload[offset + 29],
                    payload[offset + 30],
                    payload[offset + 31],
                ]);
                WindowPlacement::Absolute { x, y }
            } else if payload.len() == offset + 36 || payload.len() >= offset + 40 {
                let placement = u32::from_le_bytes([
                    payload[offset + 24],
                    payload[offset + 25],
                    payload[offset + 26],
                    payload[offset + 27],
                ]);
                let x = i32::from_le_bytes([
                    payload[offset + 28],
                    payload[offset + 29],
                    payload[offset + 30],
                    payload[offset + 31],
                ]);
                let y = i32::from_le_bytes([
                    payload[offset + 32],
                    payload[offset + 33],
                    payload[offset + 34],
                    payload[offset + 35],
                ]);
                match placement {
                    window_placement::DEFAULT => WindowPlacement::Default,
                    window_placement::CENTERED => WindowPlacement::Centered,
                    window_placement::ABSOLUTE => WindowPlacement::Absolute { x, y },
                    _ => return Err(ProtocolError::MalformedPayload),
                }
            } else {
                WindowPlacement::Default
            };
            let activation_token = if payload.len() >= offset + 40 {
                let token_len = read_u32(payload, offset + 36)? as usize;
                if token_len == 0
                    || token_len > ACTIVATION_TOKEN_MAX_BYTES
                    || payload.len() != offset + 40 + token_len
                {
                    return Err(ProtocolError::MalformedPayload);
                }
                Some(&payload[offset + 40..])
            } else {
                None
            };
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
            })
        }
        client_msg::DESTROY_WINDOW => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ClientMessageRef::DestroyWindow { window_id })
        }
        client_msg::SET_WINDOW_TITLE => {
            if payload.len() < 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let title_len =
                u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;
            if payload.len() != 8 + title_len {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::SetWindowTitle {
                window_id,
                title: &payload[8..],
            })
        }
        client_msg::UPDATE_BUFFER => {
            if payload.len() != 20 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let x = i32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            let y = i32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
            let width = u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
            let height = u32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]);
            Ok(ClientMessageRef::UpdateBuffer {
                window_id,
                x,
                y,
                width,
                height,
            })
        }
        client_msg::REQUEST_MOVE_WINDOW => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ClientMessageRef::RequestMoveWindow { window_id })
        }
        client_msg::MOVE_WINDOW => {
            if payload.len() != 12 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let x = i32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            let y = i32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
            Ok(ClientMessageRef::MoveWindow { window_id, x, y })
        }
        client_msg::SET_WINDOW_PARENT => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let parent_id = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            Ok(ClientMessageRef::SetWindowParent {
                window_id,
                parent_id,
            })
        }
        client_msg::SET_WINDOW_TRANSIENT_FLAGS => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let flags = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            Ok(ClientMessageRef::SetWindowTransientFlags { window_id, flags })
        }
        client_msg::RESIZE_WINDOW => {
            if payload.len() != 12 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let width = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            let height = u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
            Ok(ClientMessageRef::ResizeWindow {
                window_id,
                width,
                height,
            })
        }
        client_msg::SET_WINDOW_SIZE_LIMITS => {
            if payload.len() != 20 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let min_width = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            let min_height = u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
            let max_width =
                u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
            let max_height =
                u32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]);
            Ok(ClientMessageRef::SetWindowSizeLimits {
                window_id,
                min_width,
                min_height,
                max_width,
                max_height,
            })
        }
        client_msg::MINIMIZE_WINDOW => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ClientMessageRef::MinimizeWindow { window_id })
        }
        client_msg::MAXIMIZE_WINDOW => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ClientMessageRef::MaximizeWindow { window_id })
        }
        client_msg::RESTORE_WINDOW => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ClientMessageRef::RestoreWindow { window_id })
        }
        client_msg::SET_FULLSCREEN => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ClientMessageRef::SetFullscreen { window_id })
        }
        client_msg::UNSET_FULLSCREEN => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ClientMessageRef::UnsetFullscreen { window_id })
        }
        client_msg::SET_POINTER_LOCK => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = read_u32(payload, 0)?;
            let locked = match read_u32(payload, 4)? {
                0 => false,
                1 => true,
                _ => return Err(ProtocolError::MalformedPayload),
            };
            Ok(ClientMessageRef::SetPointerLock { window_id, locked })
        }
        client_msg::SET_CURSOR_ICON => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = read_u32(payload, 0)?;
            let icon = CursorIcon::from_raw(read_u32(payload, 4)?)
                .ok_or(ProtocolError::MalformedPayload)?;
            Ok(ClientMessageRef::SetCursorIcon { window_id, icon })
        }
        client_msg::SET_CURSOR_THEME => {
            let theme_path = read_len_prefixed_bytes(payload, 0, CURSOR_THEME_PATH_MAX_BYTES)?;
            if theme_path.is_empty() {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::SetCursorTheme { theme_path })
        }
        client_msg::SET_WINDOW_TYPE => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let window_type = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            Ok(ClientMessageRef::SetWindowType {
                window_id,
                window_type,
            })
        }
        client_msg::SET_WINDOW_OPACITY => {
            if payload.len() != 5 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let opacity = payload[4];
            Ok(ClientMessageRef::SetWindowOpacity { window_id, opacity })
        }
        client_msg::REGISTER_EXTENSION => {
            if payload.len() < 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let name_len =
                u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
            if payload.len() != 4 + name_len {
                return Err(ProtocolError::MalformedPayload);
            }
            let extension_name = &payload[4..4 + name_len];
            Ok(ClientMessageRef::RegisterExtension { extension_name })
        }
        client_msg::EXTENSION_CREATE_WINDOW => {
            if payload.len() != 12 {
                return Err(ProtocolError::MalformedPayload);
            }
            let external_client_id =
                u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let width = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            let height = u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
            Ok(ClientMessageRef::ExtensionCreateWindow {
                external_client_id,
                width,
                height,
            })
        }
        client_msg::EXTENSION_UPDATE_BUFFER => {
            if payload.len() != 24 {
                return Err(ProtocolError::MalformedPayload);
            }
            let external_client_id =
                u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let window_id = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            let x = i32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
            let y = i32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
            let width = u32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]);
            let height = u32::from_le_bytes([payload[20], payload[21], payload[22], payload[23]]);
            Ok(ClientMessageRef::ExtensionUpdateBuffer {
                external_client_id,
                window_id,
                x,
                y,
                width,
                height,
            })
        }
        client_msg::EXTENSION_ATTACH_BUFFER => {
            if payload.len() != 36 {
                return Err(ProtocolError::MalformedPayload);
            }
            let external_client_id =
                u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let window_id = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            let width = u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
            let height = u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
            let offset = i32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]);
            let stride = i32::from_le_bytes([payload[20], payload[21], payload[22], payload[23]]);
            let format = u32::from_le_bytes([payload[24], payload[25], payload[26], payload[27]]);
            let shm_size = u64::from_le_bytes([
                payload[28],
                payload[29],
                payload[30],
                payload[31],
                payload[32],
                payload[33],
                payload[34],
                payload[35],
            ]);
            Ok(ClientMessageRef::ExtensionAttachBuffer {
                external_client_id,
                window_id,
                width,
                height,
                offset,
                stride,
                format,
                shm_size,
            })
        }
        client_msg::SET_WORKAREA => {
            if payload.len() != 16 {
                return Err(ProtocolError::MalformedPayload);
            }
            let x = i32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let y = i32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            let width = u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
            let height = u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
            Ok(ClientMessageRef::SetWorkarea {
                x,
                y,
                width,
                height,
            })
        }
        client_msg::SET_WINDOW_RESIZABLE => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let resizable = payload[4] != 0;
            Ok(ClientMessageRef::SetWindowResizable {
                window_id,
                resizable,
            })
        }
        client_msg::GET_SCREEN_SIZE => {
            if !payload.is_empty() {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::GetScreenSize {})
        }
        client_msg::GET_OUTPUT_SCALE => {
            if !payload.is_empty() {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::GetOutputScale {})
        }
        client_msg::GET_WINDOW_LIST => {
            if !payload.is_empty() {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::GetWindowList {})
        }
        client_msg::REQUEST_ACTIVATION_TOKEN => {
            if payload.len() < 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let source_window_id = read_u32(payload, 0)?;
            let target_app_id_len = read_u32(payload, 4)? as usize;
            if target_app_id_len == 0 || payload.len() != 8 + target_app_id_len {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::RequestActivationToken {
                source_window_id,
                target_app_id: &payload[8..],
            })
        }
        client_msg::GET_CAPABILITIES => {
            if !payload.is_empty() {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::GetCapabilities {})
        }
        client_msg::REGISTER_SGFX_BUFFER => {
            if payload.len() != 24 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::RegisterSgfxBuffer {
                window_id: read_u32(payload, 0)?,
                buffer_id: read_u32(payload, 4)?,
                generation: read_u32(payload, 8)?,
                compositor_epoch: read_u32(payload, 12)?,
                width: read_u32(payload, 16)?,
                height: read_u32(payload, 20)?,
            })
        }
        client_msg::COMMIT_SGFX_FRAME => {
            if payload.len() < 28 {
                return Err(ProtocolError::MalformedPayload);
            }
            let commit_serial = read_u64(payload, 16)?;
            let damage_count = read_u32(payload, 24)? as usize;
            let expected_len = damage_count
                .checked_mul(16)
                .and_then(|bytes| bytes.checked_add(28))
                .ok_or(ProtocolError::MalformedPayload)?;
            if commit_serial == 0
                || damage_count == 0
                || damage_count > SGFX_MAX_DAMAGE_RECTS
                || payload.len() != expected_len
            {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::CommitSgfxFrame {
                window_id: read_u32(payload, 0)?,
                buffer_id: read_u32(payload, 4)?,
                generation: read_u32(payload, 8)?,
                compositor_epoch: read_u32(payload, 12)?,
                commit_serial,
                damage_rects: &payload[28..],
            })
        }
        client_msg::DESTROY_SGFX_BUFFER => {
            if payload.len() != 16 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::DestroySgfxBuffer {
                window_id: read_u32(payload, 0)?,
                buffer_id: read_u32(payload, 4)?,
                generation: read_u32(payload, 8)?,
                compositor_epoch: read_u32(payload, 12)?,
            })
        }
        client_msg::LAUNCH_OR_FOCUS => {
            // Payload: app_id_len (u32) + app_id_bytes + exec_path_len (u32) + exec_path_bytes
            if payload.len() < 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let app_id_len =
                u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
            let exec_path_len =
                u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;

            if payload.len() != 8 + app_id_len + exec_path_len {
                return Err(ProtocolError::MalformedPayload);
            }

            let app_id = &payload[8..8 + app_id_len];
            let exec_path = &payload[8 + app_id_len..8 + app_id_len + exec_path_len];

            Ok(ClientMessageRef::LaunchOrFocus { app_id, exec_path })
        }
        client_msg::FOCUS_WINDOW => {
            // Payload: window_id (u32)
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ClientMessageRef::FocusWindow { window_id })
        }
        client_msg::GET_ACTIVE_APP => {
            // No payload
            if !payload.is_empty() {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::GetActiveApp {})
        }
        client_msg::SET_WINDOW_HAS_ALPHA_CONTENT => {
            // Payload: window_id (u32) + has_alpha (u8, 0 = false, 1 = true)
            if payload.len() != 5 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let has_alpha = payload[4] != 0;
            Ok(ClientMessageRef::SetWindowHasAlphaContent {
                window_id,
                has_alpha,
            })
        }
        client_msg::SET_WINDOW_MENU_TITLES => {
            // Payload: window_id (u32) + menu_titles_len (u32) + menu_titles_bytes
            if payload.len() < 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let titles_len =
                u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;
            if payload.len() != 8 + titles_len {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::SetWindowMenuTitles {
                window_id,
                menu_titles: &payload[8..],
            })
        }
        client_msg::ACTIVATE_MENU_ITEM => {
            // Payload: window_id (u32) + menu_item_id_len (u32) + menu_item_id_bytes
            if payload.len() < 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let menu_item_id_len =
                u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;
            if payload.len() != 8 + menu_item_id_len {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::ActivateMenuItem {
                window_id,
                menu_item_id: &payload[8..],
            })
        }
        client_msg::TEXT_INPUT_CREATE => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::TextInputCreate {
                window_id: read_u32(payload, 0)?,
                seat_id: read_u32(payload, 4)?,
            })
        }
        client_msg::TEXT_INPUT_DESTROY => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::TextInputDestroy {
                context_id: read_u32(payload, 0)?,
            })
        }
        client_msg::TEXT_INPUT_ENABLE => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::TextInputEnable {
                context_id: read_u32(payload, 0)?,
            })
        }
        client_msg::TEXT_INPUT_DISABLE => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::TextInputDisable {
                context_id: read_u32(payload, 0)?,
            })
        }
        client_msg::TEXT_INPUT_SET_CURSOR_RECT => {
            if payload.len() != 20 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::TextInputSetCursorRect {
                context_id: read_u32(payload, 0)?,
                x: read_i32(payload, 4)?,
                y: read_i32(payload, 8)?,
                width: read_u32(payload, 12)?,
                height: read_u32(payload, 16)?,
            })
        }
        client_msg::TEXT_INPUT_SET_SURROUNDING_TEXT => {
            if payload.len() < 16 {
                return Err(ProtocolError::MalformedPayload);
            }
            let context_id = read_u32(payload, 0)?;
            let cursor_byte = read_u32(payload, 4)?;
            let anchor_byte = read_u32(payload, 8)?;
            let text = read_len_prefixed_bytes(payload, 12, TEXT_INPUT_MAX_BYTES)?;
            Ok(ClientMessageRef::TextInputSetSurroundingText {
                context_id,
                cursor_byte,
                anchor_byte,
                text,
            })
        }
        client_msg::TEXT_INPUT_SET_CONTENT_TYPE => {
            if payload.len() != 12 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::TextInputSetContentType {
                context_id: read_u32(payload, 0)?,
                hint: read_u32(payload, 4)?,
                purpose: read_u32(payload, 8)?,
            })
        }
        client_msg::TEXT_INPUT_SET_TEXT_CHANGE_CAUSE => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::TextInputSetTextChangeCause {
                context_id: read_u32(payload, 0)?,
                cause: read_u32(payload, 4)?,
            })
        }
        client_msg::TEXT_INPUT_COMMIT_STATE => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::TextInputCommitState {
                context_id: read_u32(payload, 0)?,
                serial: read_u32(payload, 4)?,
            })
        }
        client_msg::IME_GET_METHODS => {
            if !payload.is_empty() {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::ImeGetMethods {})
        }
        client_msg::IME_GET_ACTIVE => {
            if !payload.is_empty() {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::ImeGetActive {})
        }
        client_msg::IME_REGISTER => {
            if payload.len() < 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let capabilities = read_u32(payload, 0)?;
            let name = read_len_prefixed_bytes(payload, 4, TEXT_INPUT_MAX_BYTES)?;
            Ok(ClientMessageRef::ImeRegister { name, capabilities })
        }
        client_msg::IME_SET_ACTIVE => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::ImeSetActive {
                ime_id: read_u32(payload, 0)?,
            })
        }
        client_msg::IME_KEY_HANDLED => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::ImeKeyHandled {
                key_serial: read_u32(payload, 0)?,
                handled: read_u32(payload, 4)? != 0,
            })
        }
        client_msg::IME_SET_PREEDIT => {
            if payload.len() < 20 {
                return Err(ProtocolError::MalformedPayload);
            }
            let context_id = read_u32(payload, 0)?;
            let cursor_byte = read_u32(payload, 4)?;
            let anchor_byte = read_u32(payload, 8)?;
            let text_len = read_u32(payload, 12)? as usize;
            let spans_len_offset = 16usize.saturating_add(text_len);
            if text_len > TEXT_INPUT_MAX_BYTES || payload.len() < spans_len_offset + 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let spans_len = read_u32(payload, spans_len_offset)? as usize;
            let spans_offset = spans_len_offset + 4;
            if spans_len > TEXT_INPUT_PREEDIT_SPANS_MAX_BYTES
                || payload.len() != spans_offset + spans_len
            {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::ImeSetPreedit {
                context_id,
                cursor_byte,
                anchor_byte,
                text: &payload[16..16 + text_len],
                spans: &payload[spans_offset..],
            })
        }
        client_msg::IME_COMMIT_TEXT => {
            if payload.len() < 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let context_id = read_u32(payload, 0)?;
            let text = read_len_prefixed_bytes(payload, 4, TEXT_INPUT_MAX_BYTES)?;
            Ok(ClientMessageRef::ImeCommitText { context_id, text })
        }
        client_msg::IME_DELETE_SURROUNDING_TEXT => {
            if payload.len() != 12 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::ImeDeleteSurroundingText {
                context_id: read_u32(payload, 0)?,
                before_bytes: read_u32(payload, 4)?,
                after_bytes: read_u32(payload, 8)?,
            })
        }
        client_msg::IME_GRAB_KEYBOARD => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::ImeGrabKeyboard {
                context_id: read_u32(payload, 0)?,
            })
        }
        client_msg::IME_RELEASE_KEYBOARD => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::ImeReleaseKeyboard {
                context_id: read_u32(payload, 0)?,
            })
        }
        client_msg::IME_SET_STATUS => {
            if payload.len() < 20 {
                return Err(ProtocolError::MalformedPayload);
            }
            let context_id = read_u32(payload, 0)?;
            let state = read_u32(payload, 4)?;
            let mode_id = read_u32(payload, 8)?;
            let flags = read_u32(payload, 12)?;
            let mode_label = read_len_prefixed_bytes(payload, 16, TEXT_INPUT_MAX_BYTES)?;
            Ok(ClientMessageRef::ImeSetStatus {
                context_id,
                state,
                mode_id,
                flags,
                mode_label,
            })
        }
        client_msg::IME_SET_POPUP_WINDOW => {
            if payload.len() != 20 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::ImeSetPopupWindow {
                context_id: read_u32(payload, 0)?,
                window_id: read_u32(payload, 4)?,
                offset_x: read_i32(payload, 8)?,
                offset_y: read_i32(payload, 12)?,
                visible: read_u32(payload, 16)? != 0,
            })
        }
        _ => Err(ProtocolError::UnknownMessageType),
    }
}

fn parse_ime_context_message(
    payload: &[u8],
    activate: bool,
) -> Result<ServerMessage, ProtocolError> {
    if payload.len() < 52 {
        return Err(ProtocolError::MalformedPayload);
    }
    let context_id = read_u32(payload, 0)?;
    let window_id = read_u32(payload, 4)?;
    let serial = read_u32(payload, 8)?;
    let cursor_x = read_i32(payload, 12)?;
    let cursor_y = read_i32(payload, 16)?;
    let cursor_width = read_u32(payload, 20)?;
    let cursor_height = read_u32(payload, 24)?;
    let content_hint = read_u32(payload, 28)?;
    let content_purpose = read_u32(payload, 32)?;
    let text_change_cause = read_u32(payload, 36)?;
    let cursor_byte = read_u32(payload, 40)?;
    let anchor_byte = read_u32(payload, 44)?;
    let surrounding_text = read_len_prefixed_bytes(payload, 48, TEXT_INPUT_MAX_BYTES)?;
    let (surrounding_text, surrounding_text_len) = copy_bounded(surrounding_text)?;

    if activate {
        Ok(ServerMessage::ImeActivate {
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
        })
    } else {
        Ok(ServerMessage::ImeContextState {
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
        })
    }
}

fn parse_sgfx_buffer_identity(payload: &[u8]) -> Result<(u32, u32, u32, u32), ProtocolError> {
    if payload.len() != 16 {
        return Err(ProtocolError::MalformedPayload);
    }
    Ok((
        read_u32(payload, 0)?,
        read_u32(payload, 4)?,
        read_u32(payload, 8)?,
        read_u32(payload, 12)?,
    ))
}

/// Parse a server->client message from `(msg_type, payload)`.
pub fn parse_server_message(msg_type: u32, payload: &[u8]) -> Result<ServerMessage, ProtocolError> {
    match msg_type {
        server_msg::WINDOW_CREATED => {
            if payload.len() != 12 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let shm_size = u64::from_le_bytes([
                payload[4],
                payload[5],
                payload[6],
                payload[7],
                payload[8],
                payload[9],
                payload[10],
                payload[11],
            ]);
            Ok(ServerMessage::WindowCreated {
                window_id,
                shm_size,
            })
        }
        server_msg::WINDOW_DESTROYED => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ServerMessage::WindowDestroyed { window_id })
        }
        server_msg::WINDOW_RESIZED => {
            if payload.len() != 20 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let shm_size = u64::from_le_bytes([
                payload[4],
                payload[5],
                payload[6],
                payload[7],
                payload[8],
                payload[9],
                payload[10],
                payload[11],
            ]);
            let width = u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
            let height = u32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]);
            Ok(ServerMessage::WindowResized {
                window_id,
                shm_size,
                width,
                height,
            })
        }
        server_msg::WINDOW_CONFIGURE => {
            if payload.len() != 12 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let width = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            let height = u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
            Ok(ServerMessage::WindowConfigure {
                window_id,
                width,
                height,
            })
        }
        server_msg::WINDOW_STATE_CHANGED => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let state_flags = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            Ok(ServerMessage::WindowStateChanged {
                window_id,
                state_flags,
            })
        }
        server_msg::POINTER_LOCK_CHANGED => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = read_u32(payload, 0)?;
            let locked = match read_u32(payload, 4)? {
                0 => false,
                1 => true,
                _ => return Err(ProtocolError::MalformedPayload),
            };
            Ok(ServerMessage::PointerLockChanged { window_id, locked })
        }
        server_msg::CURSOR_THEME_CHANGED => {
            if !payload.is_empty() {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ServerMessage::CursorThemeChanged)
        }
        server_msg::INPUT_EVENT => {
            if payload.len() != 20 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let time = u64::from_le_bytes([
                payload[4],
                payload[5],
                payload[6],
                payload[7],
                payload[8],
                payload[9],
                payload[10],
                payload[11],
            ]);
            let type_ = u16::from_le_bytes([payload[12], payload[13]]);
            let code = u16::from_le_bytes([payload[14], payload[15]]);
            let value = i32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]);
            Ok(ServerMessage::InputEvent {
                window_id,
                time,
                type_,
                code,
                value,
            })
        }
        server_msg::TEXT_INPUT_CREATED => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ServerMessage::TextInputCreated {
                context_id: read_u32(payload, 0)?,
                serial: read_u32(payload, 4)?,
            })
        }
        server_msg::TEXT_INPUT_PREEDIT => {
            if payload.len() < 24 {
                return Err(ProtocolError::MalformedPayload);
            }
            let context_id = read_u32(payload, 0)?;
            let serial = read_u32(payload, 4)?;
            let cursor_byte = read_u32(payload, 8)?;
            let anchor_byte = read_u32(payload, 12)?;
            let text_len = read_u32(payload, 16)? as usize;
            let spans_len_offset = 20usize.saturating_add(text_len);
            if text_len > TEXT_INPUT_MAX_BYTES || payload.len() < spans_len_offset + 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let spans_len = read_u32(payload, spans_len_offset)? as usize;
            let spans_offset = spans_len_offset + 4;
            if spans_len > TEXT_INPUT_PREEDIT_SPANS_MAX_BYTES
                || payload.len() != spans_offset + spans_len
            {
                return Err(ProtocolError::MalformedPayload);
            }
            let (text, text_len) = copy_bounded(&payload[20..20 + text_len])?;
            let (spans, spans_len) = copy_bounded(&payload[spans_offset..])?;
            Ok(ServerMessage::TextInputPreedit {
                context_id,
                serial,
                cursor_byte,
                anchor_byte,
                text,
                text_len,
                spans,
                spans_len,
            })
        }
        server_msg::TEXT_INPUT_COMMIT => {
            if payload.len() < 12 {
                return Err(ProtocolError::MalformedPayload);
            }
            let context_id = read_u32(payload, 0)?;
            let serial = read_u32(payload, 4)?;
            let text = read_len_prefixed_bytes(payload, 8, TEXT_INPUT_MAX_BYTES)?;
            let (text, text_len) = copy_bounded(text)?;
            Ok(ServerMessage::TextInputCommit {
                context_id,
                serial,
                text,
                text_len,
            })
        }
        server_msg::TEXT_INPUT_DELETE_SURROUNDING_TEXT => {
            if payload.len() != 16 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ServerMessage::TextInputDeleteSurroundingText {
                context_id: read_u32(payload, 0)?,
                serial: read_u32(payload, 4)?,
                before_bytes: read_u32(payload, 8)?,
                after_bytes: read_u32(payload, 12)?,
            })
        }
        server_msg::TEXT_INPUT_DONE => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ServerMessage::TextInputDone {
                context_id: read_u32(payload, 0)?,
                serial: read_u32(payload, 4)?,
            })
        }
        server_msg::TEXT_INPUT_STATUS => {
            if payload.len() < 24 {
                return Err(ProtocolError::MalformedPayload);
            }
            let context_id = read_u32(payload, 0)?;
            let serial = read_u32(payload, 4)?;
            let state = read_u32(payload, 8)?;
            let mode_id = read_u32(payload, 12)?;
            let flags = read_u32(payload, 16)?;
            let mode_label = read_len_prefixed_bytes(payload, 20, TEXT_INPUT_MAX_BYTES)?;
            let (mode_label, mode_label_len) = copy_bounded(mode_label)?;
            Ok(ServerMessage::TextInputStatus {
                context_id,
                serial,
                state,
                mode_id,
                flags,
                mode_label,
                mode_label_len,
            })
        }
        server_msg::IME_REGISTERED => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ServerMessage::ImeRegistered {
                ime_id: read_u32(payload, 0)?,
            })
        }
        server_msg::IME_METHODS => Ok(ServerMessage::ImeMethods),
        server_msg::IME_ACTIVE => Ok(ServerMessage::ImeActive),
        server_msg::IME_ACTIVATE => parse_ime_context_message(payload, true),
        server_msg::IME_CONTEXT_STATE => parse_ime_context_message(payload, false),
        server_msg::IME_DEACTIVATE => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ServerMessage::ImeDeactivate {
                context_id: read_u32(payload, 0)?,
                serial: read_u32(payload, 4)?,
            })
        }
        server_msg::IME_KEY_EVENT => {
            if payload.len() != 28 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ServerMessage::ImeKeyEvent {
                context_id: read_u32(payload, 0)?,
                key_serial: read_u32(payload, 4)?,
                window_id: read_u32(payload, 8)?,
                time: read_u64(payload, 12)?,
                type_: read_u16(payload, 20)?,
                code: read_u16(payload, 22)?,
                value: read_i32(payload, 24)?,
            })
        }
        server_msg::IME_RESET => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ServerMessage::ImeReset {
                context_id: read_u32(payload, 0)?,
                serial: read_u32(payload, 4)?,
            })
        }
        server_msg::IME_TRIGGER => {
            if payload.len() != 24 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ServerMessage::ImeTrigger {
                context_id: read_u32(payload, 0)?,
                serial: read_u32(payload, 4)?,
                trigger_id: read_u32(payload, 8)?,
                code: read_u16(payload, 12)?,
                time: read_u64(payload, 16)?,
            })
        }
        server_msg::SCREEN_SIZE => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let width = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let height = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            Ok(ServerMessage::ScreenSize { width, height })
        }
        server_msg::SCREEN_SIZE_CHANGED => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let width = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let height = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            Ok(ServerMessage::ScreenSizeChanged { width, height })
        }
        server_msg::OUTPUT_SCALE => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let scale_milli = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ServerMessage::OutputScale { scale_milli })
        }
        server_msg::OUTPUT_SCALE_CHANGED => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let scale_milli = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ServerMessage::OutputScaleChanged { scale_milli })
        }
        server_msg::CAPABILITIES => {
            if payload.len() != 20 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ServerMessage::Capabilities {
                protocol_version: read_u32(payload, 0)?,
                capabilities: read_u64(payload, 4)?,
                compositor_epoch: read_u32(payload, 12)?,
                compositor_backend: read_u32(payload, 16)?,
            })
        }
        server_msg::SGFX_BUFFER_REGISTERED => {
            let (window_id, buffer_id, generation, compositor_epoch) =
                parse_sgfx_buffer_identity(payload)?;
            Ok(ServerMessage::SgfxBufferRegistered {
                window_id,
                buffer_id,
                generation,
                compositor_epoch,
            })
        }
        server_msg::SGFX_FRAME_REJECTED => {
            if payload.len() != 28 {
                return Err(ProtocolError::MalformedPayload);
            }
            let commit_serial = read_u64(payload, 16)?;
            if commit_serial == 0 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ServerMessage::SgfxFrameRejected {
                window_id: read_u32(payload, 0)?,
                buffer_id: read_u32(payload, 4)?,
                generation: read_u32(payload, 8)?,
                compositor_epoch: read_u32(payload, 12)?,
                commit_serial,
                code: read_u32(payload, 24)?,
            })
        }
        server_msg::SGFX_BUFFER_RELEASED => {
            if payload.len() != 24 {
                return Err(ProtocolError::MalformedPayload);
            }
            let commit_serial = read_u64(payload, 16)?;
            if commit_serial == 0 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ServerMessage::SgfxBufferReleased {
                window_id: read_u32(payload, 0)?,
                buffer_id: read_u32(payload, 4)?,
                generation: read_u32(payload, 8)?,
                compositor_epoch: read_u32(payload, 12)?,
                commit_serial,
            })
        }
        server_msg::SGFX_BACKEND_LOST => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ServerMessage::SgfxBackendLost {
                compositor_epoch: read_u32(payload, 0)?,
            })
        }
        server_msg::SGFX_BUFFER_DESTROYED => {
            let (window_id, buffer_id, generation, compositor_epoch) =
                parse_sgfx_buffer_identity(payload)?;
            Ok(ServerMessage::SgfxBufferDestroyed {
                window_id,
                buffer_id,
                generation,
                compositor_epoch,
            })
        }
        server_msg::ACTIVATION_TOKEN => {
            let token = read_len_prefixed_bytes(payload, 0, ACTIVATION_TOKEN_MAX_BYTES)?;
            if token.is_empty() {
                return Err(ProtocolError::MalformedPayload);
            }
            let (token, token_len) = copy_bounded(token)?;
            Ok(ServerMessage::ActivationToken { token, token_len })
        }
        server_msg::WINDOW_LIST => {
            // Window list payload is variable length, just validate it's not empty
            if payload.is_empty() {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ServerMessage::WindowList)
        }
        server_msg::FOCUS_CHANGED => {
            // Payload: window_id (u32) + app_id_len (u32) + app_id (variable, max 128)
            //          + app_name_len (u32) + app_name (variable, max 128)
            //          + title_len (u32) + title (variable, max 256)
            //          + menu_titles_len (u32) + menu_titles (variable, max 2048)
            if payload.len() < 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let app_id_len =
                u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;

            let mut offset = 8 + app_id_len;
            if payload.len() < offset + 4 {
                return Err(ProtocolError::MalformedPayload);
            }

            let mut app_id = [0u8; 128];
            if app_id_len > 0 {
                app_id[..app_id_len].copy_from_slice(&payload[8..8 + app_id_len]);
            }

            let app_name_len = u32::from_le_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]) as usize;
            offset += 4 + app_name_len;

            if payload.len() < offset + 4 {
                return Err(ProtocolError::MalformedPayload);
            }

            let mut app_name = [0u8; 128];
            if app_name_len > 0 {
                app_name[..app_name_len].copy_from_slice(&payload[offset - app_name_len..offset]);
            }

            let title_len = u32::from_le_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]) as usize;
            offset += 4 + title_len;

            if payload.len() < offset + 4 {
                return Err(ProtocolError::MalformedPayload);
            }

            let mut title = [0u8; 256];
            if title_len > 0 {
                title[..title_len].copy_from_slice(&payload[offset - title_len..offset]);
            }

            let menu_titles_len = u32::from_le_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]) as usize;
            offset += 4 + menu_titles_len;

            if payload.len() < offset {
                return Err(ProtocolError::MalformedPayload);
            }

            let mut menu_titles = [0u8; 2048];
            if menu_titles_len > 0 {
                menu_titles[..menu_titles_len]
                    .copy_from_slice(&payload[offset - menu_titles_len..offset]);
            }

            Ok(ServerMessage::FocusChanged {
                window_id,
                app_id,
                app_id_len: app_id_len as u32,
                app_name,
                app_name_len: app_name_len as u32,
                title,
                title_len: title_len as u32,
                menu_titles,
                menu_titles_len: menu_titles_len as u32,
            })
        }
        server_msg::ACTIVE_APP_CHANGED => {
            // Payload format is the same as FOCUS_CHANGED
            if payload.len() < 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);

            let mut offset = 4;

            let app_id_len = if offset + 4 <= payload.len() {
                let len = u32::from_le_bytes([
                    payload[offset],
                    payload[offset + 1],
                    payload[offset + 2],
                    payload[offset + 3],
                ]) as usize;
                offset += 4;
                len
            } else {
                return Err(ProtocolError::MalformedPayload);
            };

            if offset + app_id_len > payload.len() {
                return Err(ProtocolError::MalformedPayload);
            }
            let mut app_id = [0u8; 128];
            if app_id_len > 0 {
                app_id[..app_id_len].copy_from_slice(&payload[offset..offset + app_id_len]);
            }
            offset += app_id_len;

            let app_name_len = if offset + 4 <= payload.len() {
                let len = u32::from_le_bytes([
                    payload[offset],
                    payload[offset + 1],
                    payload[offset + 2],
                    payload[offset + 3],
                ]) as usize;
                offset += 4;
                len
            } else {
                return Err(ProtocolError::MalformedPayload);
            };

            if offset + app_name_len > payload.len() {
                return Err(ProtocolError::MalformedPayload);
            }
            let mut app_name = [0u8; 128];
            if app_name_len > 0 {
                app_name[..app_name_len].copy_from_slice(&payload[offset..offset + app_name_len]);
            }
            offset += app_name_len;

            let title_len = if offset + 4 <= payload.len() {
                let len = u32::from_le_bytes([
                    payload[offset],
                    payload[offset + 1],
                    payload[offset + 2],
                    payload[offset + 3],
                ]) as usize;
                offset += 4;
                len
            } else {
                return Err(ProtocolError::MalformedPayload);
            };

            if offset + title_len > payload.len() {
                return Err(ProtocolError::MalformedPayload);
            }
            let mut title = [0u8; 256];
            if title_len > 0 {
                title[..title_len].copy_from_slice(&payload[offset..offset + title_len]);
            }
            offset += title_len;

            let menu_titles_len = if offset + 4 <= payload.len() {
                let len = u32::from_le_bytes([
                    payload[offset],
                    payload[offset + 1],
                    payload[offset + 2],
                    payload[offset + 3],
                ]) as usize;
                offset += 4;
                len
            } else {
                return Err(ProtocolError::MalformedPayload);
            };

            if offset + menu_titles_len > payload.len() {
                return Err(ProtocolError::MalformedPayload);
            }

            let mut menu_titles = [0u8; 2048];
            if menu_titles_len > 0 {
                menu_titles[..menu_titles_len]
                    .copy_from_slice(&payload[offset..offset + menu_titles_len]);
            }

            Ok(ServerMessage::ActiveAppChanged {
                window_id,
                app_id,
                app_id_len: app_id_len as u32,
                app_name,
                app_name_len: app_name_len as u32,
                title,
                title_len: title_len as u32,
                menu_titles,
                menu_titles_len: menu_titles_len as u32,
            })
        }
        server_msg::ERROR => {
            if payload.len() < 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let code = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ServerMessage::Error { code })
        }
        server_msg::EXTENSION_REGISTERED => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let extension_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ServerMessage::ExtensionRegistered { extension_id })
        }
        server_msg::EXTENSION_INPUT_EVENT => {
            if payload.len() != 24 {
                return Err(ProtocolError::MalformedPayload);
            }
            let external_client_id =
                u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let window_id = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            let time = u64::from_le_bytes([
                payload[8],
                payload[9],
                payload[10],
                payload[11],
                payload[12],
                payload[13],
                payload[14],
                payload[15],
            ]);
            let type_ = u16::from_le_bytes([payload[16], payload[17]]);
            let code = u16::from_le_bytes([payload[18], payload[19]]);
            let value = i32::from_le_bytes([payload[20], payload[21], payload[22], payload[23]]);
            Ok(ServerMessage::ExtensionInputEvent {
                external_client_id,
                window_id,
                time,
                type_,
                code,
                value,
            })
        }
        server_msg::ACTIVE_APP => {
            // Payload: app_id_len (u32) + app_id (variable, max 128)
            //          + app_name_len (u32) + app_name (variable, max 128)
            //          + menu_titles_len (u32) + menu_titles (variable, max 2048)
            if payload.len() < 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let app_id_len =
                u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;

            if payload.len() < 4 + app_id_len + 4 {
                return Err(ProtocolError::MalformedPayload);
            }

            let mut app_id = [0u8; 128];
            if app_id_len > 0 {
                app_id[..app_id_len].copy_from_slice(&payload[4..4 + app_id_len]);
            }

            let offset = 4 + app_id_len;
            let app_name_len = u32::from_le_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]) as usize;

            if payload.len() < offset + 4 + app_name_len + 4 {
                return Err(ProtocolError::MalformedPayload);
            }

            let mut app_name = [0u8; 128];
            if app_name_len > 0 {
                app_name[..app_name_len]
                    .copy_from_slice(&payload[offset + 4..offset + 4 + app_name_len]);
            }

            let offset2 = offset + 4 + app_name_len;
            let menu_titles_len = u32::from_le_bytes([
                payload[offset2],
                payload[offset2 + 1],
                payload[offset2 + 2],
                payload[offset2 + 3],
            ]) as usize;

            if payload.len() < offset2 + 4 + menu_titles_len {
                return Err(ProtocolError::MalformedPayload);
            }

            let mut menu_titles = [0u8; 2048];
            if menu_titles_len > 0 {
                menu_titles[..menu_titles_len]
                    .copy_from_slice(&payload[offset2 + 4..offset2 + 4 + menu_titles_len]);
            }

            Ok(ServerMessage::ActiveApp {
                app_id,
                app_id_len: app_id_len as u32,
                app_name,
                app_name_len: app_name_len as u32,
                menu_titles,
                menu_titles_len: menu_titles_len as u32,
            })
        }
        server_msg::MENU_ITEM_ACTIVATED => {
            // Payload: window_id (u32) + menu_item_id_len (u32) + menu_item_id (variable, max 128)
            if payload.len() < 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let menu_item_id_len =
                u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;
            if payload.len() != 8 + menu_item_id_len {
                return Err(ProtocolError::MalformedPayload);
            }
            let mut menu_item_id = [0u8; 128];
            if menu_item_id_len > 0 {
                let capped_len = menu_item_id_len.min(menu_item_id.len());
                menu_item_id[..capped_len].copy_from_slice(&payload[8..8 + capped_len]);
                return Ok(ServerMessage::MenuItemActivated {
                    window_id,
                    menu_item_id,
                    menu_item_id_len: capped_len as u32,
                });
            }
            Ok(ServerMessage::MenuItemActivated {
                window_id,
                menu_item_id,
                menu_item_id_len: 0,
            })
        }
        _ => Err(ProtocolError::UnknownMessageType),
    }
}

/// Build payload for client->server `CREATE_WINDOW`.
///
/// Payload format:
/// - app_id_len (u32)
/// - app_id_bytes (variable)
/// - app_name_len (u32)
/// - app_name_bytes (variable)
/// - menu_titles_len (u32)
/// - menu_titles_bytes (variable, format: "menu1|menu2|menu3")
/// - width (u32)
/// - height (u32)
/// - window_type (u32)
/// - resizable (u32, 0=false, 1=true)
/// - focus_on_create (u32, 0=false, 1=true)
/// - active_on_focus (u32, 0=false, 1=true)
pub fn payload_create_window(
    app_id: &[u8],
    app_name: &[u8],
    menu_titles: &[u8],
    width: u32,
    height: u32,
    window_type: u32,
    resizable: bool,
    focus_on_create: bool,
    active_on_focus: bool,
) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&(app_id.len() as u32).to_le_bytes());
    payload.extend_from_slice(app_id);
    payload.extend_from_slice(&(app_name.len() as u32).to_le_bytes());
    payload.extend_from_slice(app_name);
    payload.extend_from_slice(&(menu_titles.len() as u32).to_le_bytes());
    payload.extend_from_slice(menu_titles);
    payload.extend_from_slice(&width.to_le_bytes());
    payload.extend_from_slice(&height.to_le_bytes());
    payload.extend_from_slice(&window_type.to_le_bytes());
    payload.extend_from_slice(&(resizable as u32).to_le_bytes());
    payload.extend_from_slice(&(focus_on_create as u32).to_le_bytes());
    payload.extend_from_slice(&(active_on_focus as u32).to_le_bytes());
    payload
}

/// Build payload for client->server `CREATE_WINDOW` with initial position.
///
/// Payload format extends `payload_create_window` with:
/// - initial_x (i32)
/// - initial_y (i32)
pub fn payload_create_window_with_position(
    app_id: &[u8],
    app_name: &[u8],
    menu_titles: &[u8],
    width: u32,
    height: u32,
    window_type: u32,
    resizable: bool,
    focus_on_create: bool,
    active_on_focus: bool,
    initial_x: i32,
    initial_y: i32,
) -> Vec<u8> {
    let mut payload = payload_create_window(
        app_id,
        app_name,
        menu_titles,
        width,
        height,
        window_type,
        resizable,
        focus_on_create,
        active_on_focus,
    );
    payload.extend_from_slice(&initial_x.to_le_bytes());
    payload.extend_from_slice(&initial_y.to_le_bytes());
    payload
}

/// Build a `CREATE_WINDOW` payload with an explicit initial placement policy.
///
/// The trailing fields are `placement`, `initial_x`, and `initial_y`.
/// Coordinates are used only for [`window_placement::ABSOLUTE`].
pub fn payload_create_window_with_placement(
    app_id: &[u8],
    app_name: &[u8],
    menu_titles: &[u8],
    width: u32,
    height: u32,
    window_type: u32,
    resizable: bool,
    focus_on_create: bool,
    active_on_focus: bool,
    placement: u32,
    initial_x: i32,
    initial_y: i32,
) -> Vec<u8> {
    let mut payload = payload_create_window(
        app_id,
        app_name,
        menu_titles,
        width,
        height,
        window_type,
        resizable,
        focus_on_create,
        active_on_focus,
    );
    payload.extend_from_slice(&placement.to_le_bytes());
    payload.extend_from_slice(&initial_x.to_le_bytes());
    payload.extend_from_slice(&initial_y.to_le_bytes());
    payload
}

/// Build a `CREATE_WINDOW` payload carrying an activation token.
///
/// The token is opaque to the client and is consumed by SWS when the first
/// matching normal toplevel is created.
///
/// # Arguments
///
/// * `app_id` - Stable application identifier.
/// * `app_name` - Human-readable application name.
/// * `menu_titles` - Serialized application menu titles.
/// * `width` - Initial buffer width.
/// * `height` - Initial buffer height.
/// * `window_type` - SWS window role.
/// * `resizable` - Whether interactive resize is permitted.
/// * `focus_on_create` - Whether the new window normally requests focus.
/// * `active_on_focus` - Whether focus activates the application.
/// * `placement` - Initial placement hint.
/// * `initial_x` - Absolute horizontal position when requested.
/// * `initial_y` - Absolute vertical position when requested.
/// * `activation_token` - Opaque token issued by SWS.
///
/// # Returns
///
/// Serialized `CREATE_WINDOW` payload.
pub fn payload_create_window_with_placement_and_activation_token(
    app_id: &[u8],
    app_name: &[u8],
    menu_titles: &[u8],
    width: u32,
    height: u32,
    window_type: u32,
    resizable: bool,
    focus_on_create: bool,
    active_on_focus: bool,
    placement: u32,
    initial_x: i32,
    initial_y: i32,
    activation_token: &[u8],
) -> Vec<u8> {
    let mut payload = payload_create_window_with_placement(
        app_id,
        app_name,
        menu_titles,
        width,
        height,
        window_type,
        resizable,
        focus_on_create,
        active_on_focus,
        placement,
        initial_x,
        initial_y,
    );
    payload.extend_from_slice(&(activation_token.len() as u32).to_le_bytes());
    payload.extend_from_slice(activation_token);
    payload
}

/// Build a request for an opaque application activation token.
///
/// # Arguments
///
/// * `source_window_id` - Focused window initiating the activation.
/// * `target_app_id` - Application identifier expected on the target toplevel.
///
/// # Returns
///
/// Serialized activation-token request payload.
pub fn payload_request_activation_token(source_window_id: u32, target_app_id: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&source_window_id.to_le_bytes());
    payload.extend_from_slice(&(target_app_id.len() as u32).to_le_bytes());
    payload.extend_from_slice(target_app_id);
    payload
}

/// Build an activation-token response payload.
///
/// # Arguments
///
/// * `token` - Opaque token bytes generated by SWS.
///
/// # Returns
///
/// Length-prefixed activation-token response payload.
pub fn payload_activation_token(token: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&(token.len() as u32).to_le_bytes());
    payload.extend_from_slice(token);
    payload
}

/// Build payload for client->server `DESTROY_WINDOW`.
pub fn payload_destroy_window(window_id: u32) -> [u8; 4] {
    window_id.to_le_bytes()
}

/// Build payload for client->server `SET_WINDOW_TITLE`.
pub fn payload_set_window_title(window_id: u32, title: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&window_id.to_le_bytes());
    out.extend_from_slice(&(title.len() as u32).to_le_bytes());
    out.extend_from_slice(title);
    out
}

/// Build payload for client->server `SET_WINDOW_MENU_TITLES`.
///
/// Payload format:
/// - window_id (u32)
/// - menu_titles_len (u32)
/// - menu_titles_bytes (variable, format: "menu1|menu2|menu3")
pub fn payload_set_window_menu_titles(window_id: u32, menu_titles: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&window_id.to_le_bytes());
    out.extend_from_slice(&(menu_titles.len() as u32).to_le_bytes());
    out.extend_from_slice(menu_titles);
    out
}

/// Build payload for client->server `ACTIVATE_MENU_ITEM`.
///
/// Payload format:
/// - window_id (u32)
/// - menu_item_id_len (u32)
/// - menu_item_id_bytes (variable)
pub fn payload_activate_menu_item(window_id: u32, menu_item_id: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&window_id.to_le_bytes());
    out.extend_from_slice(&(menu_item_id.len() as u32).to_le_bytes());
    out.extend_from_slice(menu_item_id);
    out
}

/// Build payload for client->server `UPDATE_BUFFER` (damage notification).
pub fn payload_update_buffer(window_id: u32, x: i32, y: i32, width: u32, height: u32) -> [u8; 20] {
    let mut payload = [0u8; 20];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&x.to_le_bytes());
    payload[8..12].copy_from_slice(&y.to_le_bytes());
    payload[12..16].copy_from_slice(&width.to_le_bytes());
    payload[16..20].copy_from_slice(&height.to_le_bytes());
    payload
}

/// Build payload for extension->server `EXTENSION_ATTACH_BUFFER`.
pub fn payload_extension_attach_buffer(
    external_client_id: u32,
    window_id: u32,
    width: u32,
    height: u32,
    offset: i32,
    stride: i32,
    format: u32,
    shm_size: u64,
) -> [u8; 36] {
    let mut payload = [0u8; 36];
    payload[0..4].copy_from_slice(&external_client_id.to_le_bytes());
    payload[4..8].copy_from_slice(&window_id.to_le_bytes());
    payload[8..12].copy_from_slice(&width.to_le_bytes());
    payload[12..16].copy_from_slice(&height.to_le_bytes());
    payload[16..20].copy_from_slice(&offset.to_le_bytes());
    payload[20..24].copy_from_slice(&stride.to_le_bytes());
    payload[24..28].copy_from_slice(&format.to_le_bytes());
    payload[28..36].copy_from_slice(&shm_size.to_le_bytes());
    payload
}

/// Build payload for client->server `REQUEST_MOVE_WINDOW`.
pub fn payload_request_move_window(window_id: u32) -> [u8; 4] {
    window_id.to_le_bytes()
}

/// Build payload for client->server `MOVE_WINDOW`.
pub fn payload_move_window(window_id: u32, x: i32, y: i32) -> [u8; 12] {
    let mut payload = [0u8; 12];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&x.to_le_bytes());
    payload[8..12].copy_from_slice(&y.to_le_bytes());
    payload
}

/// Build payload for client->server `SET_WINDOW_PARENT`.
///
/// `parent_id == 0` means "no parent".
pub fn payload_set_window_parent(window_id: u32, parent_id: u32) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&parent_id.to_le_bytes());
    payload
}

/// Build payload for client->server `SET_WINDOW_TRANSIENT_FLAGS`.
pub fn payload_set_window_transient_flags(window_id: u32, flags: u32) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&flags.to_le_bytes());
    payload
}

/// Build payload for client->server `RESIZE_WINDOW`.
pub fn payload_resize_window(window_id: u32, width: u32, height: u32) -> [u8; 12] {
    let mut payload = [0u8; 12];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&width.to_le_bytes());
    payload[8..12].copy_from_slice(&height.to_le_bytes());
    payload
}

/// Build payload for client->server `SET_WINDOW_SIZE_LIMITS`.
pub fn payload_set_window_size_limits(
    window_id: u32,
    min_width: u32,
    min_height: u32,
    max_width: u32,
    max_height: u32,
) -> [u8; 20] {
    let mut payload = [0u8; 20];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&min_width.to_le_bytes());
    payload[8..12].copy_from_slice(&min_height.to_le_bytes());
    payload[12..16].copy_from_slice(&max_width.to_le_bytes());
    payload[16..20].copy_from_slice(&max_height.to_le_bytes());
    payload
}

/// Build payload for server->client `WINDOW_CREATED`.
pub fn payload_window_created(window_id: u32, shm_size: u64) -> [u8; 12] {
    let mut payload = [0u8; 12];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..12].copy_from_slice(&shm_size.to_le_bytes());
    payload
}

/// Build payload for server->client `WINDOW_RESIZED`.
pub fn payload_window_resized(window_id: u32, shm_size: u64, width: u32, height: u32) -> [u8; 20] {
    let mut payload = [0u8; 20];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..12].copy_from_slice(&shm_size.to_le_bytes());
    payload[12..16].copy_from_slice(&width.to_le_bytes());
    payload[16..20].copy_from_slice(&height.to_le_bytes());
    payload
}

/// Build payload for server->client `WINDOW_CONFIGURE`.
pub fn payload_window_configure(window_id: u32, width: u32, height: u32) -> [u8; 12] {
    let mut payload = [0u8; 12];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&width.to_le_bytes());
    payload[8..12].copy_from_slice(&height.to_le_bytes());
    payload
}

/// Build payload for server->client `WINDOW_STATE_CHANGED`.
///
/// # Arguments
///
/// * `window_id` - Window whose state changed.
/// * `state_flags` - Bitset from [`window_state`].
///
/// # Returns
///
/// The serialized window identifier and presentation-state flags.
pub fn payload_window_state_changed(window_id: u32, state_flags: u32) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&state_flags.to_le_bytes());
    payload
}

/// Build payload for server->client `WINDOW_DESTROYED`.
pub fn payload_window_destroyed(window_id: u32) -> [u8; 4] {
    window_id.to_le_bytes()
}

/// Build payload for server->client `INPUT_EVENT`.
pub fn payload_input_event(
    window_id: u32,
    time: u64,
    type_: u16,
    code: u16,
    value: i32,
) -> [u8; 20] {
    let mut payload = [0u8; 20];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..12].copy_from_slice(&time.to_le_bytes());
    payload[12..14].copy_from_slice(&type_.to_le_bytes());
    payload[14..16].copy_from_slice(&code.to_le_bytes());
    payload[16..20].copy_from_slice(&value.to_le_bytes());
    payload
}

pub fn payload_text_input_create(window_id: u32, seat_id: u32) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&seat_id.to_le_bytes());
    payload
}

pub fn payload_text_input_context_id(context_id: u32) -> [u8; 4] {
    context_id.to_le_bytes()
}

pub fn payload_text_input_created(context_id: u32, serial: u32) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&context_id.to_le_bytes());
    payload[4..8].copy_from_slice(&serial.to_le_bytes());
    payload
}

pub fn payload_text_input_set_cursor_rect(
    context_id: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> [u8; 20] {
    let mut payload = [0u8; 20];
    payload[0..4].copy_from_slice(&context_id.to_le_bytes());
    payload[4..8].copy_from_slice(&x.to_le_bytes());
    payload[8..12].copy_from_slice(&y.to_le_bytes());
    payload[12..16].copy_from_slice(&width.to_le_bytes());
    payload[16..20].copy_from_slice(&height.to_le_bytes());
    payload
}

fn clamp_text_offset(offset: u32, text_len: usize) -> u32 {
    offset.min(text_len as u32)
}

pub fn payload_text_input_set_surrounding_text(
    context_id: u32,
    cursor_byte: u32,
    anchor_byte: u32,
    text: &[u8],
) -> Vec<u8> {
    let mut payload = Vec::new();
    let text_len = text.len().min(TEXT_INPUT_MAX_BYTES);
    let cursor_byte = clamp_text_offset(cursor_byte, text_len);
    let anchor_byte = clamp_text_offset(anchor_byte, text_len);
    payload.extend_from_slice(&context_id.to_le_bytes());
    payload.extend_from_slice(&cursor_byte.to_le_bytes());
    payload.extend_from_slice(&anchor_byte.to_le_bytes());
    payload.extend_from_slice(&(text_len as u32).to_le_bytes());
    payload.extend_from_slice(&text[..text_len]);
    payload
}

pub fn payload_text_input_set_content_type(context_id: u32, hint: u32, purpose: u32) -> [u8; 12] {
    let mut payload = [0u8; 12];
    payload[0..4].copy_from_slice(&context_id.to_le_bytes());
    payload[4..8].copy_from_slice(&hint.to_le_bytes());
    payload[8..12].copy_from_slice(&purpose.to_le_bytes());
    payload
}

pub fn payload_text_input_set_text_change_cause(context_id: u32, cause: u32) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&context_id.to_le_bytes());
    payload[4..8].copy_from_slice(&cause.to_le_bytes());
    payload
}

pub fn payload_text_input_commit_state(context_id: u32, serial: u32) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&context_id.to_le_bytes());
    payload[4..8].copy_from_slice(&serial.to_le_bytes());
    payload
}

pub fn payload_text_input_preedit(
    context_id: u32,
    serial: u32,
    cursor_byte: u32,
    anchor_byte: u32,
    text: &[u8],
    spans: &[u8],
) -> Vec<u8> {
    let mut payload = Vec::new();
    let text_len = text.len().min(TEXT_INPUT_MAX_BYTES);
    let spans_len = spans.len().min(TEXT_INPUT_PREEDIT_SPANS_MAX_BYTES);
    let cursor_byte = clamp_text_offset(cursor_byte, text_len);
    let anchor_byte = clamp_text_offset(anchor_byte, text_len);
    payload.extend_from_slice(&context_id.to_le_bytes());
    payload.extend_from_slice(&serial.to_le_bytes());
    payload.extend_from_slice(&cursor_byte.to_le_bytes());
    payload.extend_from_slice(&anchor_byte.to_le_bytes());
    payload.extend_from_slice(&(text_len as u32).to_le_bytes());
    payload.extend_from_slice(&text[..text_len]);
    payload.extend_from_slice(&(spans_len as u32).to_le_bytes());
    payload.extend_from_slice(&spans[..spans_len]);
    payload
}

pub fn payload_text_input_commit(context_id: u32, serial: u32, text: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    let text_len = text.len().min(TEXT_INPUT_MAX_BYTES);
    payload.extend_from_slice(&context_id.to_le_bytes());
    payload.extend_from_slice(&serial.to_le_bytes());
    payload.extend_from_slice(&(text_len as u32).to_le_bytes());
    payload.extend_from_slice(&text[..text_len]);
    payload
}

pub fn payload_text_input_delete_surrounding_text(
    context_id: u32,
    serial: u32,
    before_bytes: u32,
    after_bytes: u32,
) -> [u8; 16] {
    let mut payload = [0u8; 16];
    payload[0..4].copy_from_slice(&context_id.to_le_bytes());
    payload[4..8].copy_from_slice(&serial.to_le_bytes());
    payload[8..12].copy_from_slice(&before_bytes.to_le_bytes());
    payload[12..16].copy_from_slice(&after_bytes.to_le_bytes());
    payload
}

pub fn payload_text_input_done(context_id: u32, serial: u32) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&context_id.to_le_bytes());
    payload[4..8].copy_from_slice(&serial.to_le_bytes());
    payload
}

pub fn payload_text_input_status(
    context_id: u32,
    serial: u32,
    state: u32,
    mode_id: u32,
    flags: u32,
    mode_label: &[u8],
) -> Vec<u8> {
    let mut payload = Vec::new();
    let mode_label_len = mode_label.len().min(TEXT_INPUT_MAX_BYTES);
    payload.extend_from_slice(&context_id.to_le_bytes());
    payload.extend_from_slice(&serial.to_le_bytes());
    payload.extend_from_slice(&state.to_le_bytes());
    payload.extend_from_slice(&mode_id.to_le_bytes());
    payload.extend_from_slice(&flags.to_le_bytes());
    payload.extend_from_slice(&(mode_label_len as u32).to_le_bytes());
    payload.extend_from_slice(&mode_label[..mode_label_len]);
    payload
}

pub fn payload_ime_register(name: &[u8], capabilities: u32) -> Vec<u8> {
    let mut payload = Vec::new();
    let name_len = name.len().min(TEXT_INPUT_MAX_BYTES);
    payload.extend_from_slice(&capabilities.to_le_bytes());
    payload.extend_from_slice(&(name_len as u32).to_le_bytes());
    payload.extend_from_slice(&name[..name_len]);
    payload
}

pub fn payload_ime_registered(ime_id: u32) -> [u8; 4] {
    ime_id.to_le_bytes()
}

pub fn payload_ime_set_active(ime_id: u32) -> [u8; 4] {
    ime_id.to_le_bytes()
}

pub fn payload_ime_key_handled(key_serial: u32, handled: bool) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&key_serial.to_le_bytes());
    payload[4..8].copy_from_slice(&(handled as u32).to_le_bytes());
    payload
}

pub fn payload_ime_set_preedit(
    context_id: u32,
    cursor_byte: u32,
    anchor_byte: u32,
    text: &[u8],
    spans: &[u8],
) -> Vec<u8> {
    let mut payload = Vec::new();
    let text_len = text.len().min(TEXT_INPUT_MAX_BYTES);
    let spans_len = spans.len().min(TEXT_INPUT_PREEDIT_SPANS_MAX_BYTES);
    let cursor_byte = clamp_text_offset(cursor_byte, text_len);
    let anchor_byte = clamp_text_offset(anchor_byte, text_len);
    payload.extend_from_slice(&context_id.to_le_bytes());
    payload.extend_from_slice(&cursor_byte.to_le_bytes());
    payload.extend_from_slice(&anchor_byte.to_le_bytes());
    payload.extend_from_slice(&(text_len as u32).to_le_bytes());
    payload.extend_from_slice(&text[..text_len]);
    payload.extend_from_slice(&(spans_len as u32).to_le_bytes());
    payload.extend_from_slice(&spans[..spans_len]);
    payload
}

pub fn payload_ime_commit_text(context_id: u32, text: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    let text_len = text.len().min(TEXT_INPUT_MAX_BYTES);
    payload.extend_from_slice(&context_id.to_le_bytes());
    payload.extend_from_slice(&(text_len as u32).to_le_bytes());
    payload.extend_from_slice(&text[..text_len]);
    payload
}

pub fn payload_ime_delete_surrounding_text(
    context_id: u32,
    before_bytes: u32,
    after_bytes: u32,
) -> [u8; 12] {
    let mut payload = [0u8; 12];
    payload[0..4].copy_from_slice(&context_id.to_le_bytes());
    payload[4..8].copy_from_slice(&before_bytes.to_le_bytes());
    payload[8..12].copy_from_slice(&after_bytes.to_le_bytes());
    payload
}

pub fn payload_ime_set_status(
    context_id: u32,
    state: u32,
    mode_id: u32,
    flags: u32,
    mode_label: &[u8],
) -> Vec<u8> {
    let mut payload = Vec::new();
    let mode_label_len = mode_label.len().min(TEXT_INPUT_MAX_BYTES);
    payload.extend_from_slice(&context_id.to_le_bytes());
    payload.extend_from_slice(&state.to_le_bytes());
    payload.extend_from_slice(&mode_id.to_le_bytes());
    payload.extend_from_slice(&flags.to_le_bytes());
    payload.extend_from_slice(&(mode_label_len as u32).to_le_bytes());
    payload.extend_from_slice(&mode_label[..mode_label_len]);
    payload
}

pub fn payload_ime_set_popup_window(
    context_id: u32,
    window_id: u32,
    offset_x: i32,
    offset_y: i32,
    visible: bool,
) -> [u8; 20] {
    let mut payload = [0u8; 20];
    payload[0..4].copy_from_slice(&context_id.to_le_bytes());
    payload[4..8].copy_from_slice(&window_id.to_le_bytes());
    payload[8..12].copy_from_slice(&offset_x.to_le_bytes());
    payload[12..16].copy_from_slice(&offset_y.to_le_bytes());
    payload[16..20].copy_from_slice(&(visible as u32).to_le_bytes());
    payload
}

pub fn payload_ime_grab_keyboard(context_id: u32) -> [u8; 4] {
    context_id.to_le_bytes()
}

pub fn payload_ime_release_keyboard(context_id: u32) -> [u8; 4] {
    context_id.to_le_bytes()
}

pub fn payload_ime_context(
    context_id: u32,
    window_id: u32,
    serial: u32,
    cursor_rect: (i32, i32, u32, u32),
    content_hint: u32,
    content_purpose: u32,
    text_change_cause: u32,
    cursor_byte: u32,
    anchor_byte: u32,
    surrounding_text: &[u8],
) -> Vec<u8> {
    let mut payload = Vec::new();
    let surrounding_text_len = surrounding_text.len().min(TEXT_INPUT_MAX_BYTES);
    let cursor_byte = clamp_text_offset(cursor_byte, surrounding_text_len);
    let anchor_byte = clamp_text_offset(anchor_byte, surrounding_text_len);
    payload.extend_from_slice(&context_id.to_le_bytes());
    payload.extend_from_slice(&window_id.to_le_bytes());
    payload.extend_from_slice(&serial.to_le_bytes());
    payload.extend_from_slice(&cursor_rect.0.to_le_bytes());
    payload.extend_from_slice(&cursor_rect.1.to_le_bytes());
    payload.extend_from_slice(&cursor_rect.2.to_le_bytes());
    payload.extend_from_slice(&cursor_rect.3.to_le_bytes());
    payload.extend_from_slice(&content_hint.to_le_bytes());
    payload.extend_from_slice(&content_purpose.to_le_bytes());
    payload.extend_from_slice(&text_change_cause.to_le_bytes());
    payload.extend_from_slice(&cursor_byte.to_le_bytes());
    payload.extend_from_slice(&anchor_byte.to_le_bytes());
    payload.extend_from_slice(&(surrounding_text_len as u32).to_le_bytes());
    payload.extend_from_slice(&surrounding_text[..surrounding_text_len]);
    payload
}

pub fn payload_ime_deactivate(context_id: u32, serial: u32) -> [u8; 8] {
    payload_text_input_done(context_id, serial)
}

pub fn payload_ime_key_event(
    context_id: u32,
    key_serial: u32,
    window_id: u32,
    time: u64,
    type_: u16,
    code: u16,
    value: i32,
) -> [u8; 28] {
    let mut payload = [0u8; 28];
    payload[0..4].copy_from_slice(&context_id.to_le_bytes());
    payload[4..8].copy_from_slice(&key_serial.to_le_bytes());
    payload[8..12].copy_from_slice(&window_id.to_le_bytes());
    payload[12..20].copy_from_slice(&time.to_le_bytes());
    payload[20..22].copy_from_slice(&type_.to_le_bytes());
    payload[22..24].copy_from_slice(&code.to_le_bytes());
    payload[24..28].copy_from_slice(&value.to_le_bytes());
    payload
}

pub fn payload_ime_reset(context_id: u32, serial: u32) -> [u8; 8] {
    payload_text_input_done(context_id, serial)
}

pub fn payload_ime_trigger(
    context_id: u32,
    serial: u32,
    trigger_id: u32,
    code: u16,
    time: u64,
) -> [u8; 24] {
    let mut payload = [0u8; 24];
    payload[0..4].copy_from_slice(&context_id.to_le_bytes());
    payload[4..8].copy_from_slice(&serial.to_le_bytes());
    payload[8..12].copy_from_slice(&trigger_id.to_le_bytes());
    payload[12..14].copy_from_slice(&code.to_le_bytes());
    payload[16..24].copy_from_slice(&time.to_le_bytes());
    payload
}

/// Build payload for server->client `EXTENSION_INPUT_EVENT`.
/// Payload format: external_client_id (4) + window_id (4) + time (8) + type (2) + code (2) + value (4) = 24 bytes
pub fn payload_extension_input_event(
    external_client_id: u32,
    window_id: u32,
    time: u64,
    type_: u16,
    code: u16,
    value: i32,
) -> [u8; 24] {
    let mut payload = [0u8; 24];
    payload[0..4].copy_from_slice(&external_client_id.to_le_bytes());
    payload[4..8].copy_from_slice(&window_id.to_le_bytes());
    payload[8..16].copy_from_slice(&time.to_le_bytes());
    payload[16..18].copy_from_slice(&type_.to_le_bytes());
    payload[18..20].copy_from_slice(&code.to_le_bytes());
    payload[20..24].copy_from_slice(&value.to_le_bytes());
    payload
}

/// Build payload for server->client `ERROR`.
pub fn payload_error(code: u32) -> [u8; 4] {
    code.to_le_bytes()
}

/// Build payload for client->server `MINIMIZE_WINDOW`.
pub fn payload_minimize_window(window_id: u32) -> [u8; 4] {
    window_id.to_le_bytes()
}

/// Build payload for client->server `MAXIMIZE_WINDOW`.
pub fn payload_maximize_window(window_id: u32) -> [u8; 4] {
    window_id.to_le_bytes()
}

/// Build payload for client->server `RESTORE_WINDOW`.
pub fn payload_restore_window(window_id: u32) -> [u8; 4] {
    window_id.to_le_bytes()
}

/// Build payload for client->server `SET_FULLSCREEN`.
///
/// # Arguments
///
/// * `window_id` - Window that should enter fullscreen.
///
/// # Returns
///
/// The serialized window identifier.
pub fn payload_set_fullscreen(window_id: u32) -> [u8; 4] {
    window_id.to_le_bytes()
}

/// Build payload for client->server `UNSET_FULLSCREEN`.
///
/// # Arguments
///
/// * `window_id` - Window that should leave fullscreen.
///
/// # Returns
///
/// The serialized window identifier.
pub fn payload_unset_fullscreen(window_id: u32) -> [u8; 4] {
    window_id.to_le_bytes()
}

/// Build payload for client->server `SET_POINTER_LOCK`.
///
/// # Arguments
///
/// * `window_id` - Window that should capture or release pointer motion.
/// * `locked` - `true` to capture the pointer, `false` to release it.
///
/// # Returns
///
/// The fixed-width pointer lock request payload.
pub fn payload_set_pointer_lock(window_id: u32, locked: bool) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&(locked as u32).to_le_bytes());
    payload
}

/// Build payload for client->server `SET_CURSOR_ICON`.
///
/// # Arguments
///
/// * `window_id` - Window whose hover cursor should change.
/// * `icon` - Compositor-provided cursor icon to select.
///
/// # Returns
///
/// The fixed-width cursor selection request payload.
pub fn payload_set_cursor_icon(window_id: u32, icon: CursorIcon) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&icon.as_raw().to_le_bytes());
    payload
}

/// Build payload for client->server `SET_CURSOR_THEME`.
///
/// # Arguments
///
/// * `theme_path` - UTF-8 path to an installed theme below `/share/cursors`.
///
/// # Returns
///
/// A length-prefixed cursor theme path.
pub fn payload_set_cursor_theme(theme_path: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&(theme_path.len() as u32).to_le_bytes());
    payload.extend_from_slice(theme_path);
    payload
}

/// Build payload for server->client `POINTER_LOCK_CHANGED`.
///
/// # Arguments
///
/// * `window_id` - Window whose pointer lock state changed.
/// * `locked` - Current compositor-confirmed lock state.
///
/// # Returns
///
/// The fixed-width pointer lock event payload.
pub fn payload_pointer_lock_changed(window_id: u32, locked: bool) -> [u8; 8] {
    payload_set_pointer_lock(window_id, locked)
}

/// Build payload for client->server `SET_WINDOW_TYPE`.
pub fn payload_set_window_type(window_id: u32, window_type: u32) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&window_type.to_le_bytes());
    payload
}

/// Build payload for client->server `SET_WINDOW_OPACITY`.
pub fn payload_set_window_opacity(window_id: u32, opacity: u8) -> [u8; 5] {
    let mut payload = [0u8; 5];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4] = opacity;
    payload
}

/// Build payload for client->server `REGISTER_EXTENSION`.
///
/// Registers a client as an extension server (e.g., Wayland bridge).
/// Extension servers can create windows on behalf of other clients.
///
/// Payload (variable):
/// - extension_name_len: u32 (length of extension name)
/// - extension_name: bytes (UTF-8 string)
pub fn payload_register_extension(extension_name: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(extension_name.len() as u32).to_le_bytes());
    out.extend_from_slice(extension_name);
    out
}

/// Build payload for client->server `EXTENSION_CREATE_WINDOW`.
///
/// Extension servers use this to create windows that will be associated
/// with external clients (e.g., Wayland clients).
///
/// Payload (12 bytes):
/// - external_client_id: u32 (identifier for the external client)
/// - width: u32
/// - height: u32
pub fn payload_extension_create_window(
    external_client_id: u32,
    width: u32,
    height: u32,
) -> [u8; 12] {
    let mut payload = [0u8; 12];
    payload[0..4].copy_from_slice(&external_client_id.to_le_bytes());
    payload[4..8].copy_from_slice(&width.to_le_bytes());
    payload[8..12].copy_from_slice(&height.to_le_bytes());
    payload
}

/// Build payload for server->client `EXTENSION_REGISTERED`.
///
/// Confirms successful extension registration.
///
/// Payload (4 bytes):
/// - extension_id: u32 (assigned extension ID)
pub fn payload_extension_registered(extension_id: u32) -> [u8; 4] {
    extension_id.to_le_bytes()
}

/// Build payload for client->server `SET_WORKAREA`.
pub fn payload_set_workarea(x: i32, y: i32, width: u32, height: u32) -> [u8; 16] {
    let mut payload = [0u8; 16];
    payload[0..4].copy_from_slice(&x.to_le_bytes());
    payload[4..8].copy_from_slice(&y.to_le_bytes());
    payload[8..12].copy_from_slice(&width.to_le_bytes());
    payload[12..16].copy_from_slice(&height.to_le_bytes());
    payload
}

/// Build payload for client->server `SET_WINDOW_RESIZABLE`.
pub fn payload_set_window_resizable(window_id: u32, resizable: bool) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4] = if resizable { 1 } else { 0 };
    payload
}

/// Build payload for server->client `SCREEN_SIZE`.
pub fn payload_screen_size(width: u32, height: u32) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&width.to_le_bytes());
    payload[4..8].copy_from_slice(&height.to_le_bytes());
    payload
}

/// Build payload for server->client `OUTPUT_SCALE`.
///
/// Scale is encoded in milli-units: `1000` means 1.0, `2000` means 2.0.
pub fn payload_output_scale(scale_milli: u32) -> [u8; 4] {
    scale_milli.to_le_bytes()
}

/// Build a shared SGFX buffer registration payload.
///
/// # Arguments
///
/// * `window_id` - Target SWS window.
/// * `buffer_id` - Client-assigned buffer slot identifier.
/// * `generation` - Window-buffer generation, incremented after resize.
/// * `compositor_epoch` - SWS shared-image epoch returned by capability negotiation.
/// * `width` - Image width in pixels.
/// * `height` - Image height in pixels.
///
/// # Returns
///
/// Fixed-width registration payload carried in the same atomic socket record
/// as exactly one GPU image capability.
pub fn payload_register_sgfx_buffer(
    window_id: u32,
    buffer_id: u32,
    generation: u32,
    compositor_epoch: u32,
    width: u32,
    height: u32,
) -> [u8; 24] {
    let mut payload = [0u8; 24];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&buffer_id.to_le_bytes());
    payload[8..12].copy_from_slice(&generation.to_le_bytes());
    payload[12..16].copy_from_slice(&compositor_epoch.to_le_bytes());
    payload[16..20].copy_from_slice(&width.to_le_bytes());
    payload[20..24].copy_from_slice(&height.to_le_bytes());
    payload
}

/// Build an SGFX buffer identity payload.
///
/// # Arguments
///
/// * `window_id` - Target SWS window.
/// * `buffer_id` - Client-assigned buffer slot identifier.
/// * `generation` - Window-buffer generation.
/// * `compositor_epoch` - SWS shared-image epoch returned by capability negotiation.
///
/// # Returns
///
/// Fixed-width identity used by register, commit, destroy, reject, and release.
pub fn payload_sgfx_buffer_identity(
    window_id: u32,
    buffer_id: u32,
    generation: u32,
    compositor_epoch: u32,
) -> [u8; 16] {
    let mut payload = [0u8; 16];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&buffer_id.to_le_bytes());
    payload[8..12].copy_from_slice(&generation.to_le_bytes());
    payload[12..16].copy_from_slice(&compositor_epoch.to_le_bytes());
    payload
}

/// Build an atomic shared SGFX frame commit payload.
///
/// # Arguments
///
/// * `window_id` - Target SWS window.
/// * `buffer_id` - Registered buffer slot identifier.
/// * `generation` - Window-buffer generation.
/// * `compositor_epoch` - Current SWS shared-image epoch.
/// * `commit_serial` - Non-zero client-generated serial for this buffer use.
/// * `damage_rects` - Non-empty bounded list of window-local damage rectangles.
///
/// # Returns
///
/// A complete commit payload, or [`ProtocolError::MalformedPayload`] when the
/// damage list is empty or exceeds [`SGFX_MAX_DAMAGE_RECTS`].
pub fn payload_commit_sgfx_frame(
    window_id: u32,
    buffer_id: u32,
    generation: u32,
    compositor_epoch: u32,
    commit_serial: u64,
    damage_rects: &[SgfxDamageRect],
) -> Result<Vec<u8>, ProtocolError> {
    if commit_serial == 0 || damage_rects.is_empty() || damage_rects.len() > SGFX_MAX_DAMAGE_RECTS {
        return Err(ProtocolError::MalformedPayload);
    }
    let mut payload = Vec::new();
    payload.extend_from_slice(&window_id.to_le_bytes());
    payload.extend_from_slice(&buffer_id.to_le_bytes());
    payload.extend_from_slice(&generation.to_le_bytes());
    payload.extend_from_slice(&compositor_epoch.to_le_bytes());
    payload.extend_from_slice(&commit_serial.to_le_bytes());
    payload.extend_from_slice(&(damage_rects.len() as u32).to_le_bytes());
    for rect in damage_rects {
        payload.extend_from_slice(&rect.x.to_le_bytes());
        payload.extend_from_slice(&rect.y.to_le_bytes());
        payload.extend_from_slice(&rect.width.to_le_bytes());
        payload.extend_from_slice(&rect.height.to_le_bytes());
    }
    Ok(payload)
}

/// Build an asynchronous SGFX frame-rejection payload.
///
/// # Arguments
///
/// * `window_id` - Target SWS window.
/// * `buffer_id` - Registered buffer slot identifier.
/// * `generation` - Window-buffer generation.
/// * `compositor_epoch` - SWS shared-image epoch.
/// * `commit_serial` - Serial supplied by the rejected commit.
/// * `code` - Stable protocol error code explaining the rejection.
///
/// # Returns
///
/// Fixed-width rejection payload that identifies one exact buffer use.
pub fn payload_sgfx_frame_rejected(
    window_id: u32,
    buffer_id: u32,
    generation: u32,
    compositor_epoch: u32,
    commit_serial: u64,
    code: u32,
) -> [u8; 28] {
    let mut payload = [0u8; 28];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&buffer_id.to_le_bytes());
    payload[8..12].copy_from_slice(&generation.to_le_bytes());
    payload[12..16].copy_from_slice(&compositor_epoch.to_le_bytes());
    payload[16..24].copy_from_slice(&commit_serial.to_le_bytes());
    payload[24..28].copy_from_slice(&code.to_le_bytes());
    payload
}

/// Build an asynchronous SGFX buffer-release payload.
///
/// # Arguments
///
/// * `window_id` - Target SWS window.
/// * `buffer_id` - Registered buffer slot identifier.
/// * `generation` - Window-buffer generation.
/// * `compositor_epoch` - SWS shared-image epoch.
/// * `commit_serial` - Serial of the buffer use that is no longer retained.
///
/// # Returns
///
/// Fixed-width release payload that identifies one exact buffer use.
pub fn payload_sgfx_buffer_released(
    window_id: u32,
    buffer_id: u32,
    generation: u32,
    compositor_epoch: u32,
    commit_serial: u64,
) -> [u8; 24] {
    let mut payload = [0u8; 24];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&buffer_id.to_le_bytes());
    payload[8..12].copy_from_slice(&generation.to_le_bytes());
    payload[12..16].copy_from_slice(&compositor_epoch.to_le_bytes());
    payload[16..24].copy_from_slice(&commit_serial.to_le_bytes());
    payload
}

/// Parse the packed rectangle suffix from an SGFX frame commit.
///
/// # Arguments
///
/// * `payload` - Packed 16-byte rectangle records after the commit prefix.
///
/// # Returns
///
/// Parsed rectangles, or [`ProtocolError::MalformedPayload`] for an empty,
/// misaligned, or oversized list.
pub fn parse_sgfx_damage_rects(payload: &[u8]) -> Result<Vec<SgfxDamageRect>, ProtocolError> {
    if payload.is_empty() || payload.len() % 16 != 0 || payload.len() / 16 > SGFX_MAX_DAMAGE_RECTS {
        return Err(ProtocolError::MalformedPayload);
    }
    let mut rects = Vec::new();
    for offset in (0..payload.len()).step_by(16) {
        rects.push(SgfxDamageRect {
            x: read_i32(payload, offset)?,
            y: read_i32(payload, offset + 4)?,
            width: read_u32(payload, offset + 8)?,
            height: read_u32(payload, offset + 12)?,
        });
    }
    Ok(rects)
}

/// Build an SWS capability response payload.
///
/// # Arguments
///
/// * `protocol_version` - Negotiated SWS protocol version.
/// * `capabilities` - Bitmask from [`capabilities`].
/// * `compositor_epoch` - Current shared-image epoch.
/// * `compositor_backend` - Active backend from [`compositor_backends`].
///
/// # Returns
///
/// Fixed-width capability response payload.
pub fn payload_capabilities(
    protocol_version: u32,
    capabilities: u64,
    compositor_epoch: u32,
    compositor_backend: u32,
) -> [u8; 20] {
    let mut payload = [0u8; 20];
    payload[0..4].copy_from_slice(&protocol_version.to_le_bytes());
    payload[4..12].copy_from_slice(&capabilities.to_le_bytes());
    payload[12..16].copy_from_slice(&compositor_epoch.to_le_bytes());
    payload[16..20].copy_from_slice(&compositor_backend.to_le_bytes());
    payload
}

/// Build an SGFX backend-loss notification payload.
///
/// # Arguments
///
/// * `compositor_epoch` - New minimum SWS shared-image epoch. All lower identities
///   are invalid and clients must renegotiate before registering replacements.
///
/// # Returns
///
/// Fixed-width backend-loss payload.
pub fn payload_sgfx_backend_lost(compositor_epoch: u32) -> [u8; 4] {
    compositor_epoch.to_le_bytes()
}

/// Window list entry for WINDOW_LIST message
#[derive(Debug, Clone)]
pub struct WindowListEntry {
    pub window_id: u32,
    pub app_id: std::string::String,
    pub title: std::string::String,
    pub window_type: u32,
    pub visible: bool,
    pub focused: bool,
    pub minimized: bool,
}

/// Build payload for server->client `WINDOW_LIST`.
///
/// Serializes a list of windows into the wire format:
/// - count (u32)
/// - For each window:
///   - window_id (u32)
///   - app_id_length (u32)
///   - app_id_bytes (variable)
///   - title_length (u32)
///   - title_bytes (variable)
///   - window_type (u32)
///   - flags (3 bytes: visible, focused, minimized) + 1 byte padding
pub fn payload_window_list(windows: &[WindowListEntry]) -> Vec<u8> {
    let mut payload = Vec::new();

    // Window count
    payload.extend_from_slice(&(windows.len() as u32).to_le_bytes());

    for entry in windows {
        // Window ID
        payload.extend_from_slice(&entry.window_id.to_le_bytes());

        // App ID length and app_id
        let app_id_bytes = entry.app_id.as_bytes();
        payload.extend_from_slice(&(app_id_bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(app_id_bytes);

        // Title length and title
        let title_bytes = entry.title.as_bytes();
        payload.extend_from_slice(&(title_bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(title_bytes);

        // Window type
        payload.extend_from_slice(&entry.window_type.to_le_bytes());

        // Flags
        payload.push(if entry.visible { 1 } else { 0 });
        payload.push(if entry.focused { 1 } else { 0 });
        payload.push(if entry.minimized { 1 } else { 0 });
        payload.push(0); // padding
    }

    payload
}

/// Parse WINDOW_LIST payload into a list of window entries.
///
/// See `payload_window_list` for the wire format.
pub fn parse_window_list_payload(payload: &[u8]) -> Result<Vec<WindowListEntry>, ProtocolError> {
    if payload.len() < 4 {
        return Err(ProtocolError::MalformedPayload);
    }

    let count = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
    let mut windows = Vec::new();
    let mut offset = 4;

    for _ in 0..count {
        // Window ID (4 bytes)
        if offset + 4 > payload.len() {
            return Err(ProtocolError::MalformedPayload);
        }
        let window_id = u32::from_le_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]);
        offset += 4;

        // App ID length (4 bytes)
        if offset + 4 > payload.len() {
            return Err(ProtocolError::MalformedPayload);
        }
        let app_id_len = u32::from_le_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]) as usize;
        offset += 4;

        // App ID bytes
        if offset + app_id_len > payload.len() {
            return Err(ProtocolError::MalformedPayload);
        }
        let app_id = std::string::String::from_utf8_lossy(&payload[offset..offset + app_id_len])
            .into_owned();
        offset += app_id_len;

        // Title length (4 bytes)
        if offset + 4 > payload.len() {
            return Err(ProtocolError::MalformedPayload);
        }
        let title_len = u32::from_le_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]) as usize;
        offset += 4;

        // Title bytes
        if offset + title_len > payload.len() {
            return Err(ProtocolError::MalformedPayload);
        }
        let title =
            std::string::String::from_utf8_lossy(&payload[offset..offset + title_len]).into_owned();
        offset += title_len;

        // Window type (4 bytes)
        if offset + 4 > payload.len() {
            return Err(ProtocolError::MalformedPayload);
        }
        let window_type = u32::from_le_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]);
        offset += 4;

        // Flags (4 bytes: visible, focused, minimized, padding)
        if offset + 4 > payload.len() {
            return Err(ProtocolError::MalformedPayload);
        }
        let visible = payload[offset] != 0;
        let focused = payload[offset + 1] != 0;
        let minimized = payload[offset + 2] != 0;
        offset += 4;

        windows.push(WindowListEntry {
            window_id,
            app_id,
            title,
            window_type,
            visible,
            focused,
            minimized,
        });
    }

    Ok(windows)
}

/// Build payload for client->server `LAUNCH_OR_FOCUS`.
///
/// Payload format:
/// - app_id_len (u32)
/// - app_id_bytes (variable)
/// - exec_path_len (u32)
/// - exec_path_bytes (variable)
pub fn payload_launch_or_focus(app_id: &[u8], exec_path: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&(app_id.len() as u32).to_le_bytes());
    payload.extend_from_slice(app_id);
    payload.extend_from_slice(&(exec_path.len() as u32).to_le_bytes());
    payload.extend_from_slice(exec_path);
    payload
}

/// Build payload for client->server `FOCUS_WINDOW`.
pub fn payload_focus_window(window_id: u32) -> Vec<u8> {
    window_id.to_le_bytes().to_vec()
}

/// Build payload for server->client `FOCUS_CHANGED`.
///
/// Payload format:
/// - window_id (u32)
/// - app_id_len (u32)
/// - app_id_bytes (variable, max 128)
/// - app_name_len (u32)
/// - app_name_bytes (variable, max 128)
/// - title_len (u32)
/// - title_bytes (variable, max 256)
/// - menu_titles_len (u32)
/// - menu_titles_bytes (variable, max 2048, format: "menu1|menu2|menu3")
pub fn payload_focus_changed(
    window_id: u32,
    app_id: &[u8],
    app_name: &[u8],
    title: &[u8],
    menu_titles: &[u8],
) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&window_id.to_le_bytes());

    let app_id_len = app_id.len().min(128);
    payload.extend_from_slice(&(app_id_len as u32).to_le_bytes());
    payload.extend_from_slice(&app_id[..app_id_len]);

    let app_name_len = app_name.len().min(128);
    payload.extend_from_slice(&(app_name_len as u32).to_le_bytes());
    payload.extend_from_slice(&app_name[..app_name_len]);

    let title_len = title.len().min(256);
    payload.extend_from_slice(&(title_len as u32).to_le_bytes());
    payload.extend_from_slice(&title[..title_len]);

    let menu_titles_len = menu_titles.len().min(2048);
    payload.extend_from_slice(&(menu_titles_len as u32).to_le_bytes());
    payload.extend_from_slice(&menu_titles[..menu_titles_len]);

    payload
}

/// Build payload for server->client `ACTIVE_APP_CHANGED`.
///
/// Broadcast when the active application changes (normal window gains focus).
/// Payload format is the same as FOCUS_CHANGED:
/// - window_id (u32)
/// - app_id_len (u32)
/// - app_id_bytes (variable, max 128)
/// - app_name_len (u32)
/// - app_name_bytes (variable, max 128)
/// - title_len (u32)
/// - title_bytes (variable, max 256)
/// - menu_titles_len (u32)
/// - menu_titles_bytes (variable, max 2048, format: "menu1|menu2|menu3")
pub fn payload_active_app_changed(
    window_id: u32,
    app_id: &[u8],
    app_name: &[u8],
    title: &[u8],
    menu_titles: &[u8],
) -> Vec<u8> {
    payload_focus_changed(window_id, app_id, app_name, title, menu_titles)
}

/// Build payload for server->client `MENU_ITEM_ACTIVATED`.
///
/// Payload format:
/// - window_id (u32)
/// - menu_item_id_len (u32, max 128)
/// - menu_item_id_bytes (variable)
pub fn payload_menu_item_activated(window_id: u32, menu_item_id: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    let menu_item_id_len = menu_item_id.len().min(128);
    payload.extend_from_slice(&window_id.to_le_bytes());
    payload.extend_from_slice(&(menu_item_id_len as u32).to_le_bytes());
    payload.extend_from_slice(&menu_item_id[..menu_item_id_len]);
    payload
}

/// Build payload for client->server `SET_WINDOW_HAS_ALPHA_CONTENT`.
pub fn payload_set_window_has_alpha_content(window_id: u32, has_alpha: bool) -> [u8; 5] {
    let mut payload = [0u8; 5];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4] = if has_alpha { 1 } else { 0 };
    payload
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::{
        CURSOR_THEME_PATH_MAX_BYTES, ClientMessageRef, CursorIcon, MessageHeader, ProtocolError,
        ServerMessage, WindowPlacement, client_msg, encode_routed_frame, error_codes,
        parse_client_message, parse_server_message, payload_activation_token,
        payload_create_window_with_placement,
        payload_create_window_with_placement_and_activation_token,
        payload_create_window_with_position, payload_error, payload_pointer_lock_changed,
        payload_request_activation_token, payload_set_cursor_icon, payload_set_cursor_theme,
        payload_set_fullscreen, payload_set_pointer_lock, payload_unset_fullscreen,
        payload_window_state_changed, server_msg, window_placement, window_state,
    };

    #[test]
    fn create_window_placement_preserves_focus_policies() {
        let payload = payload_create_window_with_placement(
            b"org.example.app",
            b"Example",
            b"",
            640,
            480,
            0,
            false,
            false,
            true,
            window_placement::CENTERED,
            0,
            0,
        );

        let ClientMessageRef::CreateWindow {
            resizable,
            focus_on_create,
            active_on_focus,
            initial_position,
            ..
        } = parse_client_message(client_msg::CREATE_WINDOW, &payload).unwrap()
        else {
            panic!("expected CREATE_WINDOW");
        };

        assert!(!resizable);
        assert!(!focus_on_create);
        assert!(active_on_focus);
        assert_eq!(initial_position, WindowPlacement::Centered);
    }

    #[test]
    fn legacy_window_position_is_parsed_as_absolute() {
        let payload = payload_create_window_with_position(
            b"org.example.app",
            b"Example",
            b"",
            640,
            480,
            0,
            true,
            true,
            true,
            -20,
            30,
        );

        let ClientMessageRef::CreateWindow {
            initial_position, ..
        } = parse_client_message(client_msg::CREATE_WINDOW, &payload).unwrap()
        else {
            panic!("expected CREATE_WINDOW");
        };

        assert_eq!(
            initial_position,
            WindowPlacement::Absolute { x: -20, y: 30 }
        );
    }

    #[test]
    fn create_window_preserves_activation_token() {
        let payload = payload_create_window_with_placement_and_activation_token(
            b"org.example.app",
            b"Example",
            b"",
            640,
            480,
            0,
            true,
            true,
            true,
            window_placement::DEFAULT,
            0,
            0,
            b"sws-token",
        );

        let ClientMessageRef::CreateWindow {
            activation_token,
            initial_position,
            ..
        } = parse_client_message(client_msg::CREATE_WINDOW, &payload).unwrap()
        else {
            panic!("expected CREATE_WINDOW");
        };

        assert_eq!(activation_token, Some(b"sws-token".as_slice()));
        assert_eq!(initial_position, WindowPlacement::Default);
    }

    #[test]
    fn activation_token_request_preserves_source_and_target() {
        let payload = payload_request_activation_token(42, b"org.example.target");

        let ClientMessageRef::RequestActivationToken {
            source_window_id,
            target_app_id,
        } = parse_client_message(client_msg::REQUEST_ACTIVATION_TOKEN, &payload).unwrap()
        else {
            panic!("expected REQUEST_ACTIVATION_TOKEN");
        };

        assert_eq!(source_window_id, 42);
        assert_eq!(target_app_id, b"org.example.target");
    }

    #[test]
    fn activation_token_response_is_opaque() {
        let payload = payload_activation_token(b"sws-token");

        let ServerMessage::ActivationToken { token, token_len } =
            parse_server_message(server_msg::ACTIVATION_TOKEN, &payload).unwrap()
        else {
            panic!("expected ACTIVATION_TOKEN");
        };

        assert_eq!(&token[..token_len as usize], b"sws-token");
    }

    #[test]
    fn fullscreen_requests_preserve_window_id() {
        let set_payload = payload_set_fullscreen(42);
        assert_eq!(
            parse_client_message(client_msg::SET_FULLSCREEN, &set_payload).unwrap(),
            ClientMessageRef::SetFullscreen { window_id: 42 }
        );

        let unset_payload = payload_unset_fullscreen(42);
        assert_eq!(
            parse_client_message(client_msg::UNSET_FULLSCREEN, &unset_payload).unwrap(),
            ClientMessageRef::UnsetFullscreen { window_id: 42 }
        );
    }

    #[test]
    fn window_state_changed_preserves_flags() {
        let flags = window_state::MAXIMIZED | window_state::FULLSCREEN;
        let payload = payload_window_state_changed(77, flags);
        assert_eq!(
            parse_server_message(server_msg::WINDOW_STATE_CHANGED, &payload).unwrap(),
            ServerMessage::WindowStateChanged {
                window_id: 77,
                state_flags: flags,
            }
        );
    }

    #[test]
    fn pointer_lock_request_and_event_round_trip() {
        for locked in [false, true] {
            let request = payload_set_pointer_lock(91, locked);
            assert_eq!(
                parse_client_message(client_msg::SET_POINTER_LOCK, &request).unwrap(),
                ClientMessageRef::SetPointerLock {
                    window_id: 91,
                    locked,
                }
            );

            let event = payload_pointer_lock_changed(91, locked);
            assert_eq!(
                parse_server_message(server_msg::POINTER_LOCK_CHANGED, &event).unwrap(),
                ServerMessage::PointerLockChanged {
                    window_id: 91,
                    locked,
                }
            );
        }
    }

    #[test]
    fn pointer_lock_rejects_malformed_payloads() {
        assert_eq!(
            parse_client_message(client_msg::SET_POINTER_LOCK, &[0; 7]),
            Err(ProtocolError::MalformedPayload)
        );
        let mut invalid_bool = payload_set_pointer_lock(91, false);
        invalid_bool[4..8].copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(
            parse_client_message(client_msg::SET_POINTER_LOCK, &invalid_bool),
            Err(ProtocolError::MalformedPayload)
        );
        assert_eq!(
            parse_server_message(server_msg::POINTER_LOCK_CHANGED, &invalid_bool),
            Err(ProtocolError::MalformedPayload)
        );
    }

    #[test]
    fn cursor_icon_request_round_trips_all_icons() {
        for icon in CursorIcon::ALL {
            let payload = payload_set_cursor_icon(73, icon);
            assert_eq!(
                parse_client_message(client_msg::SET_CURSOR_ICON, &payload).unwrap(),
                ClientMessageRef::SetCursorIcon {
                    window_id: 73,
                    icon,
                }
            );
        }
    }

    #[test]
    fn cursor_icon_rejects_unknown_wire_values() {
        let mut payload = payload_set_cursor_icon(73, CursorIcon::Arrow);
        payload[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            parse_client_message(client_msg::SET_CURSOR_ICON, &payload),
            Err(ProtocolError::MalformedPayload)
        );
    }

    #[test]
    fn cursor_theme_request_and_response_round_trip() {
        let path = b"/share/cursors/default";
        let payload = payload_set_cursor_theme(path);
        assert_eq!(
            parse_client_message(client_msg::SET_CURSOR_THEME, &payload).unwrap(),
            ClientMessageRef::SetCursorTheme { theme_path: path }
        );
        assert_eq!(
            parse_server_message(server_msg::CURSOR_THEME_CHANGED, &[]).unwrap(),
            ServerMessage::CursorThemeChanged
        );
    }

    #[test]
    fn cursor_theme_rejects_empty_or_oversized_paths() {
        assert_eq!(
            parse_client_message(client_msg::SET_CURSOR_THEME, &payload_set_cursor_theme(b"")),
            Err(ProtocolError::MalformedPayload)
        );

        let oversized = vec![b'a'; CURSOR_THEME_PATH_MAX_BYTES + 1];
        assert_eq!(
            parse_client_message(
                client_msg::SET_CURSOR_THEME,
                &payload_set_cursor_theme(&oversized)
            ),
            Err(ProtocolError::MalformedPayload)
        );
        assert_eq!(
            parse_server_message(server_msg::CURSOR_THEME_CHANGED, &[0]),
            Err(ProtocolError::MalformedPayload)
        );
    }

    #[test]
    fn pointer_lock_correlated_frames_preserve_response_header() {
        let cases = [
            (
                server_msg::POINTER_LOCK_CHANGED,
                7,
                payload_pointer_lock_changed(91, true).to_vec(),
            ),
            (
                server_msg::POINTER_LOCK_CHANGED,
                8,
                payload_pointer_lock_changed(91, true).to_vec(),
            ),
            (
                server_msg::ERROR,
                9,
                payload_error(error_codes::POINTER_LOCK_DENIED).to_vec(),
            ),
        ];

        for (msg_type, request_id, payload) in cases {
            let frame = encode_routed_frame(
                msg_type,
                MessageHeader::FLAG_IS_RESPONSE,
                request_id,
                &payload,
            );
            let header =
                MessageHeader::from_le_bytes(frame[..MessageHeader::SIZE].try_into().unwrap());
            assert!(header.is_response());
            assert_eq!(header.request_id, request_id);
            assert_eq!(header.msg_type_u32(), msg_type);
            assert_eq!(&frame[MessageHeader::SIZE..], payload.as_slice());
            parse_server_message(msg_type, &frame[MessageHeader::SIZE..]).unwrap();
        }
    }
}
