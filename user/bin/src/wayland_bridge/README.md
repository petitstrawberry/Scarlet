# Wayland Bridge

This directory contains Scarlet's Wayland-to-SWS compatibility compositor.
The canonical architecture, supported protocol surface, reusable-buffer
lifecycle, performance rules, and future GPU import design are documented in
[`docs/graphics/wayland-bridge.md`](../../../../docs/graphics/wayland-bridge.md).

## Run

```bash
/bin/wayland-bridge
export WAYLAND_DISPLAY=wayland-0
weston-simple-shm
```

When started by `stemd`, normal bridge output is captured by `logd`:

```bash
logctl -u wayland-bridge -f
```

SHM pools are registered with SWS once. `wl_surface.attach` remains pending
until `wl_surface.commit`, which sends a reusable SWS buffer ID plus bounded
damage. SWS uses damage-bounded copied uploads by default. Experimental direct
SHM import remains available with `SWS_EXTENSION_SHM_DIRECT_IMPORT=1`, but must
not be enabled by default until backend startup, transfer, and teardown are all
reliable. Do not reintroduce per-attach handle forwarding or a fixed commit
sleep; both are hot-path regressions for toolkits that rotate buffers.

Only application surfaces mapped into the SWS scene wait for
`EXTENSION_BUFFER_RELEASED`. Cursor and role-less surfaces are not sampled by
SWS, so their attached buffers receive `wl_buffer.release` immediately after
commit. Retaining those buffers makes GTK allocate replacement pools before it
creates a toplevel window.

The client worker waits on both the Wayland and SWS sockets with `poll`; it
must not use an idle backoff loop. SWS disconnect and partial non-blocking
writes are terminal/flow-control conditions, respectively, rather than events
that may be silently discarded. The Wayland client socket stays first in the
wait set: Scarlet's bounded multi-handle fallback registers that descriptor as
its immediate wake source and periodically rescans SWS until native wait-set
registration is available.

Correlated SWS requests time out after five seconds. Only the initial SWS
handshake is retried, before the worker has created protocol resources. Follow
the default sampled lifecycle records in `logd` to distinguish registry, seat,
surface, SHM-pool, commit, window-creation, and later client failures.

GPU-backed Wayland buffers use `wp_scarlet_sgfx_v1` and SWS protocol 13 image
registration, retaining the same scene, serial, frame, destroy and release path.
See `guest_tests/linux_vulkan` for standard Vulkan loader and Wine/Box64 checks.

## Compound surfaces and viewports

The bridge implements `wl_subcompositor`/`wl_subsurface` v1 and `wp_viewporter`/
`wp_viewport` v1 when SWS advertises `SURFACE_SCENES` (protocol version 12).
Rebuild **both `sws` and `wayland_bridge`** to enable these globals. An older SWS
continues to expose the existing single-buffer interface without the new globals.

The bridge resolves Wayland synchronization and sends an atomic ordered surface
scene to SWS. It does not map, resample or composite client pixels. SWS owns
placement, crop/scale/orthogonal transform, alpha compositing and retained buffer
lifetimes. Simple surfaces retain the existing direct buffer path. With the SGFX
backend, each layer is uploaded from SHM into a reusable texture. SGFX applies
fractional crop, orthogonal transform, scaling and premultiplied source-over
blending into a retained GPU scene texture. SWS then applies window opacity,
rounded clipping and Overview transforms to that texture as a group. There is
no CPU precomposition or GPU readback in this path. Only changed scene serials
are rendered again; source textures are reused across same-size buffer swaps.
The software backend/fallback uses the CPU scene renderer (nearest-neighbour);
SGFX uses linear sampling. This does not add dma-buf/EGL buffer import to the bridge.

Child commits default to synchronized mode. Parent commits apply cached child
state; desynchronized children still inherit synchronized ancestors. Positions
and stacking are captured with the parent's commit, while viewport changes are
captured with the owning surface's commit. Children may extend beyond their
parent. Pointer hit testing uses the committed stacking order and input regions;
keyboard focus stays on the root surface. Hidden/cached children keep their
client buffers until those references are retired, even after SWS releases a
previously visible use. Frame callbacks are paced by the root's SWS frame fence.

Regression cases are in `scene.rs` (transactions, nested synchronization, viewport
coordinates and input), `sws-protocol/src/surface_scene.rs` (wire validation) and
`sws/surface_scene.rs` (pixel sampling/transform geometry) and
`sgfx_ir_support.rs` (UV vertex layout and premultiplied blend pipeline).

Protocol references (implementation written locally; no upstream implementation
or generated protocol source is vendored):

- [Wayland core protocol](https://gitlab.freedesktop.org/wayland/wayland/-/blob/main/protocol/wayland.xml)
- [Viewporter protocol](https://gitlab.freedesktop.org/wayland/wayland-protocols/-/blob/main/stable/viewporter/viewporter.xml)

### Output scale and input coordinates

Mapped roots and subsurfaces receive `wl_surface.enter`/`leave` for the SWS
output, including when a client binds the output after mapping. Output scale
changes send `wl_output.scale` and `done` to version 2+ objects. Scene destination
sizes, child positions and hit testing use the output scale; each surface's
buffer scale only controls sampling its pixels. A version 1 output receives
neither scale nor done. Integer output scaling is supported; fractional scaling
is not advertised.

Interactive move requests continue through SWS's pointer grab handling. The
bridge logs receipt once per move request and propagates send errors instead of
silently discarding them. Window movement still requires a live pointer grab.
