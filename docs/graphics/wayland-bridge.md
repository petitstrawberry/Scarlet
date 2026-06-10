# Wayland Bridge

## Overview

The Wayland Bridge is a userspace compatibility layer that translates Wayland protocol messages into SWS (Scarlet Window System) IPC calls, allowing Wayland client applications to run on Scarlet OS.

## Architecture

```text
Wayland Client ←→ UNIX Socket ←→ Wayland Bridge ←→ SWS Socket ←→ SWS Server
                  (/tmp/wayland-0)                   (/tmp/sws.sock)
```

The bridge acts as a Wayland compositor from the client's perspective while forwarding all window operations to the native SWS compositor.

## Module Structure

```
user/bin/src/wayland_bridge/
├── main.rs       – Bridge server, socket handling, message routing
├── protocol.rs   – Wayland wire protocol parser
├── registry.rs   – wl_registry, global interface advertisement
├── surface.rs    – Surface state tracking and SWS window mapping
├── xdg_shell.rs  – XDG Shell protocol (toplevel windows)
├── shm.rs        – Shared memory buffer management
├── input.rs      – Input event translation (keyboard, pointer)
└── region.rs     – Damage region tracking
```

## Supported Wayland Protocols

### Core Protocol

| Interface | Status | Notes |
|-----------|--------|-------|
| `wl_display` | Implemented | Connection management, sync/error |
| `wl_registry` | Implemented | Global object discovery |
| `wl_compositor` | Implemented | Surface creation |
| `wl_surface` | Implemented | Attach, damage, commit, frame callbacks |
| `wl_shm` | Implemented | Shared memory pools and buffers |
| `wl_shm_pool` | Implemented | Memory pool management |
| `wl_buffer` | Implemented | Buffer attach/detach |
| `wl_callback` | Implemented | Frame synchronization |
| `wl_seat` | Implemented | Input device grouping |
| `wl_pointer` | Implemented | Motion, button, axis events |
| `wl_keyboard` | Implemented | Key events, keymap |

### XDG Shell

| Interface | Status | Notes |
|-----------|--------|-------|
| `xdg_wm_base` | Implemented | Window manager base |
| `xdg_surface` | Implemented | Surface wrapper |
| `xdg_toplevel` | Implemented | Application windows (configure, close) |

## Buffer Flow

1. Wayland client creates `wl_shm_pool` → bridge receives shared memory handle via `Socket::recv_handle()`.
2. Client creates `wl_buffer` from pool → bridge tracks buffer dimensions and format.
3. Client attaches buffer to `wl_surface` and commits.
4. Bridge translates commit to SWS `COMMIT` with damage regions.
5. SWS composites the buffer directly from shared memory.

## Input Translation

SWS input events are translated to Wayland input protocol:

| SWS Event | Wayland Event |
|-----------|---------------|
| Key press/release | `wl_keyboard.key` |
| Mouse motion | `wl_pointer.motion` |
| Mouse button | `wl_pointer.button` |
| Mouse axis (scroll) | `wl_pointer.axis` |

## Handle Transfer

The bridge uses Scarlet's native handle transfer for passing file descriptors:

- Linux compatibility layer converts SCM_RIGHTS to kernel handles transparently.
- Bridge receives handles with `Socket::recv_handle()` and forwards to SWS with `Socket::send_handle()`.
- No global shared-memory names — capability-based access control.

## Running the Bridge

```bash
# Start the bridge server
/bin/wayland_bridge

# Connect Wayland clients
export WAYLAND_DISPLAY=wayland-0
./my_wayland_app
```

## Dependencies

- `scarlet_std` — Scarlet OS standard library
- `sws_protocol` — SWS wire protocol definitions
- `sws-client` — SWS client library
