# Scarlet Window Server (SWS) IPC Protocol

This document describes the wire protocol used between the Scarlet Window Server (`sws`) and clients.

The canonical implementation is the `sws_protocol` crate located at `user/lib/sws-protocol`.

Client-side reference implementations:

- Low-level client library: `sws-client` (crate name `sws_client`) in `user/lib/sws-client`
- High-level UI toolkit: `scarlet-ui` in `user/lib/scarlet-ui`

## Transport

- Endpoint: Unix-domain socket (VFS socket)
- Default path: `/tmp/sws.sock`
- Byte order: little-endian for all integer fields

## Configuration

SWS reads `/etc/sws/config.toml` at startup. The current implementation accepts
`[output] scale` / `scale_milli`, `[cursor] theme`, and common SWS-level
`[keybindings]`. `SET_CURSOR_THEME` may validate, persist, and activate a new
cursor theme without restarting SWS.

`cursor.theme` names a directory containing `theme.toml` and its PNG images.
The manifest owns `image_scale` plus each cursor state's `image`, `hotspot_x`,
and `hotspot_y`. A state may instead use `alias = "other_state"`; aliases inherit
both image and hotspot, may refer forward, and must not form cycles. Static PNG
and full-frame APNG images are supported; APNG frame timing is advanced by the
compositor while the cursor is visible.

`keybindings.ime_toggle` is a compositor shortcut that emits `IME_TRIGGER` to
the active IME for the focused text-input context. It is not an IME-specific
setting and SWS still does not implement conversion behavior. The value may be a
single key binding string or an array of strings, for example:

```toml
[keybindings]
ime_toggle = ["Ctrl+Backslash", "Ctrl+Space", "Zenkaku_Hankaku"]
```

Supported modifiers are `Ctrl`, `Shift`, `Alt`, and `Meta`; supported named keys
include `Backslash`, `Space`, `Zenkaku_Hankaku`, `Henkan`, `Muhenkan`, and
`Hangul`. A numeric event code may be written as `keycode:N`.

## Framing

All messages are framed.

### Header

The header is always **8 bytes**:

| Offset | Size | Field         | Type | Notes |
|--------|------|---------------|------|-------|
| 0      | 2    | `msg_type`     | u16  | Message type ID |
| 2      | 1    | `flags`        | u8   | Routing flags |
| 3      | 1    | `request_id`   | u8   | Per-connection request ID |
| 4      | 4    | `payload_size` | u32  | Payload length in bytes |

Defined header flags:

| Flag | Name | Meaning |
|------|------|---------|
| `0x01` | `IS_RESPONSE` | This frame is a server response to a client request. |

`request_id` is scoped to a single socket connection. A client sets a non-zero
`request_id` on requests that expect a synchronous response. The server copies
that ID into the matching response frame and sets `IS_RESPONSE`.

`request_id = 0` is reserved for unrouted, fire-and-forget traffic. Direct
protocol senders inside the tree may use it when they do not need strict
response correlation.

Clients must not identify a synchronous response by message type alone. A client
waiting for a synchronous reply must keep reading frames until it receives a
frame with both `IS_RESPONSE` set and the expected `request_id`. Non-response
frames received while waiting are asynchronous events and should be queued for
normal dispatch.

### Payload

Immediately following the header is `payload_size` bytes of payload.

Implementations **must** reject frames with excessively large payloads to avoid unbounded allocations. The current implementation uses a fixed maximum of 1 MiB.

## Message Types

### Client → Server

#### `CREATE_WINDOW` (type = 1)

Payload (8 bytes):

| Offset | Size | Field    | Type |
|--------|------|----------|------|
| 0      | 4    | `width`  | u32  |
| 4      | 4    | `height` | u32  |

#### `DESTROY_WINDOW` (type = 2)

Payload (4 bytes):

| Offset | Size | Field       | Type |
|--------|------|-------------|------|
| 0      | 4    | `window_id` | u32  |

#### `SET_WINDOW_TITLE` (type = 3)

Payload (variable):

| Offset | Size | Field       | Type | Notes |
|--------|------|-------------|------|-------|
| 0      | 4    | `window_id` | u32  | |
| 4      | 4    | `title_len` | u32  | Length of `title` in bytes |
| 8      | N    | `title`     | bytes | UTF-8 recommended; not validated |

The payload length must equal `8 + title_len`.

#### `UPDATE_BUFFER` (type = 4)

This is currently treated as a **damage notification only** (no pixel payload is transferred).

> Implementation note: clients continue to receive and map the window shared
> memory buffer, write pixels into it, and send damage notifications only. SWS
> may internally pin and import that SharedMemory as GPU backing for composition.
> Resize or buffer replacement creates a new backing generation and releases the
> prior one first. Buffers that are not represented by SharedMemory retain the
> CPU/private upload fallback.

Payload (20 bytes):

| Offset | Size | Field    | Type |
|--------|------|----------|------|
| 0      | 4    | `window_id` | u32 |
| 4      | 4    | `x`      | i32 |
| 8      | 4    | `y`      | i32 |
| 12     | 4    | `width`  | u32 |
| 16     | 4    | `height` | u32 |

#### `REQUEST_MOVE_WINDOW` (type = 5)

Payload (4 bytes): `window_id: u32`

Semantics:

- This is a **user-initiated interactive move request** (e.g. title-bar drag).
- Clients should send this once when the user starts a drag gesture.
- After receiving this request, the compositor is expected to enter a temporary
	**move-drag mode** for `window_id` where it uses global pointer motion to update
	the window position until the initiating button is released.
- During move-drag mode, the compositor should treat the pointer as **grabbed** by
	the compositor (i.e. avoid routing pointer motion to other clients) to keep the
	gesture robust.
- The compositor may raise the window to the front. Whether the window becomes
	focused is a compositor policy (typically handled by click-to-focus), and is
	not required for moving.

#### `MOVE_WINDOW` (type = 6)

Payload (12 bytes):

| Offset | Size | Field       | Type |
|--------|------|-------------|------|
| 0      | 4    | `window_id` | u32  |
| 4      | 4    | `x`         | i32  |
| 8      | 4    | `y`         | i32  |

Semantics:

- This is a **programmatic reposition** request to set an absolute window origin.
- Intended for non-interactive use cases (e.g. centering a dialog, restoring a
	saved layout).
- Interactive drag moves should prefer `REQUEST_MOVE_WINDOW` so the compositor can
	own the pointer-grab and state machine.

#### `SET_WINDOW_PARENT` (type = 7)

Payload (8 bytes):

| Offset | Size | Field       | Type |
|--------|------|-------------|------|
| 0      | 4    | `window_id` | u32  |
| 4      | 4    | `parent_id` | u32  |

Semantics:

- Set (or clear) the logical parent relationship for a window.
- `parent_id == 0` means "no parent".
- Intended for transient windows (dialogs/popups) so the compositor can:
	- Keep the child stacked above the parent when raising.
	- Move the child together when the parent is interactively moved.
- Parent relationships are a compositor policy; clients must not assume focus changes.

#### `SET_WINDOW_TRANSIENT_FLAGS` (type = 8)

Payload (8 bytes):

| Offset | Size | Field       | Type |
|--------|------|-------------|------|
| 0      | 4    | `window_id` | u32  |
| 4      | 4    | `flags`     | u32  |

Semantics:

- Configure transient behavior flags for a window (bitset).
- Flags from `sws_protocol::transient_flags`:
	- `FOLLOW_PARENT_MOVE = 0x01`: Child moves with parent during interactive moves.
	- `RAISE_WITH_PARENT = 0x02`: Raising parent raises the child group.

#### `RESIZE_WINDOW` (type = 9)

Payload (12 bytes):

| Offset | Size | Field       | Type |
|--------|------|-------------|------|
| 0      | 4    | `window_id` | u32  |
| 4      | 4    | `width`     | u32  |
| 8      | 4    | `height`    | u32  |

Semantics:

- Request a window buffer resize for a window owned by the calling client.
- The server **must** validate that `window_id` belongs to the requesting client and **must reject** the request if the window is not owned by that client.
- For valid requests, the server allocates a new shared-memory buffer and responds with `WINDOW_RESIZED` + new SHM handle. The `WINDOW_RESIZED` frame is marked `IS_RESPONSE` and carries the request's `request_id`.

#### `GET_SCREEN_SIZE` (type = 10)

Payload: empty.

Semantics:

- Request the current compositor display size in pixels.
- The server responds with `SCREEN_SIZE`. The response frame is marked `IS_RESPONSE` and carries the request's `request_id`.

#### `SET_WINDOW_SIZE_LIMITS` (type = 16)

Payload (20 bytes):

| Offset | Size | Field        | Type | Notes |
|--------|------|--------------|------|-------|
| 0      | 4    | `window_id`  | u32  | |
| 4      | 4    | `min_width`  | u32  | 0 = no minimum |
| 8      | 4    | `min_height` | u32  | 0 = no minimum |
| 12     | 4    | `max_width`  | u32  | 0 = no maximum |
| 16     | 4    | `max_height` | u32  | 0 = no maximum |

Semantics:

- Set minimum and maximum size constraints for a window (in pixels).
- Used by the compositor during interactive resize operations.

#### `MINIMIZE_WINDOW` (type = 17)

Payload (4 bytes):

| Offset | Size | Field       | Type |
|--------|------|-------------|------|
| 0      | 4    | `window_id` | u32  |

Semantics:

- Hide the window from display but keep it in the window list.
- The window's `visible` flag is set to false.
- If the window is fullscreen, SWS first leaves fullscreen and restores its
  preceding geometry. Minimizing therefore releases the output for another
  fullscreen window.
- The window can be restored with `RESTORE_WINDOW`.

#### `MAXIMIZE_WINDOW` (type = 18)

Payload (4 bytes):

| Offset | Size | Field       | Type |
|--------|------|-------------|------|
| 0      | 4    | `window_id` | u32  |

Semantics:

- Expand a normal application window to the compositor workarea. Shell surfaces
  such as the taskbar remain visible; this is not fullscreen.
- While the window remains maximized, later workarea changes reflow it and emit
  `WINDOW_CONFIGURE` with the new size.
- The compositor saves the current position and size for restoration.
- The window can be restored with `RESTORE_WINDOW`.
- Windows that specify explicit maximum size limits (i.e. `max_width` or `max_height` is non-zero) are **not** maximized; clients SHOULD NOT send `MAXIMIZE_WINDOW` for such windows and MUST be prepared for the request to be ignored.

#### `RESTORE_WINDOW` (type = 19)

Payload (4 bytes):

| Offset | Size | Field       | Type |
|--------|------|-------------|------|
| 0      | 4    | `window_id` | u32  |

Semantics:

- Restore a minimized window (makes it visible) or a maximized window (restores saved geometry).
- Fullscreen has its own state and is left only with `UNSET_FULLSCREEN`.
- If the window is neither minimized nor maximized, this message has no effect.

#### `SET_WINDOW_TYPE` (type = 20)

Payload (8 bytes):

| Offset | Size | Field        | Type |
|--------|------|--------------|------|
| 0      | 4    | `window_id`  | u32  |
| 4      | 4    | `window_type`| u32  |

Semantics:

- Set the window type for Z-order management.
- Window type constants from `sws_protocol::window_types`:
	- `NORMAL = 0`: Standard application window (default)
	- `ALWAYS_ON_TOP = 1`: Always stays above normal windows
	- `TASKBAR = 2`: Taskbar/panel window
	- `DESKTOP = 3`: Desktop background window
	- `IME_POPUP = 4`: Input-method-owned popup surface
- Default Z-order grouping (bottom to top): Desktop → Normal → Taskbar → AlwaysOnTop → ImePopup.
- A fullscreen window is promoted above Taskbar and AlwaysOnTop windows while it
  remains below IME popup UI. Transient descendants of the fullscreen window
  remain above their parent.
- The effective Z-order is dynamically adjusted by the window server when windows are raised (see `raise_to_top_with_type` in the implementation); depending on which type is raised, windows of other types may end up above or below it.
- Within each type, windows generally maintain relative order, except when explicitly changed by focus and raise operations.

#### `SET_WINDOW_OPACITY` (type = 21)

Payload (5 bytes):

| Offset | Size | Field       | Type | Notes |
|--------|------|-------------|------|-------|
| 0      | 4    | `window_id` | u32  | |
| 4      | 1    | `opacity`   | u8   | 0 = fully transparent, 255 = fully opaque |

Semantics:

- Set per-window opacity for alpha blending.
- The compositor applies alpha blending: `output = (src × alpha + dst × (255 - alpha)) / 255`
- Windows with `opacity = 255` skip blending for performance.
- The effective alpha is `pixel_alpha × (opacity / 255)`.

#### `SET_FULLSCREEN` (type = 40)

Payload (4 bytes):

| Offset | Size | Field       | Type |
|--------|------|-------------|------|
| 0      | 4    | `window_id` | u32  |

Semantics:

- Make the window occupy the complete primary output at `(0, 0)`, including the
  area normally reserved for desktop shell surfaces.
- Fullscreen is independent from maximize. If a maximized window enters
  fullscreen, both state flags remain set internally and leaving fullscreen
  returns it to its maximized workarea geometry. A later `RESTORE_WINDOW`
  returns it to its original normal geometry.
- SWS currently permits one fullscreen owner on its single primary output.
  Another window's request fails with `FULLSCREEN_OCCUPIED` (error code 107).
- SWS ignores interactive/programmatic move and interactive resize operations
  while the window is fullscreen. It sends `WINDOW_CONFIGURE` with the output
  size so the client can replace its backing buffer.
- The requesting connection must own `window_id`.

#### `UNSET_FULLSCREEN` (type = 41)

Payload (4 bytes):

| Offset | Size | Field       | Type |
|--------|------|-------------|------|
| 0      | 4    | `window_id` | u32  |

Semantics:

- Leave fullscreen without changing the underlying maximize state.
- Restore the geometry that preceded fullscreen. If that state was maximized,
  SWS recomputes the current workarea geometry before configuring the client.
- The requesting connection must own `window_id`.

#### `SET_POINTER_LOCK` (type = 42)

Payload (8 bytes):

| Offset | Size | Field       | Type |
|--------|------|-------------|------|
| 0      | 4    | `window_id` | u32  |
| 4      | 4    | `locked`    | u32  |

`locked` is exactly `0` or `1`. The requesting connection must own the window.
SWS hides the cursor while lock is active and reports the authoritative state
with `POINTER_LOCK_CHANGED`.

#### `SET_CURSOR_ICON` (type = 43, protocol version 3)

Payload (8 bytes):

| Offset | Size | Field       | Type |
|--------|------|-------------|------|
| 0      | 4    | `window_id` | u32  |
| 4      | 4    | `icon`      | u32  |

The requesting connection must own the window. The selected icon remains the
window's hover cursor until replaced. Compositor-owned interactions such as
interactive move and resize temporarily override it.

Stable icon values are: `Arrow = 0`, `Pointer = 1`, `Text = 2`, `Crosshair = 3`,
`Move = 4`, `ResizeNs = 5`, `ResizeEw = 6`, `ResizeNesw = 7`,
`ResizeNwse = 8`, `Wait = 9`, `NotAllowed = 10`, `Help = 11`, and
`Progress = 12`. Unknown values are a malformed payload. Clients should confirm
the `CURSOR_ICONS` capability bit before using this request with an older server.

#### `SET_CURSOR_THEME` (type = 44, protocol version 3)

Payload (variable, maximum path length 512 bytes):

| Offset | Size | Field            | Type  |
|--------|------|------------------|-------|
| 0      | 4    | `theme_path_len` | u32   |
| 4      | N    | `theme_path`     | bytes |

The path must be non-empty UTF-8 and name one direct child of
`/share/cursors`. SWS loads and validates the theme before changing the active
cursor. It then updates only `[cursor].theme` in `/etc/sws/config.toml`, keeping
all unrelated settings intact, and activates the theme immediately. The current
pointer position and per-window cursor icon are preserved.

Success is a correlated empty `CURSOR_THEME_CHANGED` response. Invalid paths or
theme contents return `INVALID_CURSOR_THEME`; a configuration write failure
returns `CURSOR_THEME_PERSIST_FAILED`. Clients should confirm the
`CURSOR_THEMES` capability bit before using this request with an older server.

#### `GET_INPUT_ENVIRONMENT` (type = 45)

Payload: empty.

Clients should first confirm the `INPUT_ENVIRONMENT` capability bit (`1 << 4`)
from `GET_CAPABILITIES`. SWS responds with a correlated
`INPUT_ENVIRONMENT_CHANGED` snapshot. A successful request also subscribes that
connection to later changes, broadcast with the same message type and no
response flag. SWS does not send message type 35 to clients that have not made
this request, so clients from before this additive extension remain compatible.

#### `REGISTER_EXTENSION` (type = 100)

**Extension API**: This message is part of the SWS Extension API.

Payload (variable):

| Offset | Size | Field                | Type  | Notes |
|--------|------|----------------------|-------|-------|
| 0      | 4    | `extension_name_len` | u32   | Length of extension name in bytes |
| 4      | N    | `extension_name`     | bytes | UTF-8 extension identifier (e.g., "wayland_bridge") |

Semantics:

- Registers the calling client as an extension server.
- Extension servers can create windows on behalf of external clients (e.g., Wayland clients).
- Extension servers receive special input event notifications.
- The server responds with `EXTENSION_REGISTERED` containing an assigned extension ID. The response frame is marked `IS_RESPONSE` and carries the request's `request_id`.

#### `EXTENSION_CREATE_WINDOW` (type = 101)

**Extension API**: This message is part of the SWS Extension API and can only be sent by registered extensions.

Payload (12 bytes):

| Offset | Size | Field                | Type | Notes |
|--------|------|----------------------|------|-------|
| 0      | 4    | `external_client_id` | u32  | Identifier for the external client |
| 4      | 4    | `width`              | u32  | Window width in pixels |
| 8      | 4    | `height`             | u32  | Window height in pixels |

Semantics:

- Creates a window associated with an external client.
- The `external_client_id` is an opaque identifier chosen by the extension.
- Input events for this window are delivered to the extension via `EXTENSION_INPUT_EVENT`.
- The server responds with `WINDOW_CREATED` + SHM handle as usual. The `WINDOW_CREATED` frame is marked `IS_RESPONSE` and carries the request's `request_id`.

#### `EXTENSION_UPDATE_BUFFER` (type = 102)

**Extension API**: This message is part of the SWS Extension API and can only be sent by registered extensions.

Payload (24 bytes):

| Offset | Size | Field                | Type | Notes |
|--------|------|----------------------|------|-------|
| 0      | 4    | `external_client_id` | u32  | Identifier for the external client |
| 4      | 4    | `window_id`          | u32  | Window ID |
| 8      | 4    | `x`                  | i32  | Damage rectangle X |
| 12     | 4    | `y`                  | i32  | Damage rectangle Y |
| 16     | 4    | `width`              | u32  | Damage rectangle width |
| 20     | 4    | `height`             | u32  | Damage rectangle height |

Semantics:

- Updates the buffer for a window created via `EXTENSION_CREATE_WINDOW`.
- Similar to `UPDATE_BUFFER` but includes the external client ID.

#### Text Input Client API (types = 200-208)

Text input contexts let applications describe editable text state to SWS. SWS brokers the active focused context to a separately registered input method service. SWS does not implement conversion engines or dictionaries.

All text is UTF-8. Offsets are byte offsets into that UTF-8 text.

`TEXT_INPUT_CREATE` (200), payload 8 bytes:

| Offset | Size | Field       | Type |
|--------|------|-------------|------|
| 0      | 4    | `window_id` | u32  |
| 4      | 4    | `seat_id`   | u32  |

The server responds with `TEXT_INPUT_CREATED`. The response frame is marked
`IS_RESPONSE` and carries the request's `request_id`.

Fixed-size context messages:

| Message | Type | Payload |
|---------|------|---------|
| `TEXT_INPUT_DESTROY` | 201 | `context_id: u32` |
| `TEXT_INPUT_ENABLE` | 202 | `context_id: u32` |
| `TEXT_INPUT_DISABLE` | 203 | `context_id: u32` |
| `TEXT_INPUT_SET_TEXT_CHANGE_CAUSE` | 207 | `context_id: u32`, `cause: u32` |
| `TEXT_INPUT_COMMIT_STATE` | 208 | `context_id: u32`, `serial: u32` |

`TEXT_INPUT_SET_CURSOR_RECT` (204), payload 20 bytes:

| Offset | Size | Field        | Type |
|--------|------|--------------|------|
| 0      | 4    | `context_id` | u32  |
| 4      | 4    | `x`          | i32  |
| 8      | 4    | `y`          | i32  |
| 12     | 4    | `width`      | u32  |
| 16     | 4    | `height`     | u32  |

`TEXT_INPUT_SET_SURROUNDING_TEXT` (205), variable payload:

| Offset | Size | Field          | Type |
|--------|------|----------------|------|
| 0      | 4    | `context_id`   | u32  |
| 4      | 4    | `cursor_byte`  | u32  |
| 8      | 4    | `anchor_byte`  | u32  |
| 12     | 4    | `text_len`     | u32  |
| 16     | N    | `text`         | bytes |

`TEXT_INPUT_SET_CONTENT_TYPE` (206), payload 12 bytes:

| Offset | Size | Field        | Type |
|--------|------|--------------|------|
| 0      | 4    | `context_id` | u32  |
| 4      | 4    | `hint`       | u32  |
| 8      | 4    | `purpose`    | u32  |

State is double-buffered. Clients may send cursor rect, surrounding text, content type, and change cause in any order, then publish them atomically with `TEXT_INPUT_COMMIT_STATE`. The `serial` must match the current context serial.

#### Input Method Service API (types = 220-230)

An IME service registers on a normal SWS connection and receives focused text-input contexts, trigger events, and key events while it has requested keyboard arbitration. It replies with handled/pass-through decisions and text operations.

`TEXT_INPUT_ENABLE` means the application has an editable focused text-input context; it is not an IME on/off mode. Keyboard arbitration is separate. The active IME receives `IME_ACTIVATE` for the enabled focused context, receives configured trigger events such as Ctrl-Backslash via `IME_TRIGGER`, and may then request `IME_GRAB_KEYBOARD` or `IME_RELEASE_KEYBOARD`. SWS forwards ordinary key events to the IME only while the active context is grabbed.

SWS does not define language-specific input modes. An IME may report an opaque `mode_id` and a UTF-8 `mode_label` through `IME_SET_STATUS`; the id is stable only within that IME, and SWS must not interpret it. This follows the practical model used by existing IME stacks: the compositor/toolkit brokers composition, surrounding text, cursor geometry, and popup anchoring, while engine-specific modes, candidate contents, and properties remain owned by the IME.

`IME_REGISTER` (220), variable payload:

| Offset | Size | Field          | Type |
|--------|------|----------------|------|
| 0      | 4    | `capabilities` | u32  |
| 4      | 4    | `name_len`     | u32  |
| 8      | N    | `name`         | bytes |

The server responds with `IME_REGISTERED`. The response frame is marked
`IS_RESPONSE` and carries the request's `request_id`. The first registered IME
becomes active automatically; `IME_SET_ACTIVE` can switch the active IME.

Fixed-size IME messages:

| Message | Type | Payload |
|---------|------|---------|
| `IME_SET_ACTIVE` | 221 | `ime_id: u32` |
| `IME_KEY_HANDLED` | 222 | `key_serial: u32`, `handled: u32` |
| `IME_DELETE_SURROUNDING_TEXT` | 225 | `context_id: u32`, `before_bytes: u32`, `after_bytes: u32` |
| `IME_GRAB_KEYBOARD` | 226 | `context_id: u32` |
| `IME_RELEASE_KEYBOARD` | 227 | `context_id: u32` |
| `IME_SET_STATUS` | 228 | variable, described below |
| `IME_SET_POPUP_WINDOW` | 229 | `context_id`, `window_id`, `offset_x`, `offset_y`, `visible` |

`IME_SET_PREEDIT` (223), variable payload:

| Offset | Size | Field         | Type  |
|--------|------|---------------|-------|
| 0      | 4    | `context_id`  | u32   |
| 4      | 4    | `cursor_byte` | u32   |
| 8      | 4    | `anchor_byte` | u32   |
| 12     | 4    | `text_len`    | u32   |
| 16     | N    | `text`        | bytes |
| 16+N   | 4    | `spans_len`   | u32   |
| 20+N   | M    | `spans`       | bytes |

`cursor_byte`, `anchor_byte`, and span offsets are UTF-8 byte offsets into `text`. `spans` is a packed list of 12-byte records:

| Offset | Size | Field         | Type |
|--------|------|---------------|------|
| 0      | 4    | `start_byte`  | u32  |
| 4      | 4    | `end_byte`    | u32  |
| 8      | 4    | `style_flags` | u32  |

Style flags: `UNDERLINE`, `THICK_UNDERLINE`, `HIGHLIGHT`, `SELECTED`, `CONVERTED`, `TARGET_CONVERTING`, `ERROR`.

`IME_COMMIT_TEXT` (224), variable payload:

| Offset | Size | Field        | Type |
|--------|------|--------------|------|
| 0      | 4    | `context_id` | u32  |
| 4      | 4    | `text_len`   | u32  |
| 8      | N    | `text`       | bytes |

`IME_SET_POPUP_WINDOW` (229), payload 20 bytes:

| Offset | Size | Field        | Type |
|--------|------|--------------|------|
| 0      | 4    | `context_id` | u32  |
| 4      | 4    | `window_id`  | u32  |
| 8      | 4    | `offset_x`   | i32  |
| 12     | 4    | `offset_y`   | i32  |
| 16     | 4    | `visible`    | u32  |

This assigns an existing IME-owned SWS window the `input-method popup` role for
the text-input context. The IME creates and renders this window itself, usually
with window type `IME_POPUP`, then registers it here. SWS positions the popup at
the active text-input cursor rectangle plus the supplied offset, flips it above
the cursor when the lower screen edge does not have enough room, keeps it above
normal and shell windows, and hides it when the context is unavailable. SWS does
not inspect or render candidate contents.

This mirrors the modern Wayland model: applications provide text-input state and
cursor rectangles, the input method owns its UI surface, and the compositor only
anchors and stacks that surface.

`IME_SET_STATUS` (228), variable payload:

| Offset | Size | Field            | Type  |
|--------|------|------------------|-------|
| 0      | 4    | `context_id`     | u32   |
| 4      | 4    | `state`          | u32   |
| 8      | 4    | `mode_id`        | u32   |
| 12     | 4    | `flags`          | u32   |
| 16     | 4    | `mode_label_len` | u32   |
| 20     | N    | `mode_label`     | bytes |

Composition states: `DISABLED`, `DIRECT`, `COMPOSING`, `CANDIDATES`. Status flags include `MODE_ACTIVE`, `PRIVATE_MODE`, `PREDICTION_ENABLED`, and `CANDIDATES_VISIBLE`.

### Shared SGFX frame lifecycle (protocol version 2)

`REGISTER_SGFX_BUFFER` (33) and `DESTROY_SGFX_BUFFER` (35) are synchronous
requests. They use a non-zero `request_id` and receive the corresponding
registration or destruction response.

`COMMIT_SGFX_FRAME` (34) is intentionally one-way. Its header has `flags = 0`
and `request_id = 0`; SWS never sends a success acknowledgement. The payload is:

| Offset | Size | Field              | Type |
|--------|------|--------------------|------|
| 0      | 4    | `window_id`        | u32  |
| 4      | 4    | `buffer_id`        | u32  |
| 8      | 4    | `generation`       | u32  |
| 12     | 4    | `compositor_epoch` | u32  |
| 16     | 8    | `commit_serial`    | u64  |
| 24     | 4    | `damage_count`     | u32  |
| 28     | 16N  | damage rectangles  | records |

`commit_serial` is non-zero and identifies this exact use of a reusable buffer
slot. Each damage record contains `x: i32`, `y: i32`, `width: u32`, and
`height: u32` in window-local physical pixels.

A successfully enqueued commit has no response. The client must treat the
buffer as retained immediately after serializing the request and must not write
it again until one of these asynchronous events arrives:

- `SGFX_BUFFER_RELEASED` (28): payload is the 16-byte buffer identity followed
  by the matching `commit_serial: u64`. It means SWS no longer retains that
  exact use of the buffer. Releases are emitted only after a presentation
  boundary. A queued frame superseded before sampling is retired with its
  replacement at that boundary; a displayed front is retired only after its
  replacement composition/present completed successfully. This preserves
  two-slot display backpressure.
- `SGFX_FRAME_REJECTED` (27): payload is the 16-byte buffer identity,
  `commit_serial: u64`, and `error_code: u32`. A rejected buffer use was never
  retained and may be reused after the client consumes this event.

Both events have `flags = 0` and `request_id = 0`. Matching by the complete
identity and serial is required: matching only by `buffer_id` or identity has
an ABA race after a pool slot is reused. Backend-loss invalidates every token
from the older compositor epoch. A malformed commit that does not contain a
parseable identity and serial is a protocol violation rather than a routed
request failure.

### Server → Client

#### `WINDOW_CREATED` (type = 10)

When sent as the result of `CREATE_WINDOW` or `EXTENSION_CREATE_WINDOW`, this is
a response frame: `IS_RESPONSE` is set and `request_id` matches the request.

Payload (12 bytes):

| Offset | Size | Field       | Type | Notes |
|--------|------|-------------|------|-------|
| 0      | 4    | `window_id` | u32  | Window ID allocated by server |
| 4      | 8    | `shm_size`  | u64  | Size (bytes) of the window's shared-memory buffer |

`shm_size` is a fixed-width `u64` to keep the protocol stable across architectures.

After `WINDOW_CREATED`, the server sends the shared-memory handle out-of-band via handle passing (SCM_RIGHTS-style capability transfer). See `sws_protocol::send_shm_handle` / `sws_protocol::recv_shm_handle`.

#### `WINDOW_DESTROYED` (type = 11)

Payload (4 bytes): `window_id: u32`

#### `INPUT_EVENT` (type = 12)

Payload (16 bytes):

| Offset | Size | Field   | Type |
|--------|------|---------|------|
| 0      | 8    | `time`  | u64 |
| 8      | 2    | `type_` | u16 |
| 10     | 2    | `code`  | u16 |
| 12     | 4    | `value` | i32 |

Pointer-motion semantics:

- A motion sample is encoded as window-local `EV_ABS` / `ABS_X`, then
  `EV_ABS` / `ABS_Y`, followed by `EV_SYN` / `SYN_REPORT`.
- SWS tracks pointer focus independently from keyboard focus and routes normal
  motion to the topmost window under the global cursor.
- When pointer focus changes, SWS first sends one final, unclipped motion sample
  to the previous window. Its coordinates may be outside that window's bounds;
  clients use this sample to derive pointer leave and clear hover state. SWS
  then sends the in-bounds sample to the new pointer-focused window.
- Pressing the left button creates an implicit pointer grab for the target
  window. Motion and the matching release continue to that window with
  unclipped coordinates, while client-side hit testing remains responsible for
  hover and click-cancellation within the surface. After release, SWS transfers
  pointer focus to the actual window under the cursor even if the cursor does
  not move again.

The same pointer-motion and implicit-grab semantics apply when the packets are
wrapped in `EXTENSION_INPUT_EVENT`.

#### `ERROR` (type = 13)

Payload (at least 4 bytes): `code: u32`

#### `WINDOW_RESIZED` (type = 14)

When sent as the result of `RESIZE_WINDOW`, this is a response frame:
`IS_RESPONSE` is set and `request_id` matches the request. Resize/configure
notifications sent asynchronously must not set `IS_RESPONSE`.

Payload (20 bytes):

| Offset | Size | Field       | Type | Notes |
|--------|------|-------------|------|-------|
| 0      | 4    | `window_id` | u32  | |
| 4      | 8    | `shm_size`  | u64  | Size (bytes) of the new shared-memory buffer |
| 12     | 4    | `width`     | u32  | New window width |
| 16     | 4    | `height`    | u32  | New window height |

Semantics:

- Sent in response to `RESIZE_WINDOW`.
- The server provides the new buffer dimensions and size.
- After `WINDOW_RESIZED`, the server sends the new shared-memory handle out-of-band.

#### `WINDOW_CONFIGURE` (type = 15)

Payload (12 bytes):

| Offset | Size | Field       | Type |
|--------|------|-------------|------|
| 0      | 4    | `window_id` | u32  |
| 4      | 4    | `width`     | u32  |
| 8      | 4    | `height`    | u32  |

Semantics:

- Compositor requests the client to resize to the given dimensions.
- This does not include a new SHM handle; clients should respond by issuing a `RESIZE_WINDOW` request.
- Typically sent after interactive resize operations.

#### `SCREEN_SIZE` (type = 16)

Payload (8 bytes):

| Offset | Size | Field    | Type |
|--------|------|----------|------|
| 0      | 4    | `width`  | u32  |
| 4      | 4    | `height` | u32  |

Semantics:

- Sent in response to `GET_SCREEN_SIZE`.
- The frame is marked `IS_RESPONSE` and carries the request's `request_id`.
- Reports the current compositor display size in pixels.

#### `SCREEN_SIZE_CHANGED` (type = 22)

Payload (8 bytes):

| Offset | Size | Field    | Type |
|--------|------|----------|------|
| 0      | 4    | `width`  | u32  |
| 4      | 4    | `height` | u32  |

Semantics:

- Broadcast asynchronously to connected clients when the compositor display size changes.
- This is a notification, not a response to `GET_SCREEN_SIZE`.
- Clients that own fullscreen or screen-edge surfaces, such as desktop and taskbar clients, should update their local layout and may also receive `WINDOW_CONFIGURE` for affected windows.
- Clients with synchronous request/response code must tolerate this message arriving between a request and its expected response.

#### `WINDOW_STATE_CHANGED` (type = 32)

Payload (8 bytes):

| Offset | Size | Field         | Type |
|--------|------|---------------|------|
| 0      | 4    | `window_id`   | u32  |
| 4      | 4    | `state_flags` | u32  |

`state_flags` is a bitset:

| Bit | Constant | Meaning |
|-----|----------|---------|
| `0x01` | `MINIMIZED` | The window is hidden by minimization. |
| `0x02` | `MAXIMIZED` | The underlying presentation state is maximized to the workarea. |
| `0x04` | `FULLSCREEN` | The window occupies the complete output. |

Semantics:

- Sent after SWS accepts a minimize, maximize, restore, enter-fullscreen, or
  leave-fullscreen transition.
- `MAXIMIZED | FULLSCREEN` is valid and means that leaving fullscreen returns
  the window to maximized state.
- For geometry-changing transitions, SWS sends this state event before the
  corresponding `WINDOW_CONFIGURE`. Clients should treat the state and geometry
  as compositor-authoritative and resize their backing buffer in response to
  the configure event.

#### `CURSOR_THEME_CHANGED` (type = 34, protocol version 3)

Payload: empty.

This is a correlated response to `SET_CURSOR_THEME`. It is sent only after the
new theme has loaded, its selection has been persisted, and the compositor has
scheduled the replacement cursor for redraw.

#### `INPUT_ENVIRONMENT_CHANGED` (type = 35)

Payload (16 bytes):

| Offset | Size | Field              | Type |
|--------|------|--------------------|------|
| 0      | 4    | `generation`       | u32  |
| 4      | 4    | `known_flags`      | u32  |
| 8      | 4    | `state_flags`      | u32  |
| 12     | 4    | `capability_flags` | u32  |

This message is the correlated response to `GET_INPUT_ENVIRONMENT` and is also
broadcast asynchronously to subscribed connections whenever the input
environment changes. `generation` is a monotonically increasing snapshot
generation.

`known_flags` and `state_flags` use the same bits. A state bit is meaningful
only when the matching known bit is set:

| Bit | Constant | Meaning |
|-----|----------|---------|
| `0x01` | `TABLET_MODE` | Tablet-mode state is known / device is in tablet mode. |
| `0x02` | `LID_CLOSED` | Lid-closed state is known / lid is closed. |

`capability_flags` describes currently available input devices:

| Bit | Constant | Meaning |
|-----|----------|---------|
| `0x01` | `DIRECT_TOUCH` | A direct-touch device is present. |
| `0x02` | `FINE_POINTER` | A fine pointer device is present. |
| `0x04` | `KEYBOARD` | A keyboard is present. |
| `0x08` | `PEN` | A pen device is present. |

##### Runtime source and test override

SWS reads convertible posture from Scarlet event devices named
`/dev/switchN`. The device exposes Linux-style `EV_SW` frames; SWS samples
`SW_TABLET_MODE` and `SW_LID` initially and after `SYN_DROPPED`, then publishes
one coherent snapshot after each effective `SYN_REPORT` change.
Each field is aggregated independently across live switch readers: it is known
when at least one source reports it and true when any reporting source is true.
Disconnecting the last source for a field changes that field back to unknown.

The input discovery supervisor also reference-counts live device readers.
`/dev/touchscreenN` contributes `DIRECT_TOUCH`, mouse/touchpad/legacy absolute
tablet readers contribute `FINE_POINTER`, and `/dev/keyboardN` contributes
`KEYBOARD`. Disconnecting the last device in a class clears its bit and advances
the snapshot generation. `PEN` remains clear until the input ABI can distinguish
a pen reliably.

`SCARLET_TABLET_MODE` provides an initial posture for development hosts without
a switch device. The values `1`, `true`, `yes`, `on`, and `tablet` enable tablet
mode; `0`, `false`, `no`, `off`, and `laptop` disable it. Matching is
case-insensitive. Invalid or absent values leave the posture unknown. This is a
startup override only: a later hardware posture report is authoritative.

##### SBus mirror

The same state is available to non-window-system services over SBus:

| Field | Value |
|-------|-------|
| Service | `org.scarlet-os.sws` |
| Object path | `/org/scarlet/InputEnvironment` |
| Interface | `org.scarlet.InputEnvironment` |
| Method | `GetState` (no arguments) |
| Signal | `StateChanged` |

`GetState` returns four `UInt` arguments in
`generation, known_flags, state_flags, capability_flags` order.
`StateChanged` carries the same four arguments after an effective change. If
SBus is temporarily unavailable, SWS reconnects and coalesces pending signals
to the newest full snapshot; consumers must always treat each signal as a
complete state replacement.

#### `EXTENSION_REGISTERED` (type = 100)

**Extension API**: This message is part of the SWS Extension API.

Payload (4 bytes):

| Offset | Size | Field          | Type |
|--------|------|----------------|------|
| 0      | 4    | `extension_id` | u32  |

Semantics:

- Sent in response to `REGISTER_EXTENSION`.
- The frame is marked `IS_RESPONSE` and carries the request's `request_id`.
- Confirms successful extension registration.
- The `extension_id` is a unique identifier for this extension instance.

#### `EXTENSION_INPUT_EVENT` (type = 101)

**Extension API**: This message is part of the SWS Extension API.

Payload (24 bytes):

| Offset | Size | Field                | Type | Notes |
|--------|------|----------------------|------|-------|
| 0      | 4    | `external_client_id` | u32  | External client identifier |
| 4      | 4    | `window_id`          | u32  | Window ID |
| 8      | 8    | `time`               | u64  | Event timestamp |
| 16     | 2    | `type_`              | u16  | Event type |
| 18     | 2    | `code`               | u16  | Event code |
| 20     | 4    | `value`              | i32  | Event value |

Semantics:

- Forwards input events for windows created by an extension.
- Similar to `INPUT_EVENT` but includes the external client ID.
- Allows the extension to route events to the appropriate external client.

#### Text Input Client Events (types = 200-207)

`TEXT_INPUT_CREATED` (200), payload 8 bytes:

| Offset | Size | Field        | Type |
|--------|------|--------------|------|
| 0      | 4    | `context_id` | u32  |
| 4      | 4    | `serial`     | u32  |

When sent in response to `TEXT_INPUT_CREATE`, this frame is marked
`IS_RESPONSE` and carries the request's `request_id`.

Variable text events:

| Message | Type | Header Fields | Bytes |
|---------|------|---------------|-------|
| `TEXT_INPUT_PREEDIT` | 201 | `context_id`, `serial`, `cursor_byte`, `anchor_byte`, `text_len`, `spans_len` | UTF-8 preedit + span records |
| `TEXT_INPUT_COMMIT` | 202 | `context_id`, `serial`, `text_len` | UTF-8 committed text |
| `TEXT_INPUT_STATUS` | 205 | `context_id`, `serial`, `state`, `mode_id`, `flags`, `mode_label_len` | UTF-8 mode label |

Fixed-size text events:

| Message | Type | Payload |
|---------|------|---------|
| `TEXT_INPUT_DELETE_SURROUNDING_TEXT` | 203 | `context_id`, `serial`, `before_bytes`, `after_bytes` |
| `TEXT_INPUT_DONE` | 204 | `context_id`, `serial` |

Toolkits should apply preedit/commit/delete messages in order and treat `TEXT_INPUT_DONE` as the end of an update batch.

#### Input Method Service Events (types = 220-226)

`IME_REGISTERED` (220), payload 4 bytes: `ime_id: u32`. When sent in response
to `IME_REGISTER`, this frame is marked `IS_RESPONSE` and carries the request's
`request_id`.

`IME_ACTIVATE` (221) and `IME_CONTEXT_STATE` (223) share the same variable payload:

| Offset | Size | Field                | Type |
|--------|------|----------------------|------|
| 0      | 4    | `context_id`         | u32  |
| 4      | 4    | `window_id`          | u32  |
| 8      | 4    | `serial`             | u32  |
| 12     | 4    | `cursor_x`           | i32  |
| 16     | 4    | `cursor_y`           | i32  |
| 20     | 4    | `cursor_width`       | u32  |
| 24     | 4    | `cursor_height`      | u32  |
| 28     | 4    | `content_hint`       | u32  |
| 32     | 4    | `content_purpose`    | u32  |
| 36     | 4    | `text_change_cause`  | u32  |
| 40     | 4    | `cursor_byte`        | u32  |
| 44     | 4    | `anchor_byte`        | u32  |
| 48     | 4    | `surrounding_len`    | u32  |
| 52     | N    | `surrounding_text`   | bytes |

`IME_DEACTIVATE` (222) and `IME_RESET` (225), payload 8 bytes: `context_id: u32`, `serial: u32`.

`IME_TRIGGER` (226), payload 24 bytes:

| Offset | Size | Field        | Type |
|--------|------|--------------|------|
| 0      | 4    | `context_id` | u32  |
| 4      | 4    | `serial`     | u32  |
| 8      | 4    | `trigger_id` | u32  |
| 12     | 2    | `code`       | u16  |
| 14     | 2    | `reserved`   | u16  |
| 16     | 8    | `time`       | u64  |

Current trigger IDs:

| Name | Value | Meaning |
|------|-------|---------|
| `TOGGLE` | 1 | A compositor-recognized IME trigger key from `keybindings.ime_toggle`. |

`IME_KEY_EVENT` (224), payload 28 bytes:

| Offset | Size | Field        | Type |
|--------|------|--------------|------|
| 0      | 4    | `context_id` | u32  |
| 4      | 4    | `key_serial` | u32  |
| 8      | 4    | `window_id`  | u32  |
| 12     | 8    | `time`       | u64  |
| 20     | 2    | `type_`      | u16  |
| 22     | 2    | `code`       | u16  |
| 24     | 4    | `value`      | i32  |

SWS withholds a key event from the application while it is pending IME arbitration. This only happens after the active IME has sent `IME_GRAB_KEYBOARD` for the active enabled text-input context. The IME must respond with `IME_KEY_HANDLED`. If `handled != 0`, SWS drops the raw key. If `handled == 0`, SWS forwards the original raw key event to the focused application. When the IME sends `IME_RELEASE_KEYBOARD`, SWS stops routing ordinary key events to the IME and releases any still-pending raw key events back to the application.

## Extension API

The Extension API allows specialized bridge servers (like the Wayland bridge) to create and manage windows on behalf of external clients.

- The frame header has no version field. `GET_CAPABILITIES` reports the current
  protocol version; version 3 adds compositor-provided cursor icons and live
  cursor-theme selection. Input-environment support is additive and advertised
  by the `INPUT_ENVIRONMENT` capability bit without changing the version.
- The current wire format is the request-routed 8-byte header described above:
  `msg_type: u16`, `flags: u8`, `request_id: u8`, `payload_size: u32`.
- The older `msg_type: u32`, `payload_size: u32` header is not supported.
- Because SWS is still an internal protocol, incompatible wire changes may be
  made across the tree without preserving old-client compatibility. Update
  `sws_protocol`, `sws-client`, SWS, and direct protocol users together.

### Use Cases

- **Wayland Bridge**: Translates Wayland protocol to SWS, creating SWS windows for Wayland clients.
- **X11 Bridge**: (Future) Translates X11 protocol to SWS.
- **VNC Server**: (Future) Exports SWS display over network.

### Registration Flow

1. Extension connects to SWS via normal socket connection
2. Extension sends `REGISTER_EXTENSION` with its name (e.g., "wayland_bridge")
3. SWS responds with `EXTENSION_REGISTERED` containing an extension ID
4. Extension can now use `EXTENSION_CREATE_WINDOW` and receive `EXTENSION_INPUT_EVENT`

### Window Management

Extensions create windows using `EXTENSION_CREATE_WINDOW` instead of `CREATE_WINDOW`. The key difference is:

- `CREATE_WINDOW`: Window belongs to the calling client
- `EXTENSION_CREATE_WINDOW`: Window belongs to an external client (identified by `external_client_id`)

Input events for extension-created windows are delivered via `EXTENSION_INPUT_EVENT` instead of `INPUT_EVENT`, allowing the extension to route them to the correct external client.

### Security Considerations

- Extensions have elevated privileges (can create windows for any external client ID)
- In a production system, extension registration should be restricted to trusted processes
- Currently, any client can register as an extension (no authentication)

## Compatibility and Versioning
