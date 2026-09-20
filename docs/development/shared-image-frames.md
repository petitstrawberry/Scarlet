# Shared decoded images

## Contract

Decoded images are OS capabilities, independent of the codec, decoder and
renderer. A shared image describes its format (FourCC), coded extent, visible
crop, memory modifier, buffer sizes, plane-to-buffer mapping, pitches, offsets,
and color encoding/range/chroma positions. Metadata and backing are immutable
for the capability lifetime. User space never supplies a physical address.

The initial export contract is **ready and read-only**: a producer may publish
an image only after its writes and cache maintenance have completed. Holding
an exported capability leases the backing. A consumer must retain that lease
until all accepted reads retire, including error recovery. Closing a decoder
does not invalidate exported images. This avoids an implicit producer/consumer
race without inventing a decoder-specific release ioctl. A later asynchronous
producer API can add an acquire fence explicitly rather than changing the
meaning of a ready image.

The video client negotiates shared output explicitly. The existing mapped NV12
API stays available. A consumer imports a shared image only when it supports
the full format/modifier/color combination; unsupported combinations fail
before GPU submission. Multi-plane images may use distinct backing buffers.
NV12 storage does not imply a BT.601 or BT.709 conversion.

## Consumers

The shared image contract belongs below SGFX so it can also serve display
planes, other graphics APIs, cameras and other decoders. SGFX imports an image
for sampling and performs YUV conversion/scaling during composition. It does
not own decoder sessions. Direct scanout can consume the same capability when
the compositor has a suitable plane and visibility/transform allow it.

ScarletUI uses its existing external image paint command and GPU completion
and SWS buffer release mechanisms. Completed image imports are detached before
descriptor-compatible SGFX texture slots are recycled. The video player paints
the image, then small control/debug buffers in the same paint order. It does
not allocate an intermediate video canvas or change compositor scanout.

## Implemented support

- `scarlet-abi::shared_image`: versioned descriptors and ready, read-only image
  capabilities. A kernel producer implements `SharedImageBacking`; no NVDEC
  types or decoder-session handles enter the shared contract.
- `VIDEO_SET_OUTPUT_MODE` / `VIDEO_DEQUEUE_IMAGE`: opt-in per-session native
  output. Existing mapped NV12 clients continue to use the previous ABI.
- `GPU_IMPORT_SHARED_IMAGE`: imports an image lease into a GPU without copying
  pixels. The consumer supplies its explicit color interpretation.
- GM20B imports linear NV12 and NVIDIA uncompressed kind `0xfe`, 2-GOB
  block-linear NV12. Plane offsets/pitches and separate backing buffers are
  validated before mapping. NVDEC exports the latter after its completion fence.
- SGFX adds sampled-only `TextureFormat::Nv12` and `YcbcrConversion`, with
  BT.601/BT.709, full/limited range, and cosited/midpoint chroma. It rejects
  direct writes, render targets, mipmaps and unsupported conversions.
- H.264 VUI metadata supplies matrix, range and chroma location to the player.
  Unspecified matrix uses an explicit BT.601 fallback. HDR/BT.2020 and unsupported
  chroma positions are rejected. Conversion produces encoded RGB; this is not a
  full display color-management or HDR tone-mapping pipeline.

SGFX formats are plane-aware: `bytes_per_pixel()` returns `None` for NV12, while
`byte_size()` accounts for both planes and odd chroma extents. Backends without
NV12 sampling report unsupported instead of interpreting it as packed RGB.

## Validation (Switch, 2026-09-20)

- 12 real GPU draw/readback cases passed: CPU-produced linear and block-linear
  NV12, both shader variants, red/blue/grey, BT.601/BT.709, limited/full range,
  visible crop and poisoned padding. The tests run during GPU initialization.
- Full SGFX compilation of a native NV12 draw followed by an RGB overlay passed
  for both layouts. The relocation retains read authority for the UV plane too.
- ABI tests cover separate plane buffers, odd sizes, truncation, overflow and
  unused fields. H.264 tests cover VUI color values and reset when VUI is absent.
- ScarletUI cycled 2,048 unique image sources using one logical texture slot,
  releasing each old source. All 45 renderer tests passed.
- The 1,920×1,080 H.264 test clip (2,488 access units) completed through
  `output=shared-image` on Switch. The user confirmed normal colors, controls
  and debug overlay, and visually about 24 fps. This is a user observation,
  not an independently measured compositor presentation rate. Audio stayed 0.

Hardware bring-up found and fixed a stale tile-mode whitelist and a TIC table
limit that excluded the UV descriptor. The player also preserves full local
mouse-event coordinates and the control renderer's minimum height when drawing
the small overlay buffer.

The local Switch userspace Cargo configuration points at the coordinated
Scarlet, SGFX, ScarletUI and Chromebook compatibility changes. Publishing them
requires updating the respective Git dependency pins together.

## References

- [Linux pixel buffer exchange](https://docs.kernel.org/userspace-api/dma-buf-alloc-exchange.html)
  separates pixel format from storage modifier and explains multi-plane storage.
- [Khronos YCbCr conversion](https://docs.vulkan.org/guide/latest/extensions/VK_KHR_sampler_ycbcr_conversion.html)
  separates storage from color model, range and chroma reconstruction.
