# Display Present Paths

Scarlet uses two CPU-composited display presentation paths.

## Display Surface Path

Display-system clients open `/dev/display0` through `DisplaySurface`, not the
legacy framebuffer node. Clients that track damage in screen coordinates call
`DisplaySurface::present_region` for each bounded damage rectangle after CPU
composition into the display backing store.

`DISPLAY_GET_INFO` reports both `buffer_len` and a `backing_id`. Display
clients reuse an mmap only while both values match. This matters for resize and
mode-change paths where the graphics device may allocate a new backing store
whose page-aligned size is identical to the previous one.

That call maps to `DISPLAY_PRESENT_REGION`, and the kernel forwards the region
to the graphics device as
`present_current_framebuffer_region(region)`. For virtio-gpu this becomes:

```text
TRANSFER_TO_HOST_2D(region)
RESOURCE_FLUSH(region)
```

The virtio-gpu driver is passive in this path. It does not schedule a periodic
full-screen flush; it only transfers and flushes regions requested by the
display stack.

## Legacy /dev/fb0 Path

`/dev/fb0` is a legacy compatibility alias for the display backing store. It is
not the primary display API and does not own a separate scanout buffer.

The mmap path keeps the VM mapping writable at the object level, but installs
read-only PTEs for non-store faults and after every compatibility present. A
store fault marks the framebuffer dirty, temporarily installs a writable PTE for
that page, and arms a 16 ms one-shot timer. When the timer expires, the compat
path performs a full-frame present, clears the dirty flag, and write-protects the
tracked framebuffer mappings again.

`write_at` follows the same compatibility model: it writes into the display
backing store, marks the legacy framebuffer dirty, and lets the one-shot timer
perform the full-frame present. `FBIO_FLUSH` remains the explicit synchronization
command for legacy programs; it performs an immediate full-frame present and
write-protects the tracked mappings.

Region presents intentionally belong to `/dev/displayX`, not the framebuffer
compatibility node.
