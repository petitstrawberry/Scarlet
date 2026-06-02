# Scarlet Window Server (SWS) IPC Protocol

This document describes the wire protocol used between the Scarlet Window Server (`sws`) and clients.

The canonical implementation is the `sws_protocol` crate located at `user/lib/sws_protocol`.

Client-side reference implementations:

- Low-level client library: `sws-client` (crate name `sws_client`) in `user/lib/sws-client`
- High-level UI toolkit: `scarlet-ui` in `user/lib/scarlet-ui`

## Transport

- Endpoint: Unix-domain socket (VFS socket)
- Default path: `/tmp/sws.sock`
- Byte order: little-endian for all integer fields

## Framing

All messages are framed.

### Header

The header is always **8 bytes**:

| Offset | Size | Field         | Type | Notes |
|--------|------|---------------|------|-------|
| 0      | 4    | `msg_type`     | u32  | Message type ID |
| 4      | 4    | `payload_size` | u32  | Payload length in bytes |

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
- For valid requests, the server allocates a new shared-memory buffer and responds with `WINDOW_RESIZED` + new SHM handle.

#### `GET_SCREEN_SIZE` (type = 10)

Payload: empty.

Semantics:

- Request the current compositor display size in pixels.
- The server responds with `SCREEN_SIZE`.

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
- The window can be restored with `RESTORE_WINDOW`.

#### `MAXIMIZE_WINDOW` (type = 18)

Payload (4 bytes):

| Offset | Size | Field       | Type |
|--------|------|-------------|------|
| 0      | 4    | `window_id` | u32  |

Semantics:

- Expand the window to fill the entire screen.
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
- Default Z-order grouping (bottom to top): Desktop → Normal → Taskbar → AlwaysOnTop.
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
- The server responds with `EXTENSION_REGISTERED` containing an assigned extension ID.

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
- The server responds with `WINDOW_CREATED` + SHM handle as usual.

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

The server responds with `TEXT_INPUT_CREATED`.

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

`TEXT_INPUT_ENABLE` means the application has an editable focused text-input context; it is not an IME on/off mode. Keyboard arbitration is separate. The active IME receives `IME_ACTIVATE` for the enabled focused context, receives trigger events such as Ctrl-Backslash via `IME_TRIGGER`, and may then request `IME_GRAB_KEYBOARD` or `IME_RELEASE_KEYBOARD`. SWS forwards ordinary key events to the IME only while the active context is grabbed.

SWS does not define language-specific input modes. An IME may report an opaque `mode_id` and a UTF-8 `mode_label` through `IME_SET_STATUS`; the id is stable only within that IME, and SWS must not interpret it. This follows the practical model used by existing IME stacks: the compositor/toolkit brokers composition, surrounding text, candidates, and display attributes, while engine-specific modes and properties remain owned by the IME.

`user/bin/src/simple_ime.rs` is a minimal external IME service for exercising this protocol. It registers as `simple-skk`, requests active IME status, and keeps an SKK state machine in the IME process: lower-case romaji commits kana directly, `Shift` + letter starts `▽` midashi input, `Shift` + letter inside midashi starts an okuri marker, `Space` enters or advances `▼` candidate selection, `Backspace` moves backward through candidates while `▼` conversion is active, `Enter` commits, and `Esc` cancels or returns to midashi input. It loads a UTF-8 SKK dictionary from `/system/scarlet/share/skk/SKK-JISYO.L`, `/usr/share/skk/SKK-JISYO.L`, `/usr/local/share/skk/SKK-JISYO.L`, or `/etc/skk/SKK-JISYO.L`, falling back to a tiny built-in dictionary only when no usable file exists. `tools/fetch_skk_dictionary.sh` downloads the upstream SKK dictionary and converts it from EUC-JP to UTF-8 for the Scarlet rootfs. This sample is not part of SWS compositor behavior; SWS remains only a broker for text-input state, trigger delivery, key arbitration, preedit, commit, deletion, and candidate messages.

`IME_REGISTER` (220), variable payload:

| Offset | Size | Field          | Type |
|--------|------|----------------|------|
| 0      | 4    | `capabilities` | u32  |
| 4      | 4    | `name_len`     | u32  |
| 8      | N    | `name`         | bytes |

The server responds with `IME_REGISTERED`. The first registered IME becomes active automatically; `IME_SET_ACTIVE` can switch the active IME.

Fixed-size IME messages:

| Message | Type | Payload |
|---------|------|---------|
| `IME_SET_ACTIVE` | 221 | `ime_id: u32` |
| `IME_KEY_HANDLED` | 222 | `key_serial: u32`, `handled: u32` |
| `IME_DELETE_SURROUNDING_TEXT` | 225 | `context_id: u32`, `before_bytes: u32`, `after_bytes: u32` |
| `IME_HIDE_CANDIDATES` | 227 | `context_id: u32` |
| `IME_GRAB_KEYBOARD` | 228 | `context_id: u32` |
| `IME_RELEASE_KEYBOARD` | 229 | `context_id: u32` |

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

`IME_SET_CANDIDATES` (226), variable payload:

| Offset | Size | Field            | Type  |
|--------|------|------------------|-------|
| 0      | 4    | `context_id`     | u32   |
| 4      | 4    | `selected_index` | u32   |
| 8      | 4    | `page_start`     | u32   |
| 12     | 4    | `page_size`      | u32   |
| 16     | 4    | `anchor_byte`    | u32   |
| 20     | 4    | `candidates_len` | u32   |
| 24     | N    | `candidates`     | bytes |

`candidates` is a structured blob:

| Offset | Size | Field             | Type |
|--------|------|-------------------|------|
| 0      | 4    | `candidate_count` | u32  |
| 4      | ...  | candidate entries |      |

Each candidate entry is:

| Field | Encoding |
|-------|----------|
| `id` | `u32` |
| `label` | `u32 byte_len` + UTF-8 bytes |
| `text` | `u32 byte_len` + UTF-8 bytes |
| `annotation` | `u32 byte_len` + UTF-8 bytes |
| `comment` | `u32 byte_len` + UTF-8 bytes |
| `flags` | `u32` |

IME services may also display their own candidate window; this message exists for clients or toolkits that want to render candidates inline.

`IME_SET_STATUS` (230), variable payload:

| Offset | Size | Field            | Type  |
|--------|------|------------------|-------|
| 0      | 4    | `context_id`     | u32   |
| 4      | 4    | `state`          | u32   |
| 8      | 4    | `mode_id`        | u32   |
| 12     | 4    | `flags`          | u32   |
| 16     | 4    | `mode_label_len` | u32   |
| 20     | N    | `mode_label`     | bytes |

Composition states: `DISABLED`, `DIRECT`, `COMPOSING`, `CANDIDATES`. Status flags include `MODE_ACTIVE`, `PRIVATE_MODE`, `PREDICTION_ENABLED`, and `CANDIDATES_VISIBLE`.

### Server → Client

#### `WINDOW_CREATED` (type = 10)

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

#### `ERROR` (type = 13)

Payload (at least 4 bytes): `code: u32`

#### `WINDOW_RESIZED` (type = 14)

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
- Clients that own full-screen or screen-edge surfaces, such as desktop and taskbar clients, should update their local layout and may also receive `WINDOW_CONFIGURE` for affected windows.
- Clients with synchronous request/response code must tolerate this message arriving between a request and its expected response.

#### `EXTENSION_REGISTERED` (type = 100)

**Extension API**: This message is part of the SWS Extension API.

Payload (4 bytes):

| Offset | Size | Field          | Type |
|--------|------|----------------|------|
| 0      | 4    | `extension_id` | u32  |

Semantics:

- Sent in response to `REGISTER_EXTENSION`.
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

Variable text events:

| Message | Type | Header Fields | Bytes |
|---------|------|---------------|-------|
| `TEXT_INPUT_PREEDIT` | 201 | `context_id`, `serial`, `cursor_byte`, `anchor_byte`, `text_len`, `spans_len` | UTF-8 preedit + span records |
| `TEXT_INPUT_COMMIT` | 202 | `context_id`, `serial`, `text_len` | UTF-8 committed text |
| `TEXT_INPUT_CANDIDATES` | 205 | `context_id`, `serial`, `selected_index`, `page_start`, `page_size`, `anchor_byte`, `candidates_len` | structured candidate blob |
| `TEXT_INPUT_STATUS` | 207 | `context_id`, `serial`, `state`, `mode_id`, `flags`, `mode_label_len` | UTF-8 mode label |

Fixed-size text events:

| Message | Type | Payload |
|---------|------|---------|
| `TEXT_INPUT_DELETE_SURROUNDING_TEXT` | 203 | `context_id`, `serial`, `before_bytes`, `after_bytes` |
| `TEXT_INPUT_DONE` | 204 | `context_id`, `serial` |
| `TEXT_INPUT_HIDE_CANDIDATES` | 206 | `context_id`, `serial` |

Toolkits should apply preedit/commit/delete messages in order and treat `TEXT_INPUT_DONE` as the end of an update batch.

#### Input Method Service Events (types = 220-226)

`IME_REGISTERED` (220), payload 4 bytes: `ime_id: u32`.

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
| `TOGGLE` | 1 | A compositor-recognized IME trigger key, currently Ctrl-Backslash. |

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

- The protocol currently has no explicit version field. Changes to message layouts should be made carefully.
- Prefer adding new message types over changing existing payload formats.
- When changing payload formats is unavoidable, introduce a new message type ID and keep the old one for compatibility.

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
