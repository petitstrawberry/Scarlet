//! Scarlet Window Server (SWS) IPC protocol.
//!
//! This crate is the single source of truth for both the SWS server (`sws`)
//! and clients (`sws_client`) for message IDs, framing, and parsing.
//!
//! Wire format
//! -----------
//! Each message is framed as:
//! - Header (8 bytes, little-endian)
//!   - `msg_type: u32`
//!   - `payload_size: u32`
//! - Payload (`payload_size` bytes)
//!
//! See `docs/sws_ipc_protocol.md` for the detailed specification.

#![no_std]

extern crate scarlet_std as std;

use std::vec::Vec;

/// Maximum payload we accept from the socket.
///
/// This prevents unbounded allocations on malformed frames.
pub const MAX_PAYLOAD_SIZE: usize = 1024 * 1024; // 1 MiB

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

    // Input method service messages (220-239)
    pub const IME_REGISTER: u32 = 220;
    pub const IME_SET_ACTIVE: u32 = 221;
    pub const IME_KEY_HANDLED: u32 = 222;
    pub const IME_SET_PREEDIT: u32 = 223;
    pub const IME_COMMIT_TEXT: u32 = 224;
    pub const IME_DELETE_SURROUNDING_TEXT: u32 = 225;
    pub const IME_SET_CANDIDATES: u32 = 226;
    pub const IME_HIDE_CANDIDATES: u32 = 227;
    pub const IME_GRAB_KEYBOARD: u32 = 228;
    pub const IME_RELEASE_KEYBOARD: u32 = 229;
    pub const IME_SET_STATUS: u32 = 230;
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

    // Text input client events (200-219)
    pub const TEXT_INPUT_CREATED: u32 = 200;
    pub const TEXT_INPUT_PREEDIT: u32 = 201;
    pub const TEXT_INPUT_COMMIT: u32 = 202;
    pub const TEXT_INPUT_DELETE_SURROUNDING_TEXT: u32 = 203;
    pub const TEXT_INPUT_DONE: u32 = 204;
    pub const TEXT_INPUT_CANDIDATES: u32 = 205;
    pub const TEXT_INPUT_HIDE_CANDIDATES: u32 = 206;
    pub const TEXT_INPUT_STATUS: u32 = 207;

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

/// Maximum UTF-8 bytes used for candidate payloads.
pub const TEXT_INPUT_CANDIDATES_MAX_BYTES: usize = 2048;

/// Maximum bytes used for binary preedit span payloads.
pub const TEXT_INPUT_PREEDIT_SPANS_MAX_BYTES: usize = 512;

/// Maximum bytes used for binary structured candidate payloads.
pub const TEXT_INPUT_CANDIDATE_LIST_MAX_BYTES: usize = 4096;

/// IME service capabilities advertised by `IME_REGISTER`.
pub mod ime_capabilities {
    pub const KEYBOARD_GRAB: u32 = 1 << 0;
    pub const SURROUNDING_TEXT: u32 = 1 << 1;
    pub const DELETE_SURROUNDING_TEXT: u32 = 1 << 2;
    pub const STYLED_PREEDIT: u32 = 1 << 3;
    pub const CANDIDATE_LIST: u32 = 1 << 4;
    pub const STATUS: u32 = 1 << 5;
    pub const OWN_CANDIDATE_UI: u32 = 1 << 6;
    pub const PER_CONTEXT_STATE: u32 = 1 << 7;
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
}

/// Message header.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageHeader {
    pub msg_type: u32,
    pub payload_size: u32,
}

impl MessageHeader {
    pub const SIZE: usize = 8;

    pub fn to_le_bytes(self) -> [u8; Self::SIZE] {
        let mut out = [0u8; Self::SIZE];
        out[0..4].copy_from_slice(&self.msg_type.to_le_bytes());
        out[4..8].copy_from_slice(&self.payload_size.to_le_bytes());
        out
    }

    pub fn from_le_bytes(bytes: [u8; Self::SIZE]) -> Self {
        let msg_type = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let payload_size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        Self {
            msg_type,
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
    let header = MessageHeader {
        msg_type,
        payload_size: payload.len() as u32,
    };
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
        initial_x: Option<i32>,
        initial_y: Option<i32>,
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
    ImeSetCandidates {
        context_id: u32,
        selected_index: u32,
        page_start: u32,
        page_size: u32,
        anchor_byte: u32,
        candidates: &'a [u8],
    },
    ImeHideCandidates {
        context_id: u32,
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
    TextInputCandidates {
        context_id: u32,
        serial: u32,
        selected_index: u32,
        page_start: u32,
        page_size: u32,
        anchor_byte: u32,
        candidates: [u8; TEXT_INPUT_CANDIDATE_LIST_MAX_BYTES],
        candidates_len: u32,
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
    TextInputHideCandidates {
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
            let resizable = if payload.len() == offset + 16 || payload.len() == offset + 24 {
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
            if payload.len() == offset + 24 || payload.len() == offset + 32 {
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
            let (initial_x, initial_y) = if payload.len() == offset + 32 {
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
                (Some(x), Some(y))
            } else {
                (None, None)
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
                initial_x,
                initial_y,
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
            if payload.len() < 24 {
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
        client_msg::IME_SET_CANDIDATES => {
            if payload.len() < 24 {
                return Err(ProtocolError::MalformedPayload);
            }
            let context_id = read_u32(payload, 0)?;
            let selected_index = read_u32(payload, 4)?;
            let page_start = read_u32(payload, 8)?;
            let page_size = read_u32(payload, 12)?;
            let anchor_byte = read_u32(payload, 16)?;
            let candidates =
                read_len_prefixed_bytes(payload, 20, TEXT_INPUT_CANDIDATE_LIST_MAX_BYTES)?;
            Ok(ClientMessageRef::ImeSetCandidates {
                context_id,
                selected_index,
                page_start,
                page_size,
                anchor_byte,
                candidates,
            })
        }
        client_msg::IME_HIDE_CANDIDATES => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::ImeHideCandidates {
                context_id: read_u32(payload, 0)?,
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
            if payload.len() < 28 {
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
        server_msg::TEXT_INPUT_CANDIDATES => {
            if payload.len() < 28 {
                return Err(ProtocolError::MalformedPayload);
            }
            let context_id = read_u32(payload, 0)?;
            let serial = read_u32(payload, 4)?;
            let selected_index = read_u32(payload, 8)?;
            let page_start = read_u32(payload, 12)?;
            let page_size = read_u32(payload, 16)?;
            let anchor_byte = read_u32(payload, 20)?;
            let candidates =
                read_len_prefixed_bytes(payload, 24, TEXT_INPUT_CANDIDATE_LIST_MAX_BYTES)?;
            let (candidates, candidates_len) = copy_bounded(candidates)?;
            Ok(ServerMessage::TextInputCandidates {
                context_id,
                serial,
                selected_index,
                page_start,
                page_size,
                anchor_byte,
                candidates,
                candidates_len,
            })
        }
        server_msg::TEXT_INPUT_HIDE_CANDIDATES => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ServerMessage::TextInputHideCandidates {
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

pub fn payload_text_input_set_surrounding_text(
    context_id: u32,
    cursor_byte: u32,
    anchor_byte: u32,
    text: &[u8],
) -> Vec<u8> {
    let mut payload = Vec::new();
    let text_len = text.len().min(TEXT_INPUT_MAX_BYTES);
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

pub fn payload_text_input_candidates(
    context_id: u32,
    serial: u32,
    selected_index: u32,
    page_start: u32,
    page_size: u32,
    anchor_byte: u32,
    candidates: &[u8],
) -> Vec<u8> {
    let mut payload = Vec::new();
    let candidates_len = candidates.len().min(TEXT_INPUT_CANDIDATE_LIST_MAX_BYTES);
    payload.extend_from_slice(&context_id.to_le_bytes());
    payload.extend_from_slice(&serial.to_le_bytes());
    payload.extend_from_slice(&selected_index.to_le_bytes());
    payload.extend_from_slice(&page_start.to_le_bytes());
    payload.extend_from_slice(&page_size.to_le_bytes());
    payload.extend_from_slice(&anchor_byte.to_le_bytes());
    payload.extend_from_slice(&(candidates_len as u32).to_le_bytes());
    payload.extend_from_slice(&candidates[..candidates_len]);
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

pub fn payload_text_input_hide_candidates(context_id: u32, serial: u32) -> [u8; 8] {
    payload_text_input_done(context_id, serial)
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

pub fn payload_ime_set_candidates(
    context_id: u32,
    selected_index: u32,
    page_start: u32,
    page_size: u32,
    anchor_byte: u32,
    candidates: &[u8],
) -> Vec<u8> {
    let mut payload = Vec::new();
    let candidates_len = candidates.len().min(TEXT_INPUT_CANDIDATE_LIST_MAX_BYTES);
    payload.extend_from_slice(&context_id.to_le_bytes());
    payload.extend_from_slice(&selected_index.to_le_bytes());
    payload.extend_from_slice(&page_start.to_le_bytes());
    payload.extend_from_slice(&page_size.to_le_bytes());
    payload.extend_from_slice(&anchor_byte.to_le_bytes());
    payload.extend_from_slice(&(candidates_len as u32).to_le_bytes());
    payload.extend_from_slice(&candidates[..candidates_len]);
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

pub fn payload_ime_hide_candidates(context_id: u32) -> [u8; 4] {
    context_id.to_le_bytes()
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
