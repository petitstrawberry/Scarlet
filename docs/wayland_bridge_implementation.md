# Wayland Bridge Implementation Summary

This document summarizes the implementation of the Wayland Bridge for Scarlet Window System (SWS).

## Overview

The Wayland Bridge is a compatibility layer that allows Wayland client applications to run on Scarlet OS by translating Wayland protocol messages to SWS IPC calls. This implementation follows the requirements specified in issue #272.

## What Has Been Implemented

### 1. Wayland Bridge Server Structure ✓

**Location**: `user/bin/src/wayland_bridge/`

- **main.rs**: Bridge server with socket handling and message routing
- **protocol.rs**: Wayland wire protocol implementation
- **registry.rs**: Global interface registry management
- **surface.rs**: Surface state tracking
- **xdg_shell.rs**: XDG Shell protocol support
- **shm.rs**: Shared memory buffer management
- **README.md**: Comprehensive documentation

### 2. Wayland Protocol Support ✓

The bridge implements the following Wayland protocols:

#### Core Protocol
- `wl_display` - Display connection and synchronization
- `wl_registry` - Global object discovery
- `wl_compositor` - Surface composition interface
- `wl_surface` - Individual surface management
  - attach, damage, commit operations
- `wl_callback` - Frame synchronization callbacks
 - `wl_seat` - Input device grouping (advertised version 5)
 - `wl_pointer` - Motion/button events, with frame events
 - `wl_keyboard` - Key events + keymap

#### Shared Memory Protocol
- `wl_shm` - Shared memory management
- `wl_shm_pool` - Memory pool for buffers
- `wl_buffer` - Individual buffer management
  - Structure for ARGB8888 and XRGB8888 formats

#### XDG Shell Protocol
- `xdg_wm_base` - Base window management
- `xdg_surface` - Surface with window management
- `xdg_toplevel` - Application windows
  - set_title, set_app_id
  - move, resize operations
  - min/max size constraints
  - minimize, maximize, restore
  - destroy (mapped to SWS window close)

### 3. SWS Extension API ✓

**Location**: `user/lib/sws_protocol/src/lib.rs`

Extension protocol messages:

#### Client → Server
- `REGISTER_EXTENSION (100)` - Register as extension server
- `EXTENSION_CREATE_WINDOW (101)` - Create window for external client
- `EXTENSION_UPDATE_BUFFER (102)` - Update buffer for external client
- `EXTENSION_ATTACH_BUFFER (103)` - Attach SHM buffer for external client
- `REQUEST_MOVE_WINDOW (5)` - Begin compositor-side move
- `MINIMIZE_WINDOW (17)` - Minimize window
- `MAXIMIZE_WINDOW (18)` - Maximize window
- `RESTORE_WINDOW (19)` - Restore window
- `DESTROY_WINDOW (2)` - Close window

#### Server → Client
- `EXTENSION_REGISTERED (100)` - Extension registration confirmation
- `EXTENSION_INPUT_EVENT (101)` - Input event routing to extensions

**Documentation**: `docs/sws_ipc_protocol.md` updated with Extension API details

### 4. Bridge ↔ SWS Integration ✓

- Socket server listening on `/tmp/wayland-0`
- Object ID allocation and tracking
- Message header parsing and encoding
- Argument encoding for various types (int, uint, string, object, array)
- SWS extension registration + window creation
- Surface → window mapping for update/resize/move/close
- SHM handle forwarding via `EXTENSION_ATTACH_BUFFER`
- Input event translation from SWS → Wayland

## What Is Still Missing / Partial

### 1. Additional Wayland Protocols (Missing)

- `wl_data_device_manager` - Clipboard and drag-and-drop
- `xdg_popup` - Popup windows (context menus, tooltips)
- `wl_subsurface` - Sub-surfaces for complex UIs
- `xdg_decoration` - Server-side decorations / SSD hints
- `wl_output` - Real output info (currently minimal)

### 2. Input Coverage (Partial)

- `wl_pointer.axis` (scroll) is not implemented
- Touch input is not implemented
- Relative pointer protocol not implemented

### 3. Window Management Details (Partial)

- `xdg_toplevel.resize` is not mapped to compositor resize
- `xdg_toplevel.set_fullscreen` / `unset_fullscreen` are not mapped
- `xdg_surface.set_window_geometry` is accepted but not used for hit-testing

### 4. Robustness / Edge Cases (Partial)

- Multi-seat or multiple pointers not supported
- Input focus is single-surface and simplistic
- Buffer lifecycle and release paths are minimal

### 5. Testing and Validation (Missing)

Required tests:
- Minimal Wayland client application
  - Create display connection
  - Get registry and bind interfaces
  - Create surface with SHM buffer
  - Attach buffer and commit
  - Handle input events
- Integration tests:
  - Wayland client → Bridge → SWS flow
  - Window creation and display
  - Input event routing
  - Buffer updates and damage

## Architecture Diagram

```
┌─────────────────┐
│ Wayland Client  │
│  (wl_display)   │
└────────┬────────┘
         │ Wayland Protocol
         │ /tmp/wayland-0
         ▼
┌─────────────────────────┐
│   Wayland Bridge        │
│  - Protocol Parser      │
│  - Registry Manager     │
│  - Surface Manager      │
│  - XDG Shell Manager    │
│  - SHM Manager          │
└────────┬────────────────┘
         │ SWS Extension API
         │ /tmp/sws.sock
         ▼
┌─────────────────────────┐
│  SWS Compositor         │
│  - Window Manager       │
│  - Input Manager        │
│  - Framebuffer          │
│  - Extension Handler    │
└─────────────────────────┘
```

## Known Limitations

- Wayland input does not include scroll or touch events.
- `xdg_decoration` is missing; titlebar behavior depends on toolkit defaults.
- `wl_output` data is minimal; no scale/transform handling.
- Buffer updates are damage-based but do not enforce throttling or frame pacing.
- Close/minimize/maximize rely on SWS behavior, not Wayland-side configure state.

## Files Modified/Created

### Created
- `user/bin/src/wayland_bridge/main.rs`
- `user/bin/src/wayland_bridge/protocol.rs`
- `user/bin/src/wayland_bridge/registry.rs`
- `user/bin/src/wayland_bridge/surface.rs`
- `user/bin/src/wayland_bridge/xdg_shell.rs`
- `user/bin/src/wayland_bridge/shm.rs`
- `user/bin/src/wayland_bridge/README.md`

### Modified
- `user/bin/Cargo.toml` - Added wayland_bridge binary
- `user/lib/sws_protocol/src/lib.rs` - Added extension API messages
- `docs/sws_ipc_protocol.md` - Documented extension API

## Security Considerations

- Extensions have elevated privileges (can create windows for any external client)
- No authentication currently implemented for extension registration
- Future: Restrict extension registration to trusted processes
- Future: Validate external_client_id to prevent spoofing

## Next Tasks

1. Add `wl_pointer.axis` for scroll input and translate SWS scroll events.
2. Implement `xdg_toplevel.resize` and fullscreen mapping to SWS.
3. Implement `xdg_decoration` or SSD policy to improve titlebar behavior.
4. Add `wl_data_device_manager` (clipboard) and `xdg_popup`.
5. Improve focus handling and multi-seat support.
6. Add integration tests with a simple Wayland client.

## Conclusion

The bridge is functional for basic Wayland apps: surfaces, shm buffers, input, and xdg_toplevel move/min/max/close are wired through SWS. The main remaining work is protocol breadth (popup/clipboard/scroll/fullscreen/resize) and tighter window-management integration.

The architecture is extensible and can support additional Wayland protocols as needed in the future.
