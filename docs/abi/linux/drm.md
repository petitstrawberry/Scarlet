# Linux DRM Compatibility Layer

This document describes Scarlet's Linux DRM (Direct Rendering Manager) compatibility layer.

## Overview

The DRM compatibility layer is isolated in `kernel/src/abi/linux/drm/` and provides Linux applications with standard DRM ioctls while internally using Scarlet's OS-independent graphics abstractions.

## Architecture (v2)

The v2 architecture promotes graphics buffers to **First-Class Kernel Objects**. This integrates graphics memory management with the OS's native handle system while maintaining Linux ABI compatibility.

```
┌─────────────────────────────────────────┐
│      Linux Applications (DRM API)       │
│  (Uses u32 GEM handles, e.g., 1, 2)     │
└──────────────────┬──────────────────────┘
                   │ DRM ioctls
                   ▼
┌─────────────────────────────────────────┐
│  kernel/src/abi/linux/drm               │
│  (Linux DRM Compatibility Layer)        │
│                                         │
│  ┌───────────────────────────────────┐  │
│  │ DrmFile (FileObject)              │  │
│  │ • Maps GEM ID -> Arc<KernelObject>│  │
│  └───────────────────────────────────┘  │
└──────────────────┬──────────────────────┘
                   │ Arc<GraphicsBuffer>
                   ▼
┌─────────────────────────────────────────┐
│  kernel/src/object                      │
│  (Kernel Object System)                 │
│                                         │
│  ┌───────────────────────────────────┐  │
│  │ KernelObject::GraphicsBuffer      │  │
│  │ • Implements ControlOps           │  │
│  │ • Implements MemoryMappingOps     │  │
│  └───────────────────────────────────┘  │
└──────────────────┬──────────────────────┘
                   │ Trait calls
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

- **DRM_IOCTL_MODE_CREATE_DUMB**: Creates dumb buffer (backed by `GraphicsBuffer`)
- **DRM_IOCTL_MODE_MAP_DUMB**: Maps dumb buffer for CPU access  
- **DRM_IOCTL_MODE_DESTROY_DUMB**: Destroys dumb buffer

### Display Operations

- **DRM_IOCTL_MODE_PAGE_FLIP**: Performs page flip operation

## v2 Implementation Details

The v2 implementation introduces a robust object model:

- **DrmFile**: Represents an open DRM file descriptor. It maintains a translation table from Linux "GEM handles" (u32) to Scarlet `KernelObject`s.
- **GraphicsBuffer**: A new `KernelObject` variant representing a contiguous region of graphics memory.
- **Double Ownership**: Buffers are owned by both the creating task (via `HandleTable`) and the `DrmFile` session. This ensures safety even if the task dies or the file is shared.

## DRM to Scarlet Mapping

| DRM Concept | Scarlet Concept | Implementation |
|-------------|-----------------|----------------|
| CRTC | Display output | 1:1 with GraphicsDevice |
| Connector | Physical connection | Single connector per device |
| Encoder | Signal conversion | Abstracted away |
| Dumb buffer | `GraphicsBuffer` | `KernelObject::GraphicsBuffer` |
| GEM Handle | Session ID | Local u32 ID mapped to `Arc<KernelObject>` |
| Page flip | Buffer swap | `PageFlipCapable` trait or fallback |

## ioctl Implementation Details

### DRM_IOCTL_MODE_CREATE_DUMB

Creates a dumb buffer:
1. Allocates a `GraphicsBuffer` via `GraphicsManager`.
2. Wraps it in `KernelObject::GraphicsBuffer`.
3. Inserts it into the current task's `HandleTable` (returning a native handle).
4. Inserts it into the `DrmFile`'s GEM map (returning a GEM handle).
5. Returns the GEM handle to userspace.

### DRM_IOCTL_MODE_MAP_DUMB

Maps a dumb buffer for CPU access:
1. Looks up the buffer using the GEM handle.
2. Returns a "fake offset" that encodes the buffer's identity.
3. When userspace calls `mmap` with this offset, the kernel resolves it back to the `GraphicsBuffer` and maps the physical memory.

### DRM_IOCTL_MODE_DESTROY_DUMB

Destroys a dumb buffer:
1. Removes the entry from the `DrmFile`'s GEM map.
2. Drops the `Arc<KernelObject>`.
3. If no other references exist (e.g., in `HandleTable`), the memory is freed.

### DRM_IOCTL_MODE_PAGE_FLIP

Performs page flip:
1. Looks up the buffer using the GEM handle.
2. Validates it is a `GraphicsBuffer`.
3. Calls `GraphicsManager::flush_framebuffer` (or hardware flip if supported).

## DrmFile Context

Each open `/dev/dri/cardX` file descriptor corresponds to a `DrmFile` struct:

```rust
pub struct DrmFile {
    /// Connection to the physical device
    device_id: usize,
    /// Map from GEM handle (u32) to Kernel Object
    gem_handles: Mutex<HashMap<u32, Arc<KernelObject>>>,
    /// Next available GEM handle ID
    next_gem_id: Mutex<u32>,
}
```

This structure ensures that GEM handles are local to the open file, matching Linux behavior.

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
