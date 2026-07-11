# Apple DCP native display

The `scarlet-driver-dcp` module drives the internal Apple Silicon panel without
using the Limine framebuffer as its render or scanout storage. It follows the
handoff used by m1n1:

1. Rebuild DCP and display DART page tables from the reserved-memory
   `iommu-addresses` records added by m1n1.
2. Resume the DCP ASC with RTKit and start the AFK/EPIC `disp0-service` endpoint.
3. Query timing and color modes through the DCP iBoot protocol, power the panel,
   and select the best valid mode at or below 60 Hz when available.
4. Allocate one stable CPU render surface and two 16 KiB-aligned native scanout
   surfaces. Both scanout surfaces are mapped into the DCP and display DARTs.
5. On `DISPLAY_PRESENT_REGION`, copy the damage into the inactive scanout,
   submit a DCP surface swap, and copy the same damage into the old scanout.
   Keeping both scanouts coherent makes later damage-only flips safe.

The stable render surface preserves the existing `/dev/display0` mmap contract:
userspace does not have to remap after every flip. The two scanout buffers remain
kernel-owned and are never exposed as legacy framebuffer memory.

The Apple project enables the driver in
`projects/aarch64-apple-limine-full/scarlet.toml`. A successful probe logs a line
like:

```text
[apple-dcp] native panel 2560x1600 @ 60.00 Hz, handoff maps dcp=... display=...
```

The driver currently uses DCP's iBoot service and XRGB2101010 scanout. It does
not yet expose brightness, variable refresh rate, color-management controls, or
asynchronous vblank completion events. Presentation is synchronous: after the
EPIC swap acknowledgement, the driver conservatively waits one refresh period
before recycling the previous front buffer.
