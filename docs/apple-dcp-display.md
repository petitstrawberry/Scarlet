# Apple DCP native display

The `scarlet-driver-dcp` module drives the internal Apple Silicon panel without
using the Limine framebuffer as its render or scanout storage. It follows the
handoff used by m1n1:

1. Rebuild DCP and display DART page tables from the reserved-memory
   `iommu-addresses` records added by m1n1.
2. Resume the DCP ASC with RTKit and start the AFK/EPIC `disp0-service` endpoint.
3. Query timing and color modes through the DCP iBoot protocol, power the panel,
   and select the best valid mode at or below 60 Hz when available.
4. Allocate two 16 KiB-aligned native scanout surfaces and map both into the
   DCP and display DARTs.
5. Expose both scanouts through the `/dev/display0` swapchain ABI. Userspace
   composes into a persistent backbuffer, copies the required buffer-age damage
   into the inactive scanout, and submits that completed scanout for a DCP swap.

The legacy display mmap remains available when direct scanout is unsupported.
Swapchain-aware userspace maps each DCP scanout once and selects completed
buffers explicitly with `DISPLAY_PRESENT_BUFFER`.

The Apple project enables the driver in
`projects/aarch64-apple-limine-full/scarlet.toml`. A successful probe logs a line
like:

```text
[apple-dcp] native panel 2560x1600 @ 60.00 Hz, handoff maps dcp=... display=...
```

The driver currently uses DCP's iBoot service and BGRA8888 scanout. It does
not yet expose brightness, variable refresh rate, color-management controls, or
asynchronous vblank completion events. Presentation is synchronous: after the
EPIC swap-completion callback, the previous front buffer becomes available for
reuse.
