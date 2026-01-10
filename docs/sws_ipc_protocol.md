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

## Compatibility and Versioning

- The protocol currently has no explicit version field. Changes to message layouts should be made carefully.
- Prefer adding new message types over changing existing payload formats.
- When changing payload formats is unavoidable, introduce a new message type ID and keep the old one for compatibility.
