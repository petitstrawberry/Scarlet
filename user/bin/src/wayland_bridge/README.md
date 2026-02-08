# Wayland Bridge for Scarlet Window System

This is a Wayland protocol bridge server that allows Wayland client applications to run on Scarlet OS by translating Wayland protocol messages to SWS (Scarlet Window System) IPC calls.

## Architecture

```
Wayland Client <-> UNIX Socket <-> Wayland Bridge <-> SWS Socket <-> SWS Server
                  (/tmp/wayland-0)                   (/tmp/sws.sock)
```

The bridge acts as a Wayland compositor from the client's perspective, but translates all operations to the native SWS protocol.

## Supported Protocols

### Core Wayland Protocols
- `wl_display` - Display connection management
- `wl_registry` - Global object registry
- `wl_compositor` - Surface composition
- `wl_surface` - Surface management
- `wl_shm` - Shared memory buffers
- `wl_shm_pool` - Shared memory pool management
- `wl_buffer` - Buffer management

### XDG Shell Protocol
- `xdg_wm_base` - Base window management
- `xdg_surface` - XDG surface wrapper
- `xdg_toplevel` - Toplevel (application) windows

## Implementation Status

### Phase 1: Foundation ✓
- [x] Wayland wire protocol parser
- [x] Socket server listening on /tmp/wayland-0
- [x] Object ID management
- [x] Registry with global interface advertisement

### Phase 2: Core Protocol ✓
- [x] wl_display and wl_registry
- [x] wl_compositor and wl_surface
- [x] wl_shm buffer support (structure)
- [x] xdg_shell (xdg_wm_base, xdg_surface, xdg_toplevel)

### Phase 3: SWS Integration (In Progress)
- [ ] Map Wayland surfaces to SWS windows
- [ ] Forward buffer commits to SWS
- [ ] Translate input events from SWS to Wayland
- [ ] SCM_RIGHTS file descriptor passing (requires kernel support)

### Phase 4: Advanced Features
- [ ] wl_seat (input device management)
- [ ] wl_output (display information)
- [ ] wl_callback (frame synchronization)
- [ ] xdg_popup (popup windows)

## Usage

### Starting the Bridge

```bash
# Run the Wayland bridge server
/bin/wayland_bridge
```

The bridge will listen on `/tmp/wayland-0` for Wayland client connections.

### Connecting Wayland Clients

Set the `WAYLAND_DISPLAY` environment variable to connect to the bridge:

```bash
export WAYLAND_DISPLAY=wayland-0
./my_wayland_app
```

## Design Notes

### Wire Protocol

The Wayland protocol uses a binary wire format:
- All integers are in native endianness
- Message header: 8 bytes (object_id: u32, size_and_opcode: u32)
- Arguments follow based on message signature

### Object Management

- Object ID 1 is always reserved for wl_display
- The bridge allocates object IDs sequentially starting from 2
- Objects are tracked in a BTreeMap mapping ID -> interface name

### Shared Memory

Wayland clients use shared memory (wl_shm) for efficient pixel buffer transfer:
1. Client creates a shared memory pool with `wl_shm.create_pool`
2. Client creates buffers from the pool with `wl_shm_pool.create_buffer`
3. Client attaches buffers to surfaces with `wl_surface.attach`
4. Client commits changes with `wl_surface.commit`

The bridge needs to map these shared memory buffers to SWS shared memory handles.

### XDG Shell

XDG Shell is the standard protocol for desktop window management:
- `xdg_surface` wraps a `wl_surface` and adds window management
- `xdg_toplevel` represents application windows with title bar, close button, etc.
- The bridge translates xdg_toplevel operations to SWS window operations

## Dependencies

- `scarlet_std` - Scarlet OS standard library
- `sws_protocol` - SWS protocol definitions
- `sws-client` - SWS client library (for connecting to SWS server)

## Handle Transfer

The bridge uses Scarlet's native handle transfer mechanism (`Socket::recv_handle()` and `Socket::send_handle()`) for passing file descriptors. The Linux compatibility layer automatically converts SCM_RIGHTS file descriptor passing to handle transfer, so Wayland clients can use standard file descriptor passing and it will work transparently.

### Shared Memory

Wayland clients pass shared memory file descriptors using SCM_RIGHTS:
1. Client creates a shared memory pool with `wl_shm.create_pool` and passes an FD
2. Linux compatibility layer converts the FD to a kernel handle
3. Bridge receives the handle using `Socket::recv_handle()`
4. Bridge forwards the handle to SWS using `Socket::send_handle()`
5. SWS maps the shared memory for compositing

## Future Work

### Input Event Translation

Input events need to be:
1. Received from SWS server
2. Translated to Wayland input protocol (wl_seat, wl_pointer, wl_keyboard)
3. Sent to the focused Wayland client

### Multi-Client Support

Currently, the bridge handles one client at a time. Multi-client support requires:
- Thread pool or async I/O for concurrent clients
- Per-client state management
- Focus management across clients

## Testing

A minimal Wayland client can be created to test the bridge:

```c
// Minimal Wayland client pseudocode
display = wl_display_connect("wayland-0");
registry = wl_display_get_registry(display);
// Bind to wl_compositor, wl_shm, xdg_wm_base
compositor = wl_registry_bind(registry, "wl_compositor");
surface = wl_compositor_create_surface(compositor);
// Create buffer and attach to surface
wl_surface_commit(surface);
```

## References

- [Wayland Protocol Specification](https://wayland.freedesktop.org/docs/html/)
- [XDG Shell Protocol](https://gitlab.freedesktop.org/wayland/wayland-protocols/-/blob/main/stable/xdg-shell/xdg-shell.xml)
- [Wayland Wire Format](https://wayland.freedesktop.org/docs/html/ch04.html)
