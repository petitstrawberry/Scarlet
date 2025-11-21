# Linux DRM Compatibility Layer

This document describes Scarlet's Linux DRM (Direct Rendering Manager) compatibility layer.

## Overview

The DRM compatibility layer is isolated in `kernel/src/abi/linux/drm/` and provides Linux applications with standard DRM ioctls while internally using Scarlet's OS-independent graphics abstractions.

## Architecture

```
┌─────────────────────────────────────────┐
│      Linux Applications (DRM API)       │
└──────────────────┬──────────────────────┘
                   │ DRM ioctls
                   ▼
┌─────────────────────────────────────────┐
│  kernel/src/abi/linux/drm               │
│  (Linux DRM Compatibility Layer)        │
│  • types.rs - DRM structures            │
│  • ioctls.rs - ioctl handlers           │
└──────────────────┬──────────────────────┘
                   │ GraphicsManager calls
                   ▼
┌─────────────────────────────────────────┐
│  kernel/src/device/graphics             │
│  (Scarlet Graphics Subsystem)           │
└─────────────────────────────────────────┘
```

## Supported DRM ioctls

### Device Information

- **DRM_IOCTL_VERSION**: Reports Scarlet DRM driver version information

### Mode Setting (KMS)

- **DRM_IOCTL_MODE_GETRESOURCES**: Lists CRTCs, connectors, encoders
- **DRM_IOCTL_MODE_GETCRTC**: Gets CRTC configuration
- **DRM_IOCTL_MODE_SETCRTC**: Sets CRTC configuration (future)

### Dumb Buffers

- **DRM_IOCTL_MODE_CREATE_DUMB**: Creates dumb buffer
- **DRM_IOCTL_MODE_MAP_DUMB**: Maps dumb buffer for CPU access  
- **DRM_IOCTL_MODE_DESTROY_DUMB**: Destroys dumb buffer

### Display Operations

- **DRM_IOCTL_MODE_PAGE_FLIP**: Performs page flip operation

## MVP Implementation

The MVP (Minimum Viable Product) implementation provides:

- **Single display**: One CRTC, one connector, one encoder
- **Dumb buffers**: Simple CPU-accessible buffers
- **Page flip**: Implemented via memcpy + flush (software fallback)
- **No 3D**: Only 2D framebuffer operations
- **No GEM**: No GPU memory management (dumb buffers only)

## DRM to Scarlet Mapping

| DRM Concept | Scarlet Concept | Implementation |
|-------------|-----------------|----------------|
| CRTC | Display output | 1:1 with GraphicsDevice |
| Connector | Physical connection | Single connector per device |
| Encoder | Signal conversion | Abstracted away |
| Dumb buffer | Memory region | Allocated via allocate_raw_pages |
| Page flip | Buffer swap | memcpy + flush (MVP) |
| Framebuffer | Display buffer | FramebufferConfig + address |

## ioctl Implementation Details

### DRM_IOCTL_VERSION

Returns version information:
- Driver name: "scarlet"
- Version: 1.0.0
- Description: "Scarlet DRM driver"

### DRM_IOCTL_MODE_GETRESOURCES

Returns resource counts and IDs:
- 1 CRTC (ID: 1)
- 1 Connector (ID: 1)
- 1 Encoder (ID: 1)
- Framebuffer dimensions

### DRM_IOCTL_MODE_GETCRTC

Returns CRTC configuration:
- Current mode (resolution, refresh rate)
- Framebuffer ID
- Position (x, y)
- Enabled state

### DRM_IOCTL_MODE_CREATE_DUMB

Creates a dumb buffer:
- Validates dimensions and bpp
- Calculates pitch and size
- Allocates memory via `allocate_raw_pages`
- Returns handle and size

**Security**: Validates dimensions to prevent integer overflow.

### DRM_IOCTL_MODE_MAP_DUMB

Maps a dumb buffer for CPU access:
- Verifies handle exists
- Returns fake offset (actual address stored internally)
- Applications can mmap() using this offset

### DRM_IOCTL_MODE_DESTROY_DUMB

Destroys a dumb buffer:
- Verifies handle exists
- Frees allocated memory
- Removes from context

### DRM_IOCTL_MODE_PAGE_FLIP

Performs page flip:
- Retrieves source buffer by handle
- Copies to framebuffer via memcpy
- Flushes to display

**MVP Note**: This is a software implementation. Future versions will support hardware page flipping via `PageFlipCapable` trait.

## DrmDeviceContext

Each device has a `DrmDeviceContext` that manages:

- **Dumb buffers**: Handle → (address, size) mapping
- **Handle allocation**: Monotonically increasing handles
- **Device ID**: Associated GraphicsDevice

### Handle Management

Handles start at 1 and increment. When handles are exhausted (u32::MAX), the system panics (unlikely in practice).

## Usage Example

### Simple Framebuffer Drawing

```c
// Open DRM device
int fd = open("/dev/dri/card0", O_RDWR);

// Get resources
struct drm_mode_card_res res = {0};
ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, &res);

// Create dumb buffer
struct drm_mode_create_dumb create = {
    .width = 1024,
    .height = 768,
    .bpp = 32,
};
ioctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &create);

// Map buffer
struct drm_mode_map_dumb map = {
    .handle = create.handle,
};
ioctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &map);

void *buffer = mmap(NULL, create.size, PROT_READ | PROT_WRITE, 
                     MAP_SHARED, fd, map.offset);

// Draw to buffer
memset(buffer, 0xFF, create.size);  // Fill with white

// Page flip to display
struct drm_mode_crtc_page_flip flip = {
    .crtc_id = 1,
    .fb_id = create.handle,
};
ioctl(fd, DRM_IOCTL_MODE_PAGE_FLIP, &flip);
```

## Future Enhancements

### Hardware Page Flipping

When GraphicsDevice implements `PageFlipCapable`:
1. Detect capability via trait check
2. Use hardware flip instead of memcpy
3. Return vblank event when supported

### GEM (Graphics Execution Manager)

Future support for:
- GPU memory management
- Buffer sharing between processes
- Direct rendering

### Multiple Displays

Support for:
- Multiple CRTCs
- Multiple connectors
- Display hotplug events

### 3D Rendering

Via future `RenderDevice` trait:
- Command buffer submission
- Shader compilation
- GPU synchronization

## Implementation Notes

### Pointer Safety

All ioctl handlers use `read_unaligned` and `write_unaligned` for DRM structures to prevent undefined behavior on architectures requiring alignment.

### Buffer Validation

Input dimensions are validated to prevent:
- Integer overflow in size calculations
- Excessive memory allocation
- Invalid pixel formats

### Error Handling

DRM ioctls return standard Linux error codes:
- `-EINVAL`: Invalid parameters
- `-ENOENT`: Resource not found
- `-ENOMEM`: Out of memory

## Testing

The DRM layer includes 13 test cases:
- DrmDeviceContext creation and management
- Handle allocation and overflow protection
- Pointer translation
- ioctl validation (invalid pointer handling)

All tests use `#[test_case]` for no_std compatibility.

## References

- [Linux DRM Documentation](https://www.kernel.org/doc/html/latest/gpu/drm-uapi.html)
- [DRM Mode Setting](https://www.kernel.org/doc/html/latest/gpu/drm-kms.html)
- [Mesa DRI Implementation](https://www.mesa3d.org/)
- Scarlet Graphics Core: `docs/graphics/core-abstraction.md`
