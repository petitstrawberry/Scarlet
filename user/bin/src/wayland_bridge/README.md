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

For an isolated test instance, set `WAYLAND_DISPLAY=wayland-test` on both the
bridge and the client. Relative display names are resolved under
`XDG_RUNTIME_DIR` (default `/tmp`); absolute names are used as socket paths.
This allows testing a new bridge binary without replacing the running service.

`wl_shm_pool.resize` requires the client to grow its backing file first. The
bridge updates the pool extent and asks SWS to remap its retained handle; it
does not resize the client's file or require it to be a native SharedMemory
object. Linux memfd and shm_open files use the same mappable-handle path.

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

## ScarletUI window decorations

With `SURFACE_SCENES`, the bridge also advertises
`zxdg_decoration_manager_v1` v1. The default negotiated mode is server-side;
explicit client-side requests are honored. A mode changes only after its
`xdg_surface.configure` is acknowledged and committed. Clients that do not
create a decoration object retain the existing client-side surface behavior.

Server-side decoration uses the pinned ScarletUI `Window` through
`scarlet-ui-core`, including its titlebar, controls, border and rounded clip.
The client image stays in its existing GPU/SHM resource. The bridge publishes
immutable titlebar/edge strips above the client image and clips the image to
ScarletUI's rounded outline; it never reads back the game image. Chrome is
reused between game frames and redrawn only when the UI becomes dirty or its
title, size or scale changes. SWS retires old chrome resources after scene use.

Titlebar drag, minimize, maximize/restore and close actions use SWS window
management. Close sends `xdg_toplevel.close` to the client. Configure sizes and
pointer coordinates exclude the ScarletUI decoration insets. Initial SSD
windows are centered using their outer size. Fullscreen suppresses the frame;
client-side mode leaves drawing and clipping to the client. Window title and
size limits are forwarded to SWS, adding frame dimensions for SSD.

The normal build includes decorations. A minimal bridge build needs:

```sh
cd user/bin
cargo build --release --no-default-features --features wayland-decoration \
    --bin wayland-bridge --target ../targets/aarch64-unknown-scarlet-elf.json
```

Console-only builds can still use `--no-default-features` without UI crates.
The live SHM fixture `guest_tests/linux_zink/wayland-decoration.c` uses the
official generated xdg-shell and xdg-decoration client code. Each Enter key
advances SSD, CSD, SSD, fullscreen and restored SSD, with a frame callback
before each stage. It tests the bridge without an SDL/Vulkan dependency.

This is bounded by SWS scene layer limits and a 16-million-pixel decoration
limit. Interactive edge resize remains the existing SWS behavior. The fixture
does not establish complete xdg-shell or toolkit conformance.

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
