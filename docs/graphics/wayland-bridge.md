# Wayland Bridge

## Overview

`wayland-bridge` is a userspace compatibility compositor. A Linux Wayland
client sees an ordinary Wayland socket, while the bridge translates surface,
window-management, input, and presentation operations to Scarlet Window
System (SWS) IPC.

```text
Wayland client <-> /tmp/wayland-0 <-> wayland-bridge <-> /tmp/sws.sock <-> SWS
```

The bridge and SWS intentionally remain separate processes. Protocol
translation, Linux compatibility, and untrusted client parsing stay in the
bridge; workspace policy, input ownership, composition, and display scheduling
stay in SWS.

Each accepted Wayland connection owns independent protocol state and an SWS
extension connection. IDs and failures therefore cannot cross Wayland clients.

## Module Structure

```text
user/bin/src/wayland_bridge/
├── main.rs       - sockets, dispatch, SWS translation, presentation lifecycle
├── protocol.rs   - Wayland wire framing
├── registry.rs   - globals and object binding
├── surface.rs    - pending/committed wl_surface state
├── xdg_shell.rs  - xdg_surface and xdg_toplevel state
├── shm.rs        - wl_shm pool and buffer objects
├── input.rs      - keyboard and pointer translation
└── region.rs     - region objects
```

## Implemented Surface Model

The bridge follows Wayland's double-buffered surface model:

1. `wl_surface.attach`, damage, scale, and transform requests update pending
   state only.
2. `wl_surface.commit` atomically selects the pending buffer and publishes the
   accumulated damage.
3. A buffer selection is never forwarded early from `attach`.
4. `wl_callback.done` is paced by SWS `FRAME_DONE` for mapped application
   surfaces. Cursor and role-less surfaces, which have no SWS scene, complete
   locally.
5. Buffers committed to cursor or role-less surfaces are released immediately
   after the bridge consumes the commit because SWS never samples them.
   Buffers committed to mapped application surfaces are released only after
   SWS reports `EXTENSION_BUFFER_RELEASED`.

This matters for clients that prepare several requests before committing and
for toolkits that rotate through two or three buffers.

The bridge keeps at most one presentation request in flight per mapped
surface. Further commits received before that presentation completes are
coalesced: the newest buffer selection wins, damage is unioned into a bounded
list, and callbacks remain associated with the next presentation. A buffer
superseded before SWS can sample it is released locally. If the client destroys
a buffer object while its commit is coalesced, that commit is first published
so SWS observes the protocol's commit-before-destroy ordering.

## Reusable Buffer Lifecycle

SWS protocol version 9 exposes `EXTENSION_BUFFER_OBJECTS`. The bridge uses it
instead of transferring and remapping a handle on every buffer switch.

```text
wl_shm.create_pool
  -> register SHM capability once in SWS

wl_shm_pool.create_buffer
  -> define a reusable view (pool, offset, extent, stride, format)

wl_surface.commit
  -> commit effective buffer resource ID + serial + bounded damage list

SWS stops sampling the old resource
  -> EXTENSION_BUFFER_RELEASED
  -> wl_buffer.release
```

Wayland object IDs are not used as durable SWS resource IDs. The bridge assigns
monotonic, connection-scoped resource IDs so a Wayland ID can be reused while
SWS is still retiring an older object. Commit serials identify retained uses
and avoid an ABA ambiguity around delayed release.

SWS maps each SHM pool once and windows borrow validated buffer views from that
mapping. The production path uploads only committed damage into a private GPU
texture. Experimental direct import is retained behind
`SWS_EXTENSION_SHM_DIRECT_IMPORT=1`: when the selected backend supports imported
linear images, SWS pins each defined buffer view once and performs only the
cache/visibility transfer for declared damage before sampling it. Direct import
is opt-in because backend support and teardown semantics are not yet reliable
enough to make a capability probe part of ordinary GTK startup.

Pool resize remaps the CPU view once while already-defined GPU views retain
their pinned ranges. Buffer and pool destruction is deferred until no window
selects the resource. Closing a window or connection also retires its selected
buffer and imported image before the pool mapping is dropped.

The old `EXTENSION_ATTACH_BUFFER`/`EXTENSION_UPDATE_BUFFER` path remains in SWS
for compatibility, but `wayland-bridge` no longer uses it.

## GPU-Backed Wayland Plan

The protocol boundary separates a logical external buffer from its backing:

- Today, registration defines a single-plane SHM view. SWS can import that
  CPU-rendered view into SGFX, but it is still a `wl_shm` buffer rather than a
  client-rendered GPU image.
- A future GPU path adds a registration/import message for an SGFX image or a
  dma-buf-style plane set, including format/modifier metadata and explicit
  acquire synchronization.
- `EXTENSION_COMMIT_BUFFER`, commit serials, damage, frame pacing, deferred
  destruction, and `EXTENSION_BUFFER_RELEASED` remain unchanged.
- SWS owns import, composition, and release timing. The bridge translates
  Wayland objects and synchronization but does not proxy Vulkan commands or
  become part of the compositor.

Release of a GPU-backed object must occur only after every sampling operation
and display dependency has completed. Backend or compositor-epoch loss must
reject/retire outstanding uses explicitly, following the existing shared SGFX
buffer state machine rather than silently reusing stale imports.

This permits Linux clients to gain zero-copy GPU presentation later without
coupling the current SHM implementation to a particular graphics API.

## Supported Protocols

The currently useful path includes:

- `wl_display`, `wl_registry`, `wl_compositor`, `wl_surface`
- `wl_shm`, `wl_shm_pool`, `wl_buffer`, `wl_callback`
- `wl_seat`, `wl_pointer`, `wl_keyboard`
- `wl_output`
- `xdg_wm_base`, `xdg_surface`, and `xdg_toplevel`

`xdg_toplevel` maximize and fullscreen map to the corresponding independent
SWS states. Configure dimensions are converted from SWS physical pixels to
Wayland logical units with the surface buffer scale.

Important current gaps include `xdg_popup`, complete data-device behavior,
touch, relative-pointer/pointer-constraints protocols, presentation feedback,
and client-rendered GPU buffer import. Applications may also fail independently
of the bridge when the Linux rootfs lacks desktop runtime data such as D-Bus
machine IDs, settings portals, MIME data, icon loaders, or GTK theme assets.

## Damage and Performance Rules

- Empty damage remains empty for state-only commits. Switching Wayland buffers
  does not turn it into a full-window upload: the copied path caches logical
  surface contents per SWS window, while each directly imported buffer is
  initialized when its image is registered. Explicit client damage is the only
  ordinary hot-path upload range.
- A buffer-selection or frame-callback-only commit with no explicit damage uses
  a one-pixel presentation trigger. This advances presentation and release
  lifecycles without promoting pointer hover into a full-window upload.
- SWS keeps the last attached buffer extent independent from managed window
  geometry. While a maximize, fullscreen, or tablet-layout configure is still
  awaiting a replacement buffer, the GPU compositor scales the retained
  contents to the requested presentation geometry instead of rejecting the
  smaller SHM range or drawing a placeholder.
- Inactive or otherwise suspended SWS scenes retain their newest surface state
  without continuously damaging the output.
- Handle transfer occurs at pool registration, not in the pointer-hover or
  frame hot path.
- There is no fixed 16 ms sleep in `wl_surface.commit`; SWS presentation
  callbacks provide pacing. Commits that arrive during an outstanding frame
  are folded into the next frame rather than each producing SWS work.
- The bridge blocks in `poll` on the Wayland client and its SWS connection.
  It does not use the former 1--8 ms exponential idle sleep, so an SWS frame
  callback normally reaches the client after at most the kernel's current
  multi-handle poll recheck interval (1 ms). Until the kernel supports native
  registration on an entire wait set, multi-handle polling registers the first
  descriptor as its wake anchor and uses that interval to rescan the remainder.
  The Wayland client socket is deliberately first, so a request following an
  initialization round trip wakes the bridge immediately rather than depending
  solely on a timer tick.
- SWS associates a late `REQUEST_FRAME` with the window's most recent buffer
  submission. If that submission was presented between the commit and request
  messages, the callback completes instead of waiting for an unrelated redraw.

If hover remains expensive, first inspect the damage rectangles emitted by the
client. A toolkit that deliberately damages its complete surface still incurs
its own CPU rasterization and a full-range cache transfer, but ordinary GTK
buffer rotation does not remap, recopy, or recreate the texture by itself.

## Handle Transfer

Linux SCM_RIGHTS descriptors are converted to Scarlet kernel handles by the
compatibility layer. The bridge consumes ancillary handles in Wayland request
order and accepts one only for `wl_shm.create_pool`. It then sends that
capability in the correlated SWS pool-registration request. No global SHM name
or path is used.

## Running

```bash
/bin/wayland-bridge
export WAYLAND_DISPLAY=wayland-0
./my_wayland_app
```

Useful bridge logging is controlled by `WAYLAND_BRIDGE_LOG`; use `debug` only
for diagnosis because per-input logging itself is expensive.

`wayland-bridge` is a `stemd` service with an explicit `logd` dependency.
Lifecycle, client-worker failures, and unsupported protocol requests are
emitted at the default `info`/`warning` levels and can be followed with:

```bash
logctl -u wayland-bridge -f
```

Each accepted Wayland connection has a stable `client=N` identifier in these
records. A client protocol failure terminates and cleans up only that worker;
the server continues accepting other clients. Loss of that worker's SWS
connection is not ignored and is recorded as a terminal worker error.

Correlated SWS requests have a five-second deadline. The initial socket,
extension-registration, and output-scale handshake may be retried once because
no Wayland or SWS resource exists at that point. Pool, window, and later
resource operations are never replayed after partial state was published.
Default logs mark the handshake, registry binding, surface/seat creation,
sampled SHM resource and commit counts, and SWS-window creation boundaries so
a stuck GTK roundtrip can be located without enabling hot-path debug logging.
Large resource counts are sampled after the first four entries to avoid turning
an allocation loop into a second performance problem. Disconnect records
include totals for surfaces, pools, buffers, commits, and locally released
buffers.

By default SWS reports that extension SHM uses copied GPU uploads. With
`SWS_EXTENSION_SHM_DIRECT_IMPORT=1`, it separately records whether each buffer
was directly imported or fell back to copied upload. Direct import requires the
selected kernel GPU backend to implement imported-image transfer/cache
synchronization; opening a generic GPU context alone is not treated as proof
that the path works.
