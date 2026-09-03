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

GPU-backed Wayland buffers should add another registration/import backing and
reuse the existing generic commit, serial, frame, destroy, and release path.
