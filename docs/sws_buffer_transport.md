# SWS Buffer Transport Plan (Post Handle-Passing)

This document describes the planned evolution of Scarlet Window Server (SWS) buffer transport.

It is intentionally forward-looking and **does not** require pixel payloads to be sent over the SWS IPC socket.

## Current State (Today)

- SWS IPC is a framed, little-endian protocol documented in `docs/sws_ipc_protocol.md`.
- `UPDATE_BUFFER` is treated as a **damage notification only** (no pixel payload).
- This keeps Create/Destroy correctness testable while deferring zero-copy rendering.

## Goal

- Achieve **zero-copy** window buffer updates by sharing memory between client and server.
- Keep the protocol **stable and portable** across architectures (no `usize` on the wire).
- Keep ownership and access control **capability-based** (no global shared-memory names).

## Dependency: Handle Passing

True zero-copy requires passing a reference/capability for a shared-memory object from one process to another.

This document assumes a forthcoming capability/handle passing mechanism (tracked in PR #286) that enables:

- Creating a shared-memory object (server-side or client-side).
- Transferring a handle/capability to the peer.
- Mapping the shared-memory into the receiver’s address space.

The exact handle encoding is **intentionally abstract** here until the handle-passing API is finalized.

## Transport Modes

SWS will support at least the following buffer transport strategies:

1. **None (damage-only)**
   - Used today.
   - `UPDATE_BUFFER` only signals damage; server decides how/if to redraw.

2. **Shared Memory (primary, post-#286)**
   - A window has an associated SHM-backed pixel buffer.
   - Client writes pixels directly into the shared buffer.
   - Client notifies SWS via `COMMIT`/`DAMAGE` messages.

3. **Socket payload (debug-only / fallback)**
   - Avoid for production due to large payload cost and fragility.
   - If ever used, it must remain bounded and optional.

## Pixel Format

To avoid implicit assumptions, pixel format is negotiated explicitly.

Minimum fields:

- `width: u32`
- `height: u32`
- `stride_bytes: u32` (or derived from width/format)
- `format: u32` (e.g. `ARGB8888`, `XRGB8888`)
- `buffer_size: u64` (authoritative total size)

## Lifecycle (Shared Memory Mode)

### 1) Create Window

Client sends:

- `CREATE_WINDOW { x, y, width, height }`

Server replies:

- `WINDOW_CREATED { window_id, ... }`

Future extension (server indicates transport strategy):

- `WINDOW_CREATED { window_id, transport, format, stride_bytes, buffer_size, ... }`

### 2) Buffer Setup

Two viable models (choose one once #286 lands and we know which direction is easier):

**A. Server-allocates buffer (recommended)**

- Server creates SHM sized for the chosen format.
- Server passes a handle to the client.
- Client maps it and writes pixels.

Pros:
- Server controls memory sizing and constraints.
- Server can revoke/replace buffers on resize.

**B. Client-allocates buffer**

- Client allocates SHM.
- Client passes handle + metadata to server.

Pros:
- Client can choose allocation strategy.

Cons:
- More validation needed server-side.

### 3) Commit / Damage

Client sends:

- `UPDATE_BUFFER { window_id }` (today: damage-only)

Future: replace or extend with:

- `DAMAGE { window_id, x, y, width, height }` (one rectangle)
- `COMMIT { window_id, serial }` (optional sequencing)

Notes:
- `serial` is used to correlate redraw completion or coalesce commits.
- If multiple rectangles are needed, either send multiple `DAMAGE` messages or add a bounded list format.

### 4) Resize

Resizing must handle buffer replacement safely.

Options:

- Server issues `BUFFER_REPLACED` with a new handle/capability and new metadata.
- Old buffer becomes invalid/outdated after an agreed transition point (e.g. after `serial`).

### 5) Destroy

Client sends:

- `DESTROY_WINDOW { window_id }`

Server releases:

- Window state.
- Any SHM objects owned by the server.

## Validation Rules (Server-side)

When receiving or creating buffer metadata:

- Reject `buffer_size` larger than a reasonable limit.
- Validate `stride_bytes >= width * bytes_per_pixel`.
- Validate `stride_bytes * height <= buffer_size`.
- Enforce alignment constraints if required.

## Compatibility / Versioning

- Keep the existing 8-byte frame header.
- Add new message types rather than changing existing payload layouts.
- If adding optional fields, prefer new message IDs (explicit versioning) over “trailing fields” unless a stable version field is introduced.

## Next Steps

1. After PR #286 lands, decide on server-alloc vs client-alloc SHM ownership.
2. Add explicit buffer transport negotiation in `WINDOW_CREATED`.
3. Add SHM handle transfer + mapping plumbing.
4. Replace damage-only `UPDATE_BUFFER` with `DAMAGE`/`COMMIT` semantics (or keep `UPDATE_BUFFER` as a shorthand).
